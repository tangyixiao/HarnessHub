//! Git / Worktree 观察层。
//!
//! 职责：只读获取工作树状态；后续任务将扩展快照采集。
//! `git_events` 只描述 Session 时间窗口内发生的代码变化，不能据此断言代码由 AI 编写。

mod status;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StatusCounts {
    pub(crate) staged_files: u64,
    pub(crate) unstaged_files: u64,
    pub(crate) untracked_files: u64,
    pub(crate) conflicted_files: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GitCaptureError {
    MalformedOutput,
}

impl std::fmt::Display for GitCaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MalformedOutput => f.write_str("malformed Git status output"),
        }
    }
}

impl std::error::Error for GitCaptureError {}
