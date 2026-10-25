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
use std::path::Path;

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
