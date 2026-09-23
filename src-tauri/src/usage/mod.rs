//! Usage 归一化与导入：把外部来源的 Token / Cost 变成统一结构。
//!
//! 契约见 `docs/adr/0011-usage-import-contracts.md`。本模块的分工：
//!
//! * [`cost`]：金额边界（十进制原文 → 整数微单位），纯函数；
//! * [`key`]：`stable_source_key`（versioned canonical hash），纯函数；
//! * [`ccusage`]：形状识别 + 归一化，纯函数（**不碰数据库、不起进程**）；
//! * `adapter` / `runner` / `importer`：发现、调用、落库（IO 层）。
//!
//! 唯一性由数据库的 `usage_events.stable_source_key UNIQUE` 兜底，
//! 应用层不做「先查再插」的判断（ADR-0004 第 4 条的思路）。

pub mod ccusage;
pub mod cost;
pub mod key;

use serde::{Deserialize, Serialize};

/// 数据源当前是否可用于导入。
///
/// 「没装 ccusage」是**正常状态**而不是错误：UI 要能如实显示 `unavailable` + 原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    Available,
    Unavailable,
}

/// 实际用哪个 runner 拿到数据。
///
/// 必须落库：数字对不上时要能回答「是数据变了还是 runner 变了」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunnerKind {
    /// PATH 上的可执行文件。
    #[serde(rename = "path")]
    Path,
    /// 用户显式配置的命令。
    #[serde(rename = "configured")]
    Configured,
    /// 固定版本的托管 runner（例如 `npx ccusage@20.0.24`），**永远不是 `@latest`**。
    #[serde(rename = "managed-npx")]
    ManagedNpx,
}

impl RunnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RunnerKind::Path => "path",
            RunnerKind::Configured => "configured",
            RunnerKind::ManagedNpx => "managed-npx",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "path" => Some(RunnerKind::Path),
            "configured" => Some(RunnerKind::Configured),
            "managed-npx" => Some(RunnerKind::ManagedNpx),
            _ => None,
        }
    }
}

/// 一个 Usage 数据源的只读视图（`get_usage_sources` 的返回元素）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSource {
    pub id: String,
    pub display_name: String,
    pub version: Option<String>,
    pub status: SourceStatus,
    pub capabilities: UsageCapabilities,
    pub runner: Option<RunnerKind>,
    /// `unavailable` 时的人话原因，直接显示给用户。
    pub reason: Option<String>,
}

/// Usage 能力矩阵。与 `HarnessCapabilities` 同一套「能力 = 代码实现了」的语义（ADR-0005）。
///
/// `Default` 全是 `false`：没实现就必须是 `false`，不允许「看起来应该支持」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCapabilities {
    /// 能发现本机数据源并读出真实版本。
    pub detect: bool,
    /// 能导入（幂等）。
    pub import: bool,
    /// 能监听数据源变化（v0.1 未实现，保持 false）。
    pub watch: bool,
    /// 能与来源自身的汇总口径对账。
    pub reconcile: bool,
}

/// 一次导入的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportStatus {
    Running,
    Succeeded,
    Failed,
}

impl ImportStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ImportStatus::Running => "running",
            ImportStatus::Succeeded => "succeeded",
            ImportStatus::Failed => "failed",
        }
    }
}

/// 一次导入的审计记录（`refresh_usage` 的返回类型，也存进 `usage_imports`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageImport {
    pub id: String,
    pub source: String,
    pub source_version: Option<String>,
    pub runner: Option<RunnerKind>,
    pub report_kind: Option<String>,
    pub status: ImportStatus,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub records_seen: u64,
    pub records_inserted: u64,
    pub records_updated: u64,
    pub records_skipped: u64,
    /// 无法推导发生时间、因而不能参与按天聚合的事件数（必须被显式解释）。
    pub records_timestampless: u64,
    pub error: Option<String>,
}

/// 归一化之后、尚未落库的一条事件。
///
/// 落库时由 importer 补上 `id` / `import_id` / `hub_session_id` / `imported_at`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageEventDraft {
    pub stable_source_key: String,
    pub key_version: i64,
    pub source: String,
    pub report_kind: String,
    pub harness: String,
    pub source_session_id: String,
    pub model: String,
    pub provider: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cached_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub total_tokens: u64,
    pub cost_microunits: Option<i64>,
    pub currency: Option<String>,
    pub currency_source: Option<String>,
    pub token_source: String,
    pub cost_source: Option<String>,
    pub pricing_mode: Option<String>,
    pub occurred_at: Option<String>,
    pub occurred_at_source: String,
    pub day: Option<String>,
    pub raw_payload: Option<String>,
}

/// 一份报告归一化之后的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedUsage {
    pub events: Vec<UsageEventDraft>,
    /// 来源自己给出的汇总；用于**同快照**对账（缺失时说明无法对账）。
    pub totals: Option<ReportTotals>,
    /// 报告里的 session 行数（不等于事件数：一行可以有多个模型）。
    pub sessions_seen: usize,
    /// `occurred_at_source == "unavailable"` 的事件数。
    pub timestampless: usize,
    /// 落在行汇总 `totalTokens` 里、但**任何模型明细都解释不了**的 token 数。
    ///
    /// 实测存在：222 行里有 1 行（opencode）多出 910。这部分无法归因到模型，
    /// 因此不能落进任何一条事件，但必须被计数 —— 否则对账差额就成了「未知」。
    pub row_total_residual: u64,
}

/// 来源报告的汇总快照，专门用于对账。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReportTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cached_input_tokens: u64,
    pub total_tokens: u64,
    pub cost_microunits: Option<i64>,
    /// `totals.unpricedModels` 的条数：非 0 意味着 cost 只是**下界**。
    pub unpriced_models: usize,
}

/// 把一批事件汇总成同样是「四类 token」的口径，用于对账。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EventTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cached_input_tokens: u64,
    pub total_tokens: u64,
    pub cost_microunits: i64,
    pub events_without_cost: usize,
    pub timestampless: usize,
}

impl EventTotals {
    pub fn from_drafts(events: &[UsageEventDraft]) -> Self {
        let mut totals = Self::default();
        for event in events {
            totals.input_tokens += event.input_tokens;
            totals.output_tokens += event.output_tokens;
            totals.cache_creation_tokens += event.cache_creation_tokens;
            totals.cached_input_tokens += event.cached_input_tokens.unwrap_or(0);
            totals.total_tokens += event.total_tokens;
            match event.cost_microunits {
                Some(cost) => totals.cost_microunits += cost,
                None => totals.events_without_cost += 1,
            }
            if event.occurred_at.is_none() {
                totals.timestampless += 1;
            }
        }
        totals
    }
}
