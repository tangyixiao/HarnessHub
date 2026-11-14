//! MCP Binding Registry（ROADMAP E18）：**配置层**决定哪个 MCP Server 写进
//! 哪个 Harness 的原生配置（规划 §4.20——MCP Runtime 连接是 F22，与本层无关）。
//!
//! 数据流：Registry（SQLite，source-of-truth）→ diff → preview → atomic sync
//! （复用 E15 的 backup/temp/verify/rename 管线，见 `profile::projection`）。
//!
//! 安全红线：
//! - `env_keys` 只是**变量名元数据**，任何值不进本层；
//! - 同步只覆盖 Hub 管理的键（command/args/url/type），条目内用户手写的
//!   `env` / `headers` 等一律保留（绝不把 `<redacted>` 写进真配置）。

pub mod policy;
pub mod registry;
pub mod sync;

use serde::{Deserialize, Serialize};

/// 一个登记在 Hub 的 MCP Server。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServer {
    pub id: String,
    /// 写进 harness 配置时使用的键（限 `[A-Za-z0-9._-]`）。
    pub name: String,
    pub transport: McpTransport,
    /// stdio：可执行文件。
    pub command: Option<String>,
    /// stdio：参数（DB 侧以 JSON 数组落盘）。
    pub args: Vec<String>,
    /// http：endpoint。
    pub url: Option<String>,
    /// 该 server 需要的环境变量**名**（展示 / Doctor 提示用；值不进 Hub）。
    pub env_keys: Vec<String>,
    pub enabled: bool,
    /// `hub`（用户登记）或 `imported:<harness>`（从 harness 配置导入）。
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    Stdio,
    Http,
}

/// 一条绑定：某 server 在哪个 scope 生效（global 或 harness）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpBinding {
    pub id: String,
    pub server_id: String,
    /// 冗余展示名（JOIN 取出，避免前端再查一遍）。
    pub server_name: String,
    /// `global` | `harness`。
    pub scope: String,
    pub harness_id: Option<String>,
    pub created_at: String,
}
