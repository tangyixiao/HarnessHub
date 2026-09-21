//! 外部 Harness 的二进制与版本探测。
//!
//! Windows 现实（本机实测）：npm 全局安装的 Harness 是 shim —— `codex`（bash）、
//! `codex.cmd`、`codex.ps1` 三份并存，而 `PATHEXT` **不含 `.PS1`**。
//! 因此查找顺序必须显式覆盖 `.ps1`，否则「只有 ps1 的机器」会被误判为未安装。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::Result;

/// 无扩展名之外的候选扩展名，顺序即优先级。
const EXECUTABLE_EXTENSIONS: &[&str] = &["exe", "cmd", "bat", "com", "ps1"];

/// 给定目录下的候选可执行路径，顺序即优先级。
///
/// 无扩展名优先（Unix 与 npm 的 bash shim），随后按平台优先级枚举扩展名。
pub fn candidate_paths(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut candidates = vec![dir.join(name)];
    for extension in EXECUTABLE_EXTENSIONS {
        candidates.push(dir.join(format!("{name}.{extension}")));
    }
    candidates
}

/// 从 `--version` 输出中抽取第一个形如 `1.2` / `1.2.3` 的 token。
///
/// 刻意不做完整 semver 校验：不同 Harness 的版本输出五花八门
/// （`codex-cli 0.152.1`、`v1.2.3`、`gemini 0.9.0-beta`），
/// 这里只要求「前两段是数字」，其余部分原样保留。
pub fn parse_version(raw: &str) -> Option<String> {
    raw.split_whitespace()
        .map(|token| token.trim_start_matches('v'))
        .find(|token| {
            let mut parts = token.split('.');
            match (parts.next(), parts.next()) {
                (Some(major), Some(minor)) => {
                    !major.is_empty()
                        && !minor.is_empty()
                        && major.chars().all(|c| c.is_ascii_digit())
                        && minor.chars().all(|c| c.is_ascii_digit())
                }
                _ => false,
            }
        })
        .map(str::to_string)
}

/// 探测外部 Harness 所需的宿主能力。
///
/// 抽成 trait 是为了让检测逻辑可以脱离真实文件系统与真实进程测试：
/// 测试用可编程的假宿主，生产用 [`SystemHostProbe`]。
pub trait HostProbe: Send + Sync {
    /// 在 PATH 中查找可执行文件。
    fn find_executable(&self, name: &str) -> Option<PathBuf>;

    /// 执行 `<executable> --version` 并解析版本号。
    fn read_version(&self, executable: &Path) -> Result<Option<String>>;

    /// 目录是否存在（用于判断数据目录，不做任何写入）。
    fn dir_exists(&self, path: &Path) -> bool;

    /// 当前用户主目录。
    fn home_dir(&self) -> Option<PathBuf>;
}

/// 真实宿主实现：读 PATH、起子进程、查文件系统。
pub struct SystemHostProbe;

impl SystemHostProbe {
    pub fn new() -> Self {
        Self
    }

    fn path_entries() -> Vec<PathBuf> {
        std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect())
            .unwrap_or_default()
    }
}

impl Default for SystemHostProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl HostProbe for SystemHostProbe {
    fn find_executable(&self, name: &str) -> Option<PathBuf> {
        let mut seen = HashSet::new();

        for dir in Self::path_entries() {
            if !seen.insert(dir.clone()) {
                continue;
            }
            for candidate in candidate_paths(&dir, name) {
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }

        None
    }

    fn read_version(&self, executable: &Path) -> Result<Option<String>> {
        let output = Command::new(executable).arg("--version").output()?;
        if !output.status.success() {
            return Ok(None);
        }
        // 有的 CLI 把版本写到 stderr，因此两边都看。
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        Ok(parse_version(&stdout).or_else(|| parse_version(&stderr)))
    }

    fn dir_exists(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn home_dir(&self) -> Option<PathBuf> {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn candidate_paths_prefers_extensionless_then_platform_suffixes() {
        let dir = PathBuf::from("D:/npm-global");
        let candidates = candidate_paths(&dir, "codex");

        assert_eq!(candidates[0], dir.join("codex"), "无扩展名必须优先");

        if cfg!(windows) {
            // 本机实测：npm 全局包同时提供 codex.cmd 与 codex.ps1。
            assert!(candidates.contains(&dir.join("codex.cmd")));
            assert!(candidates.contains(&dir.join("codex.ps1")));
        }
    }

    #[test]
    fn candidate_paths_does_not_duplicate_entries() {
        let dir = PathBuf::from("/usr/local/bin");
        let candidates = candidate_paths(&dir, "opencode");

        let mut unique = candidates.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(candidates.len(), unique.len(), "候选列表不应有重复项");
    }

    #[test]
    fn parse_version_extracts_semver_from_cli_output() {
        assert_eq!(
            parse_version("codex-cli 0.152.1"),
            Some("0.152.1".to_string())
        );
        assert_eq!(parse_version("v1.2.3"), Some("1.2.3".to_string()));
        assert_eq!(
            parse_version("codex 0.152.1\n"),
            Some("0.152.1".to_string())
        );
        assert_eq!(parse_version("codex-cli 1.2"), Some("1.2".to_string()));
    }

    #[test]
    fn parse_version_returns_none_when_no_version_present() {
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("codex-cli"), None);
        assert_eq!(parse_version("no digits here"), None);
    }
}
