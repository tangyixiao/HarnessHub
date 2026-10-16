//! Activity timeline：usage + session 的统一活动事件流（Projection）。
//!
//! 职责（§20 第 15 步，spec：`docs/specs/2026-10-14-activity-timeline-design.md`）：
//! 把 `usage_events`（明细）与 `sessions`（Hub 自己管理的会话）折叠成一条按时间排列的
//! 活动流，并提供按本地日 / 小时的聚合，供 Activity Calendar 与 Session Timeline 渲染。
//!
//! 依赖方向：`activity → db/error/clock/usage`；`commands → activity`。
//!
//! 边界：
//! - **只读**。本模块不写任何表，也不起进程（ADR-0012）。
//! - **不碰 git_events / file_events**：这两张表至今没有 Rust 写入方
//!   （`git/mod.rs`、`watcher/mod.rs` 仍是占位模块），Git 快照属于第 16 步。
//! - **不推断归属**：usage 事件的 `project_id` 只通过 `hub_session_id` 反查
//!   `sessions.project_id`；反查不到就留 NULL，不按 `source_session_id` 猜
//!   （`usage/importer.rs` 的同一原则）。

pub mod query;

use serde::{Deserialize, Serialize};

use crate::usage::summary::UsageRangeWindow;

/// 事件来源种类。`usage` 与 `session` 永不合并（ADR-0012 决策三）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityEventKind {
    /// 来自 `usage_events` 的一条用量明细。
    Usage,
    /// 来自 `sessions` 的一条 Hub 管理会话。
    Session,
}

impl ActivityEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::Session => "session",
        }
    }

    /// 只给测试用：把库里的字符串变回枚举（query.rs 直接产出枚举，不需要它）。
    #[cfg(test)]
    #[allow(dead_code)] // 契约测试只用它证明「未知值不会 panic」，不参与生产路径
    fn from_db(value: &str) -> Self {
        match value {
            "session" => Self::Session,
            _ => Self::Usage,
        }
    }
}

/// 活动流里的一条事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEvent {
    pub kind: ActivityEventKind,
    /// UTC RFC3339（与库里一致，不做本地化 —— 本地化由前端按 timezoneOffsetMinutes 决定）。
    pub occurred_at: String,
    /// **本地**日（`YYYY-MM-DD`，按 `timezone_offset_minutes` 换算）。
    pub day: String,
    /// **本地**小时（0..=23）。
    pub hour: u8,
    pub harness_id: String,
    /// 可空：反查不到项目归属时留空，不猜。
    pub project_id: Option<String>,
    /// usage 事件的外部会话 id；session 事件为 NULL（它的 id 就是 `hub_session_id`）。
    pub source_session_id: Option<String>,
    /// 仅 usage：模型 id。
    pub model: Option<String>,
    /// usage 事件的 token 合计；session 事件恒为 0。
    pub total_tokens: u64,
    /// 仅 usage。
    pub cost_microunits: Option<i64>,
    /// 仅 usage：由后端判定（价格缺失 / 估算价时为 true）。
    pub cost_is_lower_bound: bool,
    /// 仅 session：`running` / `exited` / `failed` / `unknown`。
    pub status: Option<String>,
    /// 仅 session：`terminal` / `resume` / `imported`。
    pub launch_mode: Option<String>,
    /// 仅 session：`ended_at - started_at`；未结束的会话为 NULL。
    pub duration_ms: Option<i64>,
    /// 仅 session。
    pub cwd: Option<String>,
    /// Hub 自己的会话 id（`sessions.hub_session_id`）；usage 事件可能为 NULL。
    pub hub_session_id: Option<String>,
}

/// 按**本地日**聚合的一格（Activity Calendar 的一格）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityDayBucket {
    pub day: String,
    pub usage_event_count: u64,
    pub managed_session_count: u64,
    pub total_tokens: u64,
    pub cost_microunits: Option<i64>,
    pub cost_is_lower_bound: bool,
}

/// 按**本地小时**聚合的一格（24 格，恒有 24 项）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityHourBucket {
    pub hour: u8,
    pub usage_event_count: u64,
    pub managed_session_count: u64,
    pub total_tokens: u64,
}

/// 跨 IPC 的稳定 DTO。前端只负责格式化，不做任何判定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityTimeline {
    /// 与 Dashboard 同一个窗口语义（复用，不重复定义）。
    pub range: UsageRangeWindow,
    /// 倒序（最新在前），长度受 limit 约束。
    pub events: Vec<ActivityEvent>,
    /// 按本地日升序。
    pub days: Vec<ActivityDayBucket>,
    /// 固定 24 项，按 hour 升序（没有事件的小时是 0）。
    pub hours: Vec<ActivityHourBucket>,
    pub usage_event_count: u64,
    pub managed_session_count: u64,
    pub active_days: u64,
    pub total_tokens: u64,
    /// 前端画格子只需要一个最大值，不需要再扫一遍。
    pub max_day_tokens: u64,
    pub max_hour_tokens: u64,
    /// `events` 被 limit 截断时为 true —— 前端不得把列表当成「全部」。
    pub has_more: bool,
}

impl ActivityTimeline {
    /// 跨 IPC 契约测试用：锁顶层 12 个 camelCase 键。
    pub fn keys() -> &'static [&'static str] {
        &[
            "range",
            "events",
            "days",
            "hours",
            "usageEventCount",
            "managedSessionCount",
            "activeDays",
            "totalTokens",
            "maxDayTokens",
            "maxHourTokens",
            "hasMore",
        ]
    }
}

impl ActivityEvent {
    /// 跨 IPC 契约测试用：锁 17 个 camelCase 键。
    pub fn keys() -> &'static [&'static str] {
        &[
            "kind",
            "occurredAt",
            "day",
            "hour",
            "harnessId",
            "projectId",
            "sourceSessionId",
            "model",
            "totalTokens",
            "costMicrounits",
            "costIsLowerBound",
            "status",
            "launchMode",
            "durationMs",
            "cwd",
            "hubSessionId",
        ]
    }
}

impl ActivityDayBucket {
    /// 跨 IPC 契约测试用。
    pub fn keys() -> &'static [&'static str] {
        &[
            "day",
            "usageEventCount",
            "managedSessionCount",
            "totalTokens",
            "costMicrounits",
            "costIsLowerBound",
        ]
    }
}

impl ActivityHourBucket {
    /// 跨 IPC 契约测试用。
    pub fn keys() -> &'static [&'static str] {
        &[
            "hour",
            "usageEventCount",
            "managedSessionCount",
            "totalTokens",
        ]
    }
}

#[cfg(test)]
mod tests;
