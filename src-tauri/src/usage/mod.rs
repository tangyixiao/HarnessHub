//! Usage 归一化：把不同来源的 Token / Cost 变成统一结构。
//!
//! 这里放**纯函数**（可独立测试），IO 与数据库写入由 `UsageAdapter` 实现者负责。

pub mod adapter;

use serde::{Deserialize, Serialize};

pub use adapter::{DataSource, DataSourceKind, ImportRange, ImportResult, UsageAdapter};

/// 归一化之后的一条 Usage 记录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    /// RFC3339 UTC。
    pub occurred_at: String,
    /// `YYYY-MM-DD`（UTC），用于日历聚合。
    pub day: String,
    pub harness_id: String,
    pub model_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: Option<f64>,
    /// 只做估算时必须为 true（见 docs/CONTEXT.md 核心约束 1）。
    pub cost_estimated: bool,
    /// 幂等去重键：同一份源数据重复导入必须产生同一个键。
    pub dedupe_key: String,
}

/// Token 汇总。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
    pub total_tokens: u64,
}

impl UsageTotals {
    /// 把一个 Session / 项目 / 时间窗口内的记录汇总起来。
    pub fn from_records(records: &[UsageRecord]) -> Self {
        records.iter().fold(Self::default(), |mut totals, record| {
            totals.input_tokens += record.input_tokens;
            totals.output_tokens += record.output_tokens;
            totals.cache_creation_tokens += record.cache_creation_tokens;
            totals.cache_read_tokens += record.cache_read_tokens;
            totals.total_tokens = totals.sum();
            totals
        })
    }

    /// 四项 Token 之和。刻意单独提供，避免各处重复写加法。
    pub fn sum(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_creation_tokens + self.cache_read_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(input: u64, output: u64, cache_read: u64) -> UsageRecord {
        UsageRecord {
            occurred_at: "2026-01-01T00:00:00Z".to_string(),
            day: "2026-01-01".to_string(),
            harness_id: "codex".to_string(),
            model_id: "openai/gpt-5".to_string(),
            input_tokens: input,
            output_tokens: output,
            cache_creation_tokens: 0,
            cache_read_tokens: cache_read,
            cost_usd: Some(0.01),
            cost_estimated: true,
            dedupe_key: format!("{input}-{output}-{cache_read}"),
        }
    }

    #[test]
    fn empty_input_yields_zero_totals() {
        assert_eq!(UsageTotals::from_records(&[]), UsageTotals::default());
        assert_eq!(UsageTotals::default().sum(), 0);
    }

    #[test]
    fn totals_sum_every_token_class() {
        let records = vec![record(10, 5, 2), record(1, 1, 1)];

        let totals = UsageTotals::from_records(&records);

        assert_eq!(totals.input_tokens, 11);
        assert_eq!(totals.output_tokens, 6);
        assert_eq!(totals.cache_read_tokens, 3);
        assert_eq!(totals.total_tokens, 20);
        assert_eq!(totals.total_tokens, totals.sum());
    }
}
