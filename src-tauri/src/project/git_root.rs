//! repo root 检测：`git -C <path> rev-parse --show-toplevel`。
//!
//! 服务层通过 [`GitRootProbe`] 注入探测，不在服务里直接 `Command::new`，
//! 与 `harness::probe::HostProbe` 同一模式。

use crate::error::{Error, Result};
use std::path::Path;
use std::process::Command;

/// 探测一个目录所属的 Git 仓库 root。
///
/// - `Ok(Some(root))`：目录在某个 Git 仓库里，root 是 git 的标准输出（正斜杠）。
/// - `Ok(None)`：git 正常执行但目录不是仓库（或目录不存在）。
/// - `Err`：探测本身失败（例如 `git` 不在 PATH 上）。
pub trait GitRootProbe: Send + Sync {
    fn repo_root(&self, path: &Path) -> Result<Option<String>>;
}

/// 真机探测：不改进程 cwd，用 `-C` 指定目录。
pub struct SystemGitRootProbe;

impl GitRootProbe for SystemGitRootProbe {
    fn repo_root(&self, path: &Path) -> Result<Option<String>> {
        let output = Command::new("git")
            .args(["-C"])
            .arg(path)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map_err(|error| {
                Error::InvalidInput(format!(
                    "无法执行 git（确认 git 已安装并在 PATH 上）：{error}"
                ))
            })?;

        if !output.status.success() {
            return Ok(None);
        }

        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if root.is_empty() {
            return Ok(None);
        }
        Ok(Some(root))
    }
}
