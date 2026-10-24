//! Git / Worktree 观察层：只读获取有界工作树快照。
mod runner;
mod snapshot;
mod status;

pub mod service;
pub mod store;

#[cfg(test)]
pub(crate) mod test_support;

pub use snapshot::{capture, GitObservation, GitSnapshot};
pub use store::{GitSnapshotStore, SessionGitSnapshot, SnapshotPhase};
use std::fmt;

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct StatusCounts {
    pub(crate) staged_files: u64,
    pub(crate) unstaged_files: u64,
    pub(crate) untracked_files: u64,
    pub(crate) conflicted_files: u64,
}
#[derive(Clone, PartialEq, Eq)]
pub enum GitCaptureError {
    InvalidPath,
    CommandNotFound,
    LaunchFailed,
    CommandFailed,
    Timeout,
    OutputLimit,
    ReadFailed,
    MalformedOutput,
}
impl fmt::Display for GitCaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidPath => "invalid Git capture path",
            Self::CommandNotFound => "Git command unavailable",
            Self::LaunchFailed => "Git command could not be launched",
            Self::CommandFailed => "Git command failed",
            Self::Timeout => "Git command timed out",
            Self::OutputLimit => "Git command output exceeded limit",
            Self::ReadFailed => "Git command output could not be read",
            Self::MalformedOutput => "malformed Git status output",
        })
    }
}
impl fmt::Debug for GitCaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GitCaptureError(")?;
        fmt::Display::fmt(self, f)?;
        f.write_str(")")
    }
}
impl std::error::Error for GitCaptureError {}
