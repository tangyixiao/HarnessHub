//! 结构化启动描述：`LaunchSpec`。
//!
//! 设计边界（docs/adr/0008-launch-spec-boundary.md）：
//!
//! ```text
//! HarnessAdapter::build_launch_spec()  ← 平台差异在这一层解决
//!               ↓
//!           LaunchSpec                 ← 纯数据：program / args / cwd / env / runtime
//!               ↓
//!          PTY Manager                 ← 只负责执行，不理解任何 Harness 语义
//! ```
//!
//! **PTY 层不得拼 shell 命令字符串**，也不得 `shell("codex ...")`。原因：
//!
//! - Windows npm 装出来的是 shim：`codex`（POSIX 脚本）、`codex.cmd`、`codex.ps1`
//!   —— 其中 `.ps1` 不能被 CreateProcess 直接执行，必须交给 `pwsh -File`；
//! - 路径可能含空格；参数需要正确的转义规则；
//! - 拼接字符串会引入 shell 注入面；
//! - 未来要接 Linux binary / WSL / SSH，各自的可执行形态都不同。

use std::path::{Path, PathBuf};

/// 一次启动所需的全部信息。PTY 层只消费这个结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    /// 真正要执行的文件（可能是 shim 的壳，例如 pwsh）。
    pub program: PathBuf,
    /// 传给 program 的参数（已包含 shim 所需的前置参数）。
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// 显式追加的环境变量；不继承「隐式拼出来的 shell 环境」。
    pub env: Vec<(String, String)>,
    /// 在哪个运行目标上启动（v0.1 恒为 `local`，为 WSL / SSH 预留）。
    pub runtime_target_id: String,
}

impl LaunchSpec {
    /// 便于测试与排障的可读描述。**不是**用来执行的命令字符串。
    pub fn describe(&self) -> String {
        let mut parts = vec![self.program.to_string_lossy().into_owned()];
        parts.extend(self.args.iter().cloned());
        parts.join(" ")
    }
}

/// Windows 上必须通过 pwsh 执行的扩展名。
fn needs_powershell_host(executable: &Path) -> bool {
    executable
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ps1"))
}

/// 把「检测到的可执行文件」转换成「真正要执行的 program + 前置参数」。
///
/// - `.cmd` / `.exe` / `.bat` / 无扩展名：直接执行（Windows 上 Rust 的
///   `std::process::Command` 会自行通过 `cmd.exe` 处理 `.cmd` / `.bat`）；
/// - `.ps1`：**不能**直接 CreateProcess，必须 `pwsh -NoLogo -NoProfile -File <脚本>`
///   （本机 `PATHEXT` 不含 `.PS1`，只有 npm 的 ps1 shim 时会踩到这里）。
pub fn program_for(executable: &Path) -> (PathBuf, Vec<String>) {
    if needs_powershell_host(executable) {
        return (
            PathBuf::from("pwsh"),
            vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-File".to_string(),
                executable.to_string_lossy().into_owned(),
            ],
        );
    }

    (executable.to_path_buf(), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_shims_are_executed_directly() {
        let (program, args) = program_for(Path::new("D:/npm-global/codex.cmd"));

        assert_eq!(program, PathBuf::from("D:/npm-global/codex.cmd"));
        assert!(args.is_empty(), "直接执行不应注入任何前置参数");
    }

    #[test]
    fn exe_and_extensionless_are_executed_directly() {
        for path in ["C:/tools/codex.exe", "/usr/local/bin/codex"] {
            let (program, args) = program_for(Path::new(path));

            assert_eq!(program, PathBuf::from(path));
            assert!(args.is_empty());
        }
    }

    #[test]
    fn powershell_shims_are_hosted_by_pwsh() {
        let (program, args) = program_for(Path::new("D:/npm-global/codex.ps1"));

        assert_eq!(program, PathBuf::from("pwsh"));
        assert_eq!(
            args,
            vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-File".to_string(),
                "D:/npm-global/codex.ps1".to_string(),
            ]
        );
    }

    #[test]
    fn powershell_detection_is_case_insensitive() {
        let (program, _) = program_for(Path::new("D:/npm-global/codex.PS1"));

        assert_eq!(
            program,
            PathBuf::from("pwsh"),
            "Windows 上扩展名大小写不敏感"
        );
    }

    /// 结构化 spec 不得退化成 shell 命令字符串。
    #[test]
    fn describe_is_for_humans_and_keeps_program_separate() {
        let spec = LaunchSpec {
            program: PathBuf::from("D:/npm-global/codex.cmd"),
            args: vec!["--version".to_string()],
            cwd: Some(PathBuf::from("D:/work")),
            env: Vec::new(),
            runtime_target_id: "local".to_string(),
        };

        assert_eq!(spec.describe(), "D:/npm-global/codex.cmd --version");
        assert_eq!(spec.program, PathBuf::from("D:/npm-global/codex.cmd"));
    }
}
