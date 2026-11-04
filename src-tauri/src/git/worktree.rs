//! Worktree 列表与创建（ROADMAP C10）。
//!
//! 纪律（spec：docs/specs/2026-10-25-worktree-add-design.md）：
//!   * `list` 纯读（`git worktree list --porcelain`，3 秒上限）；
//!   * `add` 是**显式用户动作**（`git worktree add`，10 秒上限——检出可能耗时），
//!     禁止写到 repo_root 内部（嵌套 worktree 会让观察层互相污染）；
//!   * 删除/合并全部留给 C11（涉及未提交变更的安全检查，单独设计）；
//!   * harness capability `worktree` 与本模块无关（那是「该 harness 能否在
//!     worktree 中跑」的声明），保持 false。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::runner::{self, Limits};
use crate::error::{Error, Result};

/// 一条 worktree（主树或链接树）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeEntry {
    /// 绝对路径（porcelain 原样）。
    pub path: String,
    /// HEAD commit（40/64 位 hex）；unborn 为 None。
    pub head: Option<String>,
    /// 短分支名（`refs/heads/main` → `main`）；detached 为 None。
    pub branch: Option<String>,
    /// 是否主工作树（porcelain 首个条目）。
    pub main: bool,
    pub detached: bool,
    pub bare: bool,
}

/// porcelain 的半解析条目。
struct Pending {
    path: String,
    head: Option<String>,
    branch: Option<String>,
    detached: bool,
    bare: bool,
}

/// `git worktree list --porcelain` 的输出形状：
///
/// ```text
/// worktree /path/to/main
/// HEAD abc123...
/// branch refs/heads/main
///
/// worktree /path/to/wt
/// HEAD def456...
/// detached
/// ```
fn parse_porcelain(stdout: &str) -> Result<Vec<WorktreeEntry>> {
    let mut entries: Vec<WorktreeEntry> = Vec::new();
    let mut current: Option<Pending> = None;

    for line in stdout.lines() {
        if let Some(value) = line.strip_prefix("worktree ") {
            if let Some(pending) = current.take() {
                entries.push(finish(pending, entries.is_empty())?);
            }
            current = Some(Pending {
                path: value.to_string(),
                head: None,
                branch: None,
                detached: false,
                bare: false,
            });
        } else if let Some(pending) = current.as_mut() {
            if let Some(value) = line.strip_prefix("HEAD ") {
                pending.head = Some(value.to_string());
            } else if let Some(value) = line.strip_prefix("branch ") {
                pending.branch = Some(
                    value
                        .strip_prefix("refs/heads/")
                        .unwrap_or(value)
                        .to_string(),
                );
            } else if line == "detached" {
                pending.detached = true;
            } else if line == "bare" {
                pending.bare = true;
            } else if line.starts_with("locked") || line.starts_with("prunable") {
                // 修饰行：C10 展示不需要，忽略。
            } else if line.is_empty() {
                // 条目分隔：由下一个 worktree 前缀触发 flush。
            } else {
                return Err(Error::Git(format!("porcelain 输出含未知行：{line}")));
            }
        } else if line.is_empty() {
            continue;
        } else {
            return Err(Error::Git(format!("porcelain 输出以未知行开头：{line}")));
        }
    }
    if let Some(pending) = current.take() {
        entries.push(finish(pending, entries.is_empty())?);
    }

    Ok(entries)
}

fn finish(pending: Pending, is_first: bool) -> Result<WorktreeEntry> {
    if pending.path.is_empty() {
        return Err(Error::Git("porcelain 条目缺少 worktree 路径行".to_string()));
    }
    Ok(WorktreeEntry {
        path: pending.path,
        head: pending.head,
        branch: pending.branch,
        main: is_first && !pending.bare,
        detached: pending.detached,
        bare: pending.bare,
    })
}

/// Windows 路径的「可比较」形态：统一分隔符、剥 `\\?\` verbatim 前缀、小写
/// （NTFS 大小写不敏感；Unix 上无前缀可剥，lowercase 无害——本模块只在
/// 用户显式 add 的安全红线用到，非身份键）。
fn comparable_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('/', "\\");
    let stripped = text.strip_prefix(r"\\?\").unwrap_or(&text);
    stripped.to_lowercase()
}

fn is_inside(candidate: &Path, root: &Path) -> bool {
    comparable_path(candidate).starts_with(&comparable_path(root))
}

fn git_at(repo_root: &Path, tail: Vec<OsString>, limits: Limits) -> Result<Vec<u8>> {
    let mut args: Vec<OsString> = vec!["-C".into(), repo_root.as_os_str().to_owned()];
    args.extend(tail);
    let output = runner::run(&OsString::from("git"), &args, repo_root, limits)
        .map_err(|error| Error::Git(error.to_string()))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Error::Git(format!(
            "git worktree 操作失败：{}",
            stderr.trim()
        )))
    }
}

/// 列出仓库的全部 worktree（纯读）。
pub fn list(repo_root: &Path) -> Result<Vec<WorktreeEntry>> {
    let stdout = git_at(
        repo_root,
        vec!["worktree".into(), "list".into(), "--porcelain".into()],
        Limits::default(),
    )?;
    parse_porcelain(&String::from_utf8_lossy(&stdout))
}

/// 创建 worktree（显式用户动作）。
///
/// - `create_branch = true` → `git worktree add -b <branch> <path>`；
/// - 否则 → `git worktree add <path> <branch>`（复用已有分支）。
pub fn add(
    repo_root: &Path,
    path: &Path,
    branch: &str,
    create_branch: bool,
) -> Result<WorktreeEntry> {
    if branch.trim().is_empty() {
        return Err(Error::InvalidInput("分支名不能为空".to_string()));
    }
    // 路径安全红线：目标不允许位于仓库内部。
    // 目标多半还不存在（canonicalize 会失败），所以先做无 IO 的绝对化 +
    // 字符串前缀比较（`..` 段会以 `..\` 形态打断前缀或被多拦——方向安全）；
    // 已存在的目标再叠一层 canonicalize 防 `..` 与链接绕过。
    // canonicalize 会产生 `\\?\` verbatim 前缀，is_inside 统一剥掉。
    let canonical_root = repo_root.canonicalize()?;
    let absolute_target = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if is_inside(&absolute_target, &canonical_root)
        || path
            .canonicalize()
            .is_ok_and(|target| is_inside(&target, &canonical_root))
    {
        return Err(Error::InvalidInput(format!(
            "worktree 路径不允许位于仓库内部：{}",
            path.display()
        )));
    }

    let mut tail: Vec<OsString> = vec!["worktree".into(), "add".into()];
    if create_branch {
        tail.push("-b".into());
        tail.push(branch.into());
    }
    tail.push(path.as_os_str().to_owned());
    if !create_branch {
        tail.push(branch.into());
    }
    // 检出可能耗时（大仓库），10 秒上限。
    let limits = Limits {
        timeout: std::time::Duration::from_secs(10),
        ..Default::default()
    };
    git_at(repo_root, tail, limits)?;

    // add 成功后从 list 读回（形状归一，避免解析 add 的进度输出）。
    // porcelain 路径的分隔符可能与传入不同（Windows：git 输出正斜杠），
    // 用 canonicalize 后的路径比对。
    let wanted = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    list(repo_root)?
        .into_iter()
        .find(|entry| match Path::new(&entry.path).canonicalize() {
            Ok(canonical) => canonical == wanted,
            Err(_) => entry.path == path.to_string_lossy(),
        })
        .ok_or_else(|| Error::Git(format!("worktree add 后未在列表中找到 {}", path.display())))
}

/// 一条 worktree 的未提交修改计数（ROADMAP C11）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeStatusCounts {
    pub staged_files: u64,
    pub unstaged_files: u64,
    pub untracked_files: u64,
    pub conflicted_files: u64,
}

impl WorktreeStatusCounts {
    pub fn is_clean(&self) -> bool {
        self.staged_files == 0
            && self.unstaged_files == 0
            && self.untracked_files == 0
            && self.conflicted_files == 0
    }
}

/// 某 worktree 的未提交修改计数（纯读，有界）。
///
/// `worktree_path` 必须出现在 `list(repo_root)` 里（路径白名单）。
pub fn status_counts(repo_root: &Path, worktree_path: &Path) -> Result<WorktreeStatusCounts> {
    let known = list(repo_root)?
        .into_iter()
        .find(|entry| same_worktree(Path::new(&entry.path), worktree_path));
    if known.is_none() {
        return Err(Error::InvalidInput(format!(
            "不是本仓库的 worktree：{}",
            worktree_path.display()
        )));
    }

    let stdout = git_at(
        worktree_path,
        vec![
            "-c".into(),
            "core.fsmonitor=false".into(),
            "status".into(),
            "--porcelain=v1".into(),
            "-z".into(),
            "--untracked-files=all".into(),
        ],
        Limits::default(),
    )?;
    let counts =
        super::status::parse_status(&stdout).map_err(|error| Error::Git(error.to_string()))?;
    Ok(WorktreeStatusCounts {
        staged_files: counts.staged_files,
        unstaged_files: counts.unstaged_files,
        untracked_files: counts.untracked_files,
        conflicted_files: counts.conflicted_files,
    })
}

/// 两条 worktree 路径是否指向同一目录（分隔符/verbatim 前缀不敏感）。
fn same_worktree(a: &Path, b: &Path) -> bool {
    comparable_path(a) == comparable_path(b)
}

/// 查找主工作树（porcelain 首个非 bare 条目）。
fn main_worktree(repo_root: &Path) -> Result<PathBuf> {
    list(repo_root)?
        .into_iter()
        .find(|entry| entry.main)
        .map(|entry| PathBuf::from(entry.path))
        .ok_or_else(|| Error::Git("未找到主工作树".to_string()))
}

/// 删除一条**干净**的链接 worktree（显式用户动作）。
///
/// 安全边界：主树与 bare 不可删（它们不是「用完可清理」的工作副本）；
/// 未提交修改 → 明确报错并列出计数，绝不静默丢弃。
pub fn remove(repo_root: &Path, worktree_path: &Path) -> Result<()> {
    let entries = list(repo_root)?;
    let canonical_root = repo_root.canonicalize()?;
    let entry = entries
        .iter()
        .find(|entry| same_worktree(Path::new(&entry.path), worktree_path))
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "不是本仓库的 worktree：{}",
                worktree_path.display()
            ))
        })?;
    if entry.bare || same_worktree(Path::new(&entry.path), &canonical_root) {
        return Err(Error::InvalidInput(
            "主工作树 / bare 仓库不能通过 worktree 删除".to_string(),
        ));
    }

    let counts = status_counts(repo_root, worktree_path)?;
    if !counts.is_clean() {
        return Err(Error::Git(format!(
            "worktree 有未提交修改，拒绝删除（暂存 {} / 未暂存 {} / 未跟踪 {} / 冲突 {}）——请先处理或手动清理",
            counts.staged_files,
            counts.unstaged_files,
            counts.untracked_files,
            counts.conflicted_files
        )));
    }

    git_at(
        repo_root,
        vec![
            "worktree".into(),
            "remove".into(),
            worktree_path.as_os_str().to_owned(),
        ],
        Limits::default(),
    )?;
    Ok(())
}

/// 快进合并结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeOutcome {
    /// 合并前主树 HEAD（回溯用）。
    pub previous_head: Option<String>,
    /// 合并后主树 HEAD。
    pub new_head: Option<String>,
}

/// 把 `branch` 快进合并到主工作树（显式用户动作，保守版）。
///
/// - 主树有未提交修改 → 拒绝（合并会污染工作区）；
/// - `--ff-only`：非快进（历史分叉）→ 明确报错，绝不做 rebase/force；
/// - 分支必须来自 `list`（路径/引用白名单语义）。
pub fn merge_ff_only(repo_root: &Path, branch: &str) -> Result<MergeOutcome> {
    let main_path = main_worktree(repo_root)?;
    // 主树必须干净（有界只读检查）。
    let main_counts = status_counts(repo_root, &main_path)?;
    if !main_counts.is_clean() {
        return Err(Error::Git(format!(
            "主工作树有未提交修改，拒绝合并（暂存 {} / 未暂存 {} / 未跟踪 {}）",
            main_counts.staged_files, main_counts.unstaged_files, main_counts.untracked_files
        )));
    }
    // 分支必须真实存在且未被其它 worktree 检出（git 自身会拒绝，这里给出可读错误）。
    let known_branch = list(repo_root)?
        .iter()
        .any(|entry| entry.branch.as_deref() == Some(branch));
    if !known_branch {
        return Err(Error::InvalidInput(format!(
            "分支 {branch} 不是本仓库任何 worktree 的检出分支"
        )));
    }

    let previous_head = list(repo_root)?
        .into_iter()
        .find(|entry| entry.main)
        .and_then(|entry| entry.head);

    git_at(
        repo_root,
        vec!["merge".into(), "--ff-only".into(), branch.into()],
        Limits::default(),
    )?;

    let new_head = list(repo_root)?
        .into_iter()
        .find(|entry| entry.main)
        .and_then(|entry| entry.head);

    Ok(MergeOutcome {
        previous_head,
        new_head,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::test_support::Repo;
    use std::path::PathBuf;

    #[test]
    fn lists_main_detached_and_linked_worktrees() {
        let repo = Repo::new();
        std::fs::write(repo.0.join("file"), b"data").unwrap();
        repo.git(&["add", "file"]);
        repo.git(&["commit", "-qm", "initial"]);

        let main_path = repo.0.canonicalize().unwrap();
        let wt_dir = repo
            .0
            .parent()
            .unwrap()
            .join(format!("hh-wt-{}", uuid::Uuid::new_v4()));
        // 显式创建：这正是被测函数——先手动跑一次保证 list 有 linked 条目。
        let entry = add(&repo.0, &wt_dir, "feature-x", true).expect("add");
        assert_eq!(entry.branch.as_deref(), Some("feature-x"));
        assert!(!entry.main);
        assert!(!entry.detached);
        assert_eq!(Path::new(&entry.path), wt_dir.as_path());

        let entries = list(&repo.0).expect("list");
        assert_eq!(entries.len(), 2, "主树 + 链接树：{entries:?}");
        assert!(entries[0].main, "首个条目是主树");
        // porcelain 路径分隔符/前缀与 canonicalize 不同，比对规范化后的路径。
        assert_eq!(
            Path::new(&entries[0].path)
                .canonicalize()
                .expect("主树路径"),
            main_path
        );

        // detached 链接树：checkout --detach 后重新列出。
        let output = std::process::Command::new("git")
            .args(["-C", wt_dir.to_str().unwrap(), "checkout", "--detach", "-q"])
            .status()
            .unwrap();
        assert!(output.success());
        let entries = list(&repo.0).expect("list");
        let wt_canonical = wt_dir.canonicalize().expect("链接树路径");
        let detached = entries
            .iter()
            .find(|entry| {
                Path::new(&entry.path)
                    .canonicalize()
                    .is_ok_and(|canonical| canonical == wt_canonical)
            })
            .expect("链接树");
        assert!(detached.detached);
        assert_eq!(detached.branch, None);
        assert!(detached.head.as_deref().is_some_and(|h| h.len() == 40));

        let _ = std::fs::remove_dir_all(&wt_dir);
        repo.git(&["worktree", "prune"]);
    }

    #[test]
    fn add_rejects_nested_paths_and_duplicate_branches() {
        let repo = Repo::new();
        std::fs::write(repo.0.join("file"), b"data").unwrap();
        repo.git(&["add", "file"]);
        repo.git(&["commit", "-qm", "initial"]);

        // 仓库内部路径：路径安全红线。
        let nested = repo.0.join("nested-wt");
        let error = add(&repo.0, &nested, "feature-y", true).expect_err("嵌套路径必须拒绝");
        assert!(
            error.to_string().contains("不允许位于仓库内部"),
            "实际：{error}"
        );

        // 空分支名。
        assert!(add(&repo.0, &repo.0.join("../x"), "  ", true).is_err());

        // 已存在的分支：create_branch=false 复用（git 语义：该分支不能正被
        // 其它 worktree 检出，所以先建一个未检出的分支）；=true 由 git 报冲突。
        repo.git(&["branch", "reuse-me"]);
        let reuse = repo
            .0
            .parent()
            .unwrap()
            .join(format!("hh-wt-{}", uuid::Uuid::new_v4()));
        let reused = add(&repo.0, &reuse, "reuse-me", false).expect("复用分支");
        assert_eq!(reused.branch.as_deref(), Some("reuse-me"));

        let conflict = repo
            .0
            .parent()
            .unwrap()
            .join(format!("hh-wt-{}", uuid::Uuid::new_v4()));
        assert!(add(&repo.0, &conflict, "reuse-me", true).is_err());

        let _ = std::fs::remove_dir_all(&reuse);
        repo.git(&["worktree", "prune"]);
    }

    #[test]
    fn malformed_porcelain_is_a_clear_error_not_a_guess() {
        assert!(parse_porcelain("").is_ok(), "空输出 = 没有任何 worktree");
        assert!(parse_porcelain("HEAD abc\n").is_err(), "缺 worktree 前缀");
        assert!(parse_porcelain("worktree /x\nunknown-line\n").is_err());
        // locked/prunable 修饰行忽略。
        let entries =
            parse_porcelain("worktree /main\nHEAD a\nbranch refs/heads/main\nlocked reason\n")
                .expect("locked 行忽略");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].branch.as_deref(), Some("main"));
        // bare 主树不算 main 工作树。
        let bare = parse_porcelain("worktree /bare.git\nbare\n").expect("bare");
        assert!(bare[0].bare);
        assert!(!bare[0].main);
    }

    /// C11：干净 worktree 可删；脏拒绝并列出计数；主树拒绝。
    #[test]
    fn remove_requires_a_clean_linked_worktree() {
        let repo = Repo::new();
        std::fs::write(repo.0.join("file"), b"data").unwrap();
        repo.git(&["add", "file"]);
        repo.git(&["commit", "-qm", "initial"]);

        let wt = repo
            .0
            .parent()
            .unwrap()
            .join(format!("hh-wt-{}", uuid::Uuid::new_v4()));
        add(&repo.0, &wt, "feature", true).expect("创建");

        // 主树不可删。
        assert!(remove(&repo.0, &repo.0).is_err());

        // 弄脏 → 拒绝并列出计数。
        std::fs::write(wt.join("dirty"), b"dirty").unwrap();
        let error = remove(&repo.0, &wt).expect_err("脏树必须拒绝");
        assert!(
            error.to_string().contains("未提交修改") && error.to_string().contains("未跟踪 1"),
            "错误要列出计数：{error}"
        );

        // 清理干净 → 可删。
        std::fs::remove_file(wt.join("dirty")).unwrap();
        remove(&repo.0, &wt).expect("干净树可删");
        // 删除后目录已不存在，不能 canonicalize：用 comparable 路径比对。
        assert!(
            !list(&repo.0)
                .unwrap()
                .iter()
                .any(|entry| comparable_path(Path::new(&entry.path)) == comparable_path(&wt)),
            "删除后列表不得再包含该 worktree"
        );
    }

    /// C11：ff-only 合并——成功/主树脏拒绝/非 ff 拒绝/未知分支拒绝。
    #[test]
    fn merge_ff_only_is_conservative() {
        let repo = Repo::new();
        std::fs::write(repo.0.join("file"), b"base").unwrap();
        repo.git(&["add", "file"]);
        repo.git(&["commit", "-qm", "base"]);

        let wt = repo
            .0
            .parent()
            .unwrap()
            .join(format!("hh-wt-{}", uuid::Uuid::new_v4()));
        add(&repo.0, &wt, "feature", true).expect("创建");
        std::fs::write(wt.join("feature-file"), b"feature").unwrap();
        repo.git_at_worktree(&wt, &["add", "feature-file"]);
        repo.git_at_worktree(&wt, &["commit", "-qm", "feature"]);

        // 未知分支拒绝。
        assert!(merge_ff_only(&repo.0, "no-such-branch").is_err());

        // ff 合并成功：主树 HEAD 前进到 feature HEAD。
        let outcome = merge_ff_only(&repo.0, "feature").expect("ff 合并");
        let feature_head = list(&repo.0)
            .unwrap()
            .iter()
            .find(|entry| entry.branch.as_deref() == Some("feature"))
            .and_then(|entry| entry.head.clone())
            .expect("feature HEAD");
        assert_eq!(outcome.new_head, Some(feature_head));
        assert!(outcome.previous_head.is_some());

        // 主树脏 → 拒绝。
        std::fs::write(repo.0.join("main-dirty"), b"x").unwrap();
        std::fs::write(wt.join("another"), b"y").unwrap();
        repo.git_at_worktree(&wt, &["add", "another"]);
        repo.git_at_worktree(&wt, &["commit", "-qm", "another"]);
        let error = merge_ff_only(&repo.0, "feature").expect_err("主树脏必须拒绝");
        assert!(
            error.to_string().contains("主工作树有未提交修改"),
            "{error}"
        );
        std::fs::remove_file(repo.0.join("main-dirty")).unwrap();

        // 非 ff（分叉）→ 拒绝：主树上做一个 feature 没有的提交。
        repo.git(&["commit", "--allow-empty", "-qm", "main-only"]);
        let error = merge_ff_only(&repo.0, "feature").expect_err("非 ff 必须拒绝");
        assert!(
            error.to_string().contains("失败") || error.to_string().contains("merge"),
            "错误来自 git 层：{error}"
        );
    }

    /// 本机真实仓库的只读 smoke（不写任何东西）。
    #[test]
    fn real_repository_listing_matches_git_worktree_list() {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let Ok(entries) = list(&repo_root) else {
            eprintln!("SKIP：当前环境不是 git 仓库或 git 不可用");
            return;
        };
        eprintln!(
            "本机 {} 个 worktree：{:?}",
            entries.len(),
            entries.iter().map(|e| &e.path).collect::<Vec<_>>()
        );
        assert!(!entries.is_empty());
        assert!(entries[0].main);
        assert!(entries.iter().all(|e| !e.path.is_empty()));
    }
}
