//! Provider / Profile 配置层（ROADMAP E14）：**只读发现**各 Harness 的当前
//! 配置形态，规范化为 `ProfileSummary` 跨 IPC 展示。
//!
//! 安全红线（spec）：任何 key / token / secret 值不得跨 IPC——
//! redact 在 DTO 组装前执行，契约测试锁定。**不写任何外部文件**（投影是 E15）。

pub mod claude;
pub mod codex;
pub mod doctor;
pub mod gemini;
pub mod projection;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 一个 Harness 的当前配置形态（只读快照）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub harness_id: String,
    /// 配置文件绝对路径（展示用；文件不存在时为候选路径）。
    pub source_path: String,
    /// 规范化键值（已 redact）；键用「点路径」表示嵌套（如 `env.ANTHROPIC_MODEL`）。
    pub fields: BTreeMap<String, String>,
    pub parsed_at: String,
}

/// 值脱敏：匹配 secret 模式的键的值替换为 `<redacted>`。
/// 匹配键名（不匹配值）——`ANTHROPIC_AUTH_TOKEN` / `*_API_KEY` 等。
/// 例外：`env_key` 是**引用名**（指向环境变量名，不是 secret 本身），保留展示。
pub(crate) fn redact_if_secret(key: &str, value: &str) -> String {
    let upper = key.to_ascii_uppercase();
    if upper == "ENV_KEY" {
        // 引用名不是 secret：用户需要知道指向了哪个环境变量。
        return value.to_string();
    }
    if upper.contains("KEY")
        || upper.contains("TOKEN")
        || upper.contains("SECRET")
        || upper.contains("PASSWORD")
        || upper.contains("CREDENTIAL")
    {
        "<redacted>".to_string()
    } else {
        value.to_string()
    }
}

/// gemini 的 API key 存在性：只报 set/unset，绝不带值。
pub(crate) fn presence(value: Option<&str>) -> &'static str {
    match value {
        Some(text) if !text.trim().is_empty() => "set",
        _ => "unset",
    }
}
