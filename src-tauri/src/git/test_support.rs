//! git 模块共享的测试夹具：隔离环境下的真实临时 Git 仓库。
//!
//! 只在 `cfg(test)` 下编译。集成测试（`tests/git_snapshot.rs`）在另一个 crate，
//! 自带独立副本 —— 这里只服务单元测试。
//!
//! `fixture_git` 会剥掉宿主继承的 `GIT_*` 环境变量并禁用系统/全局配置，
//! 防止本机 Git 环境污染一次性仓库（真实事故：CI 上 `GIT_DIR` 泄漏）。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 构造一个与宿主 Git 环境隔离的 `git` 命令。
pub(crate) fn fixture_git(root: &Path) -> Command {
    let support = root.join(".git/hh-fixture");
    std::fs::create_dir_all(support.join("hooks")).unwrap();
    std::fs::create_dir_all(support.join("template")).unwrap();
    let config = support.join("empty-config");
    std::fs::write(&config, b"").unwrap();
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy().to_ascii_uppercase();
        if matches!(
            name.as_str(),
            "GIT_DIR"
                | "GIT_WORK_TREE"
                | "GIT_INDEX_FILE"
                | "GIT_COMMON_DIR"
                | "GIT_PREFIX"
                | "GIT_OBJECT_DIRECTORY"
                | "GIT_ALTERNATE_OBJECT_DIRECTORIES"
                | "GIT_CONFIG"
                | "GIT_CONFIG_COUNT"
                | "GIT_CONFIG_PARAMETERS"
        ) || name.starts_with("GIT_CONFIG_KEY_")
            || name.starts_with("GIT_CONFIG_VALUE_")
        {
            command.env_remove(key);
        }
    }
    command
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", &config)
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("GIT_TEMPLATE_DIR", support.join("template"))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .args([
            "-c",
            "user.name=Harness Hub Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "tag.gpgsign=false",
        ])
        .arg("-c")
        .arg(format!(
            "core.hooksPath={}",
            support.join("hooks").display()
        ));
    command
}

/// 一次性临时 Git 仓库，`Drop` 时整体删除。
pub(crate) struct Repo(pub(crate) PathBuf);

impl Repo {
    pub(crate) fn new() -> Self {
        let root = std::env::temp_dir().join(format!("hh-git-capture-{}", uuid::Uuid::new_v4()));
        Self::at(root)
    }

    pub(crate) fn at(root: PathBuf) -> Self {
        std::fs::create_dir_all(&root).unwrap();
        let repo = Self(root);
        repo.git(&["init", "-q"]);
        repo
    }

    /// 跑一条 git 命令，失败即 panic（夹具本身必须可靠）。
    pub(crate) fn git(&self, args: &[&str]) {
        let status = fixture_git(&self.0).args(args).status().unwrap();
        assert!(status.success());
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Default for Repo {
    fn default() -> Self {
        Self::new()
    }
}
