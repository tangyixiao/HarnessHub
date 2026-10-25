//! 只读会话回放（ROADMAP B6）。
//!
//! 解析外部 Harness 的会话转录并渲染时间线：**只读、不落库、不写任何外部数据**
//! （ADR-0003 local-first；不写 turns/messages 表——那是 G24 Canonical Event
//! 平面的决策点）。
//!
//! 上游格式漂移的纪律：未知行类型 / 未知内容种类一律计入 skipped / unsupported
//! 计数，绝不 panic、绝不编造（fixture 回归见 fixtures/claude/replay/）。

pub mod claude;
pub mod index;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 回放消息的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayRole {
    User,
    Assistant,
}

/// 一条可回放的消息（user / assistant）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayMessage {
    pub seq: u64,
    pub role: ReplayRole,
    /// sidechain（子代理链）里的消息；B8 会用它构建子代理树。
    pub sidechain: bool,
    pub timestamp: Option<String>,
    /// 正文 text 部分，按出现顺序。
    pub text_parts: Vec<String>,
    /// thinking 部分（渲染为标注/折叠，不与正文混排）。
    pub thinking_parts: Vec<String>,
    pub model: Option<String>,
    /// 认识但 B6 不展开的内容种类（如 tool_use）→ 出现次数。
    pub unsupported_content_kinds: BTreeMap<String, u64>,
}

/// 一次完整的只读回放。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaySession {
    pub harness_id: String,
    pub session_id: String,
    pub source_path: String,
    pub messages: Vec<ReplayMessage>,
    /// 被跳过的行类型（permission-mode / file-history-snapshot / …）→ 次数。
    pub skipped_kind_counts: BTreeMap<String, u64>,
    /// 无法解析为 JSON 的行数。
    pub malformed_lines: u64,
}

/// 列表项摘要（发现列表，不含正文）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaySessionSummary {
    pub harness_id: String,
    pub session_id: String,
    /// 转录所在的项目目录名（Claude 用编码后的路径名，如 `C--Users-tangy`）。
    pub project_dir: String,
    pub size_bytes: u64,
    /// 修改时间（Unix 毫秒）。列表排序键。
    pub modified_ms: i64,
    /// 首条 user 消息预览（≤ 120 字符）；无则 None。
    pub first_user_preview: Option<String>,
    /// 预读窗口内的首/末时间戳（文件超过预读上限时 last 是下界）。
    pub started_at: Option<String>,
    pub last_at: Option<String>,
}
