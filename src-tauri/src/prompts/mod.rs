//! Prompts Registry（ROADMAP E19）：canonical markdown 的统一登记与跨 harness
//! 记忆文件投影（CLAUDE.md / AGENTS.md / GEMINI.md，规划 §4.21）。
//!
//! 渲染纪律（spec 2026-10-31-skills-prompts-tray-design.md）：
//! - canonical 内容永远在标记块内（`<!-- harness-hub:begin -->` … end），
//!   块外用户内容**逐字节保留**；重复渲染幂等；
//! - 写盘走 E15 管线的文本通用变体（`profile::projection::apply_text`）。

pub mod registry;
pub mod render;

use serde::{Deserialize, Serialize};

/// 一条登记的 prompt（canonical markdown）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptArtifact {
    pub id: String,
    pub name: String,
    /// canonical markdown 原文。
    pub content: String,
    /// 绑定项目；None = 全局（进每个项目的渲染）。
    pub project_id: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}
