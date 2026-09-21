//! 进程生命周期管理。
//!
//! 职责：启动 / 停止 / 查询外部 Harness 进程，把 PID 与退出码回写到 Session。
//!
//! 当前为占位模块（Phase 1 Task「PTY launch」）：与 `crate::pty` 一起实现，
//! 在真正接入前不提供公开 API。
