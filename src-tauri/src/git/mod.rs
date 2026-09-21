//! Git / Worktree 观察层。
//!
//! 职责：只读获取 Session 起止时的 HEAD、diff stat、commit 数与 worktree 信息，
//! 写入 `git_events`。
//!
//! 重要约束：`git_events` 只描述「Session 时间窗口内发生了什么代码变化」，
//! **不能**用来断言代码由 AI 编写（docs/CONTEXT.md 核心约束 7）。
//!
//! 当前为占位模块（Phase 1 Task「Git observer」）：这里刻意不提供未经验证的公开 API，
//! 避免下游误以为已经可用。
