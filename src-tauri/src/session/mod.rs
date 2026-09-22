//! Session 生命周期与索引。
//!
//! 负责：统一 Session 标识、状态流转、按项目 / Harness / 时间检索。
//! 进程与 PTY 细节在 `crate::process` 与 `crate::pty`。

pub mod service;
pub mod store;

pub use service::SessionService;
pub use store::{LaunchMode, NewSession, SessionRecord, SessionStatus, SessionStore};
