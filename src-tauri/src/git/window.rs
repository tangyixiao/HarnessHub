//! 会话窗口 Git 摘要（ROADMAP A3）：由 start/end 两个 HEAD 推导 commit 数与 ±行数。
//!
//! 摘要**不落库**（见 spec）：它由已落库的 head + 仓库当前状态推导，
//! 落库反而要处理「仓库被 rebase 后摘要失真」的失效问题。
//!
//! R5 纪律：这里只是「会话窗口内仓库发生了什么」，不是 AI 产出归因。

use std::ffi::OsString;
use std::path::Path;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use super::store::{GitSnapshotStore, SnapshotPhase};
use super::{runner, GitCaptureError};
use crate::error::Result;

/// 会话窗口内的 Git 变化量。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowSummary {
    pub commits: u64,
    pub insertions: u64,
    pub deletions: u64,
}

fn git_at(root: &Path, tail: &[&str]) -> std::result::Result<Vec<u8>, GitCaptureError> {
    let mut args = vec![OsString::from("-C"), root.as_os_str().to_owned()];
    args.extend(tail.iter().map(OsString::from));
    let output = runner::run(&OsString::from("git"), &args, root, runner::Limits::default())?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(GitCaptureError::CommandFailed)
    }
}

/// `start..end` 窗口摘要。
///
/// - 两端同 HEAD → 直接返回零摘要，**不起任何进程**；
/// - git 失败（目录被移动/删除、超时）或输出形态不认识 → `None`：
///   摘要只是观测增强，宁可少显示也不猜。
pub fn window_summary(root: &Path, start_head: &str, end_head: &str) -> Option<WindowSummary> {
    if start_head == end_head {
        return Some(WindowSummary::default());
    }
    let range = format!("{start_head}..{end_head}");

    let log = git_at(root, &["rev-list", "--count", &range]).ok()?;
    let commits = std::str::from_utf8(&log).ok()?.trim().parse::<u64>().ok()?;

    let stat = git_at(root, &["diff", "--shortstat", &range]).ok()?;
    let stat = String::from_utf8(stat).ok()?;
    let (insertions, deletions) = parse_shortstat(&stat)?;

    Some(WindowSummary {
        commits,
        insertions,
        deletions,
    })
}

/// 解析 `git diff --shortstat` 输出，例如
/// ` 3 files changed, 10 insertions(+), 2 deletions(-)`。
///
/// 二进制变更可能没有 insertions/deletions 段（` 2 files changed`）——合法，按 0 处理。
/// 连「N file(s) changed」都没有的输出（错误文本、usage）一律 `None`。
fn parse_shortstat(text: &str) -> Option<(u64, u64)> {
    let text = text.trim();
    if text.is_empty() {
        return Some((0, 0));
    }
    if !(text.contains("files changed") || text.contains("file changed")) {
        return None;
    }

    let count = |label: &str| -> Option<u64> {
        let index = text.find(label)?;
        text[..index]
            .trim_end()
            .rsplit(',')
            .next()?
            .trim()
            .parse()
            .ok()
    };
    let insertions = count("insertions(+)");
    let deletions = count("deletions(-)");

    match (insertions, deletions) {
        (Some(i), Some(d)) => Some((i, d)),
        (Some(i), None) => Some((i, 0)),
        (None, Some(d)) => Some((0, d)),
        // 形态合法但没有 ± 行数（纯二进制/纯模式变更）。
        (None, None) => Some((0, 0)),
    }
}

/// 计算窗口摘要所需的输入：两端 HEAD 与快照时刻的工作树根。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInputs {
    pub worktree_root: std::path::PathBuf,
    pub start_head: String,
    pub end_head: String,
}

/// 读取会话的窗口输入：start 与 end 快照都有 HEAD 才有窗口。
///
/// 只碰数据库、不起进程 —— IPC 层在**数据库锁内**调用本函数后丢锁，
/// 再调用 [`window_summary`]（会跑 git 子进程），见 commands.rs 的锁纪律。
pub fn window_inputs(conn: &Connection, hub_session_id: &str) -> Result<Option<WindowInputs>> {
    let snapshots = GitSnapshotStore::new(conn).list_by_session(hub_session_id)?;
    let start = snapshots.iter().find(|item| item.phase == SnapshotPhase::Start);
    let end = snapshots.iter().find(|item| item.phase == SnapshotPhase::End);

    let (Some(start), Some(end)) = (start, end) else {
        return Ok(None);
    };
    let (Some(start_head), Some(end_head)) = (&start.head, &end.head) else {
        // unborn HEAD（空仓库）拍不出窗口。
        return Ok(None);
    };

    // worktree_root 是快照时刻的路径；目录被移动/删除时 git 失败 → None。
    Ok(Some(WindowInputs {
        worktree_root: std::path::PathBuf::from(&end.worktree_root),
        start_head: start_head.clone(),
        end_head: end_head.clone(),
    }))
}

/// 便捷路径：读输入并计算（**会执行 git 子进程**，调用方不得持数据库锁）。
pub fn session_summary(
    conn: &Connection,
    hub_session_id: &str,
) -> Result<Option<WindowSummary>> {
    let Some(inputs) = window_inputs(conn, hub_session_id)? else {
        return Ok(None);
    };
    Ok(window_summary(
        &inputs.worktree_root,
        &inputs.start_head,
        &inputs.end_head,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::test_support::Repo;

    fn head_of(root: &Path) -> String {
        match super::super::capture(root).expect("capture") {
            super::super::GitObservation::Repository(snapshot) => {
                snapshot.head.expect("已提交仓库必有 HEAD")
            }
            super::super::GitObservation::NotRepository => panic!("临时仓库必须是仓库"),
        }
    }

    #[test]
    fn equal_heads_return_a_zero_summary_without_spawning_git() {
        // 路径故意不存在：同 HEAD 分支必须不碰文件系统也返回零摘要。
        let summary = window_summary(Path::new("Z:/definitely/not/a/repo"), "abc", "abc");
        assert_eq!(summary, Some(WindowSummary::default()));
    }

    #[test]
    fn a_two_commit_window_counts_commits_and_lines() {
        let repo = Repo::new();
        std::fs::write(repo.0.join("file"), b"line\n").unwrap();
        repo.git(&["add", "file"]);
        repo.git(&["commit", "-qm", "base"]);

        let start = head_of(&repo.0);

        std::fs::write(repo.0.join("file"), b"line\nline2\n").unwrap();
        repo.git(&["add", "file"]);
        repo.git(&["commit", "-qm", "second"]);
        std::fs::write(repo.0.join("other"), b"a\nb\nc\n").unwrap();
        repo.git(&["add", "other"]);
        repo.git(&["commit", "-qm", "third"]);

        let end = head_of(&repo.0);

        let summary = window_summary(&repo.0, &start, &end).expect("窗口摘要");
        assert_eq!(summary.commits, 2);
        // second: +1；third: +3 → insertions 至少 4；没有删除。
        assert!(summary.insertions >= 4, "实际：{summary:?}");
        assert_eq!(summary.deletions, 0);
    }

    #[test]
    fn a_missing_repository_yields_none_instead_of_an_error() {
        let summary = window_summary(
            Path::new("Z:/definitely/not/a/repo"),
            &"a".repeat(40),
            &"b".repeat(40),
        );
        assert_eq!(summary, None, "目录不存在必须安静地没有摘要");
    }

    #[test]
    fn shortstat_shapes_parse_exactly() {
        assert_eq!(
            parse_shortstat(" 3 files changed, 10 insertions(+), 2 deletions(-)\n"),
            Some((10, 2))
        );
        assert_eq!(
            parse_shortstat(" 1 file changed, 5 insertions(+)"),
            Some((5, 0))
        );
        assert_eq!(
            parse_shortstat(" 1 file changed, 7 deletions(-)"),
            Some((0, 7))
        );
        // 纯二进制/模式变更：形态合法，按 0 处理。
        assert_eq!(parse_shortstat(" 2 files changed"), Some((0, 0)));
        assert_eq!(parse_shortstat(""), Some((0, 0)));
        // 错误文本 / usage 绝不能被当成摘要。
        assert_eq!(parse_shortstat("fatal: bad object abc"), None);
        assert_eq!(parse_shortstat("usage: git diff [options]"), None);
    }
}
