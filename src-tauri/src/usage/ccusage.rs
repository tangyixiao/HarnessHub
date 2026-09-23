//! ccusage unified JSON：形状识别 + 归一化。
//!
//! 真实结构见 `fixtures/ccusage/README.md`，契约见 ADR-0011。三条硬规则：
//!
//! 1. **一行 `modelBreakdowns` 条目 = 一条 UsageEvent**；session 行的 aggregate 只用于校验，
//!    绝不落库（两者都导会双计数）。
//! 2. **`daily` 分段永不导入**（同一批数据的另一种口径）。
//! 3. **形状不认识时报错并点名**，不允许静默少算 token。

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::error::{Error, Result};
use crate::usage::cost::decimal_to_microunits;
use crate::usage::key::{stable_source_key, KeyDimensions, KEY_VERSION};
use crate::usage::{NormalizedUsage, ReportTotals, UsageEventDraft};

/// `usage_events.source` 的取值。
pub const SOURCE_ID: &str = "ccusage";
/// `usage_events.report_kind` 的取值（v0.1 只导入 session）。
pub const REPORT_KIND: &str = "session";
/// Token 来源：外部聚合器读取 Harness 本地日志的结果，**不是** provider 直接上报。
pub const TOKEN_SOURCE: &str = "ccusage_source_log";
pub const COST_SOURCE_COMPUTED: &str = "ccusage_computed";
pub const COST_SOURCE_MISSING_PRICING: &str = "ccusage_missing_pricing";
/// ccusage 的 cost 依其文档契约是 USD，虽然 JSON 行里没有 currency 字段。
pub const CURRENCY: &str = "USD";
pub const CURRENCY_SOURCE: &str = "ccusage_contract";
/// v0.1 不传 `--mode`，因此等价于 ccusage 的默认 cost mode `auto`。
pub const PRICING_MODE_AUTO: &str = "auto";

/// ccusage 报告（unified schema v20.0.24）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CcusageReport {
    /// 缺失时为 `None`：这样才能区分「没有这个分段」与「分段是空的」。
    #[serde(default)]
    pub session: Option<Vec<CcusageSessionEntry>>,
    /// **只用于形状识别**：`daily` 与 `session` 是同一批数据的两种口径，
    /// 两者都导会双计数，所以这里保留它只为把错误信息说得更准（ADR-0011 决策二）。
    #[serde(default)]
    pub daily: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    pub totals: Option<CcusageTotals>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CcusageSessionEntry {
    pub agent: Option<String>,
    /// unified schema 的会话身份（Codex 是 rollout 路径片段）。
    pub period: Option<String>,
    /// focused/legacy schema 才会出现的字段：必须被**识别并报错**，而不是当空气。
    pub session_id: Option<String>,
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub metadata: Option<CcusageMetadata>,
    #[serde(default)]
    pub model_breakdowns: Option<Vec<CcusageModelBreakdown>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CcusageMetadata {
    /// 唯一可靠的「发生时间」来源。
    #[serde(default)]
    pub last_activity: Option<String>,
    /// session 级、**没有按模型拆分**的推理 token（见 ADR-0011 决策十一）。
    #[serde(default)]
    pub reasoning_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CcusageModelBreakdown {
    pub model_name: String,
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    /// 原始十进制文本：**不是** f64（见 `usage::cost`）。
    #[serde(default)]
    pub cost: Option<Box<RawValue>>,
    /// ccusage 用它显式表达「这个模型没有价格」。
    #[serde(default)]
    pub missing_pricing: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CcusageTotals {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub total_cost: Option<Box<RawValue>>,
    #[serde(default)]
    pub unpriced_models: Option<Vec<String>>,
}

/// 解析 + 形状识别。
///
/// 形状不认识时**点名**说清问题，而不是让 serde 的 `missing field` 变成一句
/// 「解析失败」再静默导入 0 条（ADR-0011 决策十）。
pub fn parse_report(raw: &str) -> Result<CcusageReport> {
    let report: CcusageReport = serde_json::from_str(raw)
        .map_err(|error| Error::UsageMalformed(format!("ccusage JSON 解析失败：{error}")))?;

    let Some(rows) = report.session.as_ref() else {
        let hint = if report.daily.is_some() {
            "报告里只有 daily 分段；"
        } else {
            ""
        };
        return Err(Error::UsageUnsupportedShape(format!(
            "{hint}找不到 session 数组。v0.1 只导入 unified session 报告，请用 \
             `ccusage session --sections daily --by-agent --json`"
        )));
    };

    // focused/legacy schema 用 sessionId 表示会话身份，我们没有它的 key 维度：
    // 必须报「不支持」而不是把它当空气（那会让数字静默变少）。
    for row in rows {
        if row.period.is_none() {
            if let Some(session_id) = row.session_id.as_deref() {
                return Err(Error::UsageUnsupportedShape(format!(
                    "session 行带 sessionId（{session_id}）但没有 period：这是 ccusage 的 \
                     focused session schema，v0.1 只支持 unified schema（period + modelBreakdowns）"
                )));
            }
        }
    }

    Ok(report)
}

/// 把一份 session 报告归一化成事件列表（纯函数，不碰数据库）。
pub fn normalize_session(report: &CcusageReport, pricing_mode: &str) -> Result<NormalizedUsage> {
    let rows = report.session.as_deref().unwrap_or(&[]);
    let mut events = Vec::with_capacity(rows.len());
    let mut timestampless = 0u64;
    let mut row_total_residual = 0u64;

    for row in rows {
        let Some(harness) = row.agent.as_deref() else {
            return Err(Error::UsageUnsupportedShape(
                "session 行缺少 agent，无法确定 harness".to_string(),
            ));
        };
        let Some(period) = row.period.as_deref() else {
            return Err(Error::UsageUnsupportedShape(
                "session 行缺少 period，无法确定会话身份".to_string(),
            ));
        };
        let breakdowns = row.model_breakdowns.as_deref().unwrap_or(&[]);

        if breakdowns.is_empty() {
            // 没有明细行时：0 token 的会话没有任何可丢的数据；
            // 有 token 却拿不到明细 = 会静默丢数据，必须拒绝。
            let declared = row.total_tokens.unwrap_or(0).max(token_sum_of_row(row));
            if declared > 0 {
                return Err(Error::UsageMalformed(format!(
                    "session {period} 有 {declared} tokens 却没有 modelBreakdowns：\
                     按模型落库会丢掉这些数据，拒绝导入"
                )));
            }
            continue;
        }

        validate_row_breakdowns(row, breakdowns).map(|residual| row_total_residual += residual)?;

        let (occurred_at, occurred_at_source, day) = resolve_occurred_at(row)?;
        // session 级、没有按模型拆分：只在单模型行上落库，绝不分摊编造。
        let reasoning_tokens = if breakdowns.len() == 1 {
            row.metadata
                .as_ref()
                .and_then(|metadata| metadata.reasoning_output_tokens)
        } else {
            None
        };

        for breakdown in breakdowns {
            let (cost_microunits, cost_source) = breakdown_cost(period, breakdown)?;
            let input_tokens = breakdown.input_tokens.unwrap_or(0);
            let output_tokens = breakdown.output_tokens.unwrap_or(0);
            let cache_creation_tokens = breakdown.cache_creation_tokens.unwrap_or(0);
            let cached_input_tokens = breakdown.cache_read_tokens;
            let total_tokens = input_tokens
                + output_tokens
                + cache_creation_tokens
                + cached_input_tokens.unwrap_or(0);

            if occurred_at.is_none() {
                timestampless += 1;
            }

            events.push(UsageEventDraft {
                stable_source_key: stable_source_key(&KeyDimensions {
                    source: SOURCE_ID,
                    report_kind: REPORT_KIND,
                    harness,
                    source_session_id: period,
                    model: &breakdown.model_name,
                }),
                key_version: KEY_VERSION,
                source: SOURCE_ID.to_string(),
                report_kind: REPORT_KIND.to_string(),
                harness: harness.to_string(),
                source_session_id: period.to_string(),
                model: breakdown.model_name.clone(),
                provider: None,
                input_tokens,
                output_tokens,
                cache_creation_tokens,
                cached_input_tokens,
                reasoning_tokens,
                total_tokens,
                cost_microunits,
                // 币种由来源契约确定：JSON 里没有 currency 字段 ≠ 币种未知（ADR-0011 决策五）。
                currency: Some(CURRENCY.to_string()),
                currency_source: Some(CURRENCY_SOURCE.to_string()),
                token_source: TOKEN_SOURCE.to_string(),
                cost_source: cost_source.map(str::to_string),
                pricing_mode: Some(pricing_mode.to_string()),
                occurred_at: occurred_at.clone(),
                occurred_at_source: occurred_at_source.to_string(),
                day: day.clone(),
                // 需要的字段都已显式建模；等到有来源带我们没建模的字段时再写原文，
                // 而不是把整份报告复制进每一行（ADR-0011 决策六的附注）。
                raw_payload: None,
            });
        }
    }

    Ok(NormalizedUsage {
        events,
        totals: totals_snapshot(report.totals.as_ref())?,
        sessions_seen: rows.len(),
        timestampless: timestampless as usize,
        row_total_residual,
    })
}

/// 行上四类 token 的和（用于判断「有 token 却没有明细」）。
fn token_sum_of_row(row: &CcusageSessionEntry) -> u64 {
    row.input_tokens.unwrap_or(0)
        + row.output_tokens.unwrap_or(0)
        + row.cache_creation_tokens.unwrap_or(0)
        + row.cache_read_tokens.unwrap_or(0)
}

/// `Σ(modelBreakdowns) == 行汇总` 的**四类 token** 是实测成立的恒等式（222/222 行）。
///
/// 四类对不上说明我们的模型理解错了 ccusage，或者上游改了语义 —— 静默按明细落库会让
/// 总量对不上而无人察觉，所以直接报错并给出数字。
///
/// 返回值是**行 `totalTokens` 与四类之和的差额**：这一项实测**不**总成立
/// （221/222 行成立，opencode 的一行多出 910），上游自己就自相矛盾。
/// 我们以四类为准（total = 四类之和），把这个差额单独计数而不是塞给某个模型。
fn validate_row_breakdowns(
    row: &CcusageSessionEntry,
    breakdowns: &[CcusageModelBreakdown],
) -> Result<u64> {
    let period = row.period.as_deref().unwrap_or("<无 period>");
    let sum =
        |pick: fn(&CcusageModelBreakdown) -> u64| -> u64 { breakdowns.iter().map(pick).sum() };

    let class_sums = [
        (
            "inputTokens",
            row.input_tokens,
            sum(|b| b.input_tokens.unwrap_or(0)),
        ),
        (
            "outputTokens",
            row.output_tokens,
            sum(|b| b.output_tokens.unwrap_or(0)),
        ),
        (
            "cacheCreationTokens",
            row.cache_creation_tokens,
            sum(|b| b.cache_creation_tokens.unwrap_or(0)),
        ),
        (
            "cacheReadTokens",
            row.cache_read_tokens,
            sum(|b| b.cache_read_tokens.unwrap_or(0)),
        ),
    ];

    let mut classes_total = 0u64;
    for (field, declared, actual) in class_sums {
        classes_total += actual;
        if let Some(declared) = declared {
            if declared != actual {
                return Err(Error::UsageMalformed(format!(
                    "session {period} 的 modelBreakdowns 与行汇总不一致：{field} 行={declared} 明细合计={actual}"
                )));
            }
        }
    }

    Ok(match row.total_tokens {
        Some(declared) if declared > classes_total => declared - classes_total,
        // 行汇总**小于**明细合计是另一种矛盾：那说明我们的明细读多了，属于真问题。
        Some(declared) if declared < classes_total => {
            return Err(Error::UsageMalformed(format!(
                "session {period} 的 modelBreakdowns 明细合计（{classes_total}）超过行 totalTokens（{declared}）"
            )));
        }
        _ => 0,
    })
}

/// 一个 breakdown 的金额与它的来源。
fn breakdown_cost(
    period: &str,
    breakdown: &CcusageModelBreakdown,
) -> Result<(Option<i64>, Option<&'static str>)> {
    let missing_pricing = breakdown.missing_pricing.unwrap_or(false);
    let text = breakdown
        .cost
        .as_ref()
        .map(|raw| raw.get().trim())
        .filter(|text| *text != "null");

    match (missing_pricing, text) {
        (true, Some(text)) => {
            let parsed = decimal_to_microunits(text)?;
            if parsed != 0 {
                return Err(Error::UsageMalformed(format!(
                    "session {period} 的 {} 标了 missingPricing 却给出金额 {text}：数据自相矛盾",
                    breakdown.model_name
                )));
            }
            Ok((None, Some(COST_SOURCE_MISSING_PRICING)))
        }
        // 没有价格的 0 元必须落 NULL：落 0 会把「未知」伪装成「免费」。
        (true, None) => Ok((None, Some(COST_SOURCE_MISSING_PRICING))),
        (false, None) => Ok((None, Some(COST_SOURCE_MISSING_PRICING))),
        (false, Some(text)) => Ok((
            Some(decimal_to_microunits(text)?),
            Some(COST_SOURCE_COMPUTED),
        )),
    }
}

/// 发生时间：源记录 → period 里的日期 → 不可推导（**不得**用导入时间冒充）。
fn resolve_occurred_at(
    row: &CcusageSessionEntry,
) -> Result<(Option<String>, &'static str, Option<String>)> {
    let period = row.period.as_deref().unwrap_or("<无 period>");

    if let Some(last_activity) = row
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.last_activity.as_deref())
    {
        let day = day_from_rfc3339(last_activity).ok_or_else(|| {
            Error::UsageMalformed(format!(
                "session {period} 的 metadata.lastActivity={last_activity:?} 不是时间戳"
            ))
        })?;
        return Ok((Some(last_activity.to_string()), "source_record", Some(day)));
    }

    if let Some(day) = day_from_period(period) {
        return Ok((Some(format!("{day}T00:00:00Z")), "period", Some(day)));
    }

    Ok((None, "unavailable", None))
}

/// `YYYY-MM-DD` 前缀校验；不合法返回 `None`（调用方决定是报错还是降级）。
fn day_from_rfc3339(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    if bytes.len() < 10 {
        return None;
    }
    let candidate = &text[..10];
    let shape_ok = candidate
        .char_indices()
        .all(|(index, character)| match index {
            4 | 7 => character == '-',
            _ => character.is_ascii_digit(),
        });
    if !shape_ok {
        return None;
    }
    if bytes
        .get(10)
        .is_some_and(|byte| *byte != b'T' && *byte != b' ')
    {
        return None;
    }
    Some(candidate.to_string())
}

/// Codex 的 `period` 形如 `YYYY/MM/DD/rollout-…`；其他 Harness 的 `period` 没有日期。
fn day_from_period(period: &str) -> Option<String> {
    let mut parts = period.split('/');
    let year = parts.next()?;
    let month = parts.next()?;
    let day = parts.next()?;
    let numeric = |text: &str, width: usize| {
        text.len() == width && text.bytes().all(|byte| byte.is_ascii_digit())
    };
    if numeric(year, 4) && numeric(month, 2) && numeric(day, 2) {
        Some(format!("{year}-{month}-{day}"))
    } else {
        None
    }
}

/// 报告自带的汇总 → 对账快照。
fn totals_snapshot(totals: Option<&CcusageTotals>) -> Result<Option<ReportTotals>> {
    let Some(totals) = totals else {
        return Ok(None);
    };
    let cost_microunits = match totals.total_cost.as_ref().map(|raw| raw.get().trim()) {
        None | Some("null") => None,
        Some(text) => Some(decimal_to_microunits(text)?),
    };

    Ok(Some(ReportTotals {
        input_tokens: totals.input_tokens.unwrap_or(0),
        output_tokens: totals.output_tokens.unwrap_or(0),
        cache_creation_tokens: totals.cache_creation_tokens.unwrap_or(0),
        cached_input_tokens: totals.cache_read_tokens.unwrap_or(0),
        total_tokens: totals.total_tokens.unwrap_or(0),
        cost_microunits,
        unpriced_models: totals.unpriced_models.as_ref().map_or(0, Vec::len),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_FIXTURE: &str = include_str!("../../../fixtures/ccusage/session.json");
    const SECTIONS_FIXTURE: &str = include_str!("../../../fixtures/ccusage/sections.json");
    const NOISY_COST_FIXTURE: &str = include_str!("../../../fixtures/ccusage/noisy-cost.json");
    const EMPTY_FIXTURE: &str = include_str!("../../../fixtures/ccusage/empty.json");
    const UNSUPPORTED_SHAPE_FIXTURE: &str =
        include_str!("../../../fixtures/ccusage/unsupported-shape.json");

    fn normalize(raw: &str) -> NormalizedUsage {
        let report = parse_report(raw).expect("解析");
        normalize_session(&report, PRICING_MODE_AUTO).expect("归一化")
    }

    fn event<'a>(usage: &'a NormalizedUsage, model: &str) -> &'a UsageEventDraft {
        usage
            .events
            .iter()
            .find(|event| event.model == model)
            .unwrap_or_else(|| panic!("没有模型 {model} 的事件"))
    }

    #[test]
    fn parses_the_committed_session_fixture() {
        let report = parse_report(SESSION_FIXTURE).expect("夹具必须可解析");

        assert_eq!(report.session.as_ref().expect("session").len(), 5);
        let totals = report.totals.expect("totals 必须解析出来");
        assert_eq!(totals.input_tokens, Some(13_303_993));
        assert_eq!(totals.total_tokens, Some(885_763_878));
        assert_eq!(
            totals.unpriced_models.as_ref().map(Vec::len),
            Some(1),
            "夹具里 GLM-5.3-Flash 没有价格"
        );
    }

    #[test]
    fn normalizes_one_event_per_model_breakdown() {
        let usage = normalize(SESSION_FIXTURE);

        assert_eq!(usage.sessions_seen, 5, "5 个 session 行");
        assert_eq!(usage.events.len(), 7, "2+1+1+1+2 个 breakdown");

        let rollout = "2026/08/20/rollout-2026-08-20T18-56-37-00000000-0000-4000-8000-000000000001";
        let mut sol_and_terra: Vec<&str> = usage
            .events
            .iter()
            .filter(|event| event.source_session_id == rollout)
            .map(|event| event.model.as_str())
            .collect();
        sol_and_terra.sort_unstable();
        assert_eq!(sol_and_terra, vec!["gpt-5.6-sol", "gpt-5.6-terra"]);

        assert_ne!(
            event(&usage, "gpt-5.6-sol").stable_source_key,
            event(&usage, "gpt-5.6-terra").stable_source_key,
            "同一会话的不同模型必须是不同的键"
        );
    }

    #[test]
    fn event_carries_full_provenance() {
        let usage = normalize(SESSION_FIXTURE);
        let luna = event(&usage, "gpt-5.6-luna");

        assert_eq!(luna.source, SOURCE_ID);
        assert_eq!(luna.report_kind, REPORT_KIND);
        assert_eq!(luna.harness, "codex");
        assert_eq!(luna.key_version, KEY_VERSION);
        assert_eq!(luna.token_source, TOKEN_SOURCE);
        assert_eq!(luna.cost_source.as_deref(), Some(COST_SOURCE_COMPUTED));
        assert_eq!(luna.currency.as_deref(), Some(CURRENCY));
        assert_eq!(luna.currency_source.as_deref(), Some(CURRENCY_SOURCE));
        assert_eq!(luna.pricing_mode.as_deref(), Some(PRICING_MODE_AUTO));
        assert_eq!(luna.provider, None, "ccusage 不报 provider");
        assert_eq!(
            luna.stable_source_key,
            stable_source_key(&KeyDimensions {
                source: SOURCE_ID,
                report_kind: REPORT_KIND,
                harness: "codex",
                source_session_id:
                    "2026/08/30/rollout-2026-08-30T13-30-39-00000000-0000-4000-8000-000000000002",
                model: "gpt-5.6-luna",
            }),
            "键必须由 usage::key 生成，不能就地拼"
        );
    }

    #[test]
    fn event_carries_the_token_classes() {
        let usage = normalize(SESSION_FIXTURE);
        let luna = event(&usage, "gpt-5.6-luna");

        assert_eq!(luna.input_tokens, 11_867_786);
        assert_eq!(luna.output_tokens, 1_675_327);
        assert_eq!(luna.cache_creation_tokens, 0);
        assert_eq!(luna.cached_input_tokens, Some(817_184_768));
        assert_eq!(luna.total_tokens, 830_727_881);
        assert_eq!(luna.cost_microunits, Some(21_120_615));
    }

    #[test]
    fn occurred_at_prefers_the_source_record_timestamp() {
        let usage = normalize(SESSION_FIXTURE);
        let luna = event(&usage, "gpt-5.6-luna");

        assert_eq!(
            luna.occurred_at.as_deref(),
            Some("2026-09-02T11:26:42.287Z")
        );
        assert_eq!(luna.occurred_at_source, "source_record");
        assert_eq!(luna.day.as_deref(), Some("2026-09-02"));
        assert_eq!(usage.timestampless, 0);
    }

    /// `metadata.reasoningOutputTokens` 是 session 级、没有按模型拆分。
    ///
    /// 多模型行上把它复制给每个模型就是**分摊编造**，所以只落单模型行。
    #[test]
    fn reasoning_tokens_are_only_attributed_when_the_session_has_one_model() {
        let usage = normalize(SESSION_FIXTURE);

        assert_eq!(
            event(&usage, "gpt-5.6-luna").reasoning_tokens,
            Some(540_968)
        );
        assert_eq!(
            event(&usage, "gpt-5.6-sol").reasoning_tokens,
            None,
            "多模型行的 session 级推理 token 不得分摊到各模型"
        );
        assert_eq!(event(&usage, "gpt-5.6-terra").reasoning_tokens, None);
    }

    #[test]
    fn missing_pricing_becomes_a_null_cost_with_its_own_source() {
        let usage = normalize(SESSION_FIXTURE);
        let unpriced = event(&usage, "GLM-5.3-Flash");

        assert_eq!(unpriced.harness, "zcode");
        assert_eq!(unpriced.cost_microunits, None, "没有价格不等于 0 元");
        assert_eq!(
            unpriced.cost_source.as_deref(),
            Some(COST_SOURCE_MISSING_PRICING)
        );
        assert_eq!(unpriced.currency.as_deref(), Some(CURRENCY));
    }

    #[test]
    fn every_cost_goes_through_the_money_boundary() {
        let usage = normalize(NOISY_COST_FIXTURE);

        assert_eq!(event(&usage, "gpt-5.6-sol").cost_microunits, Some(32_590));
        assert_eq!(
            event(&usage, "gpt-5.6-terra").cost_microunits,
            Some(21_120_615)
        );
        assert_eq!(
            event(&usage, "gpt-5.6-luna").cost_microunits,
            Some(2),
            "1.5e-6 恰好半个微单位：ties-to-even 得 2"
        );
        assert_eq!(event(&usage, "gpt-5.6-nova").cost_microunits, Some(2));
        assert_eq!(
            event(&usage, "gpt-5.6-iris").cost_microunits,
            None,
            "missingPricing 的行不能落 0"
        );
    }

    /// 同快照对账必须是**恒等式**，不是「差不多」。
    ///
    /// 实测：夹具里 opencode 的那一行，行 `totalTokens`（204906）比它自己的四类之和
    /// （203996）多 910 —— 上游自相矛盾。我们以四类为准，把这个差额单独计数，
    /// 于是 `Σ事件 + 差额 == totals` 精确成立。
    #[test]
    fn reconciles_against_the_reports_own_totals_on_the_fixture() {
        let usage = normalize(SESSION_FIXTURE);
        let totals = usage.totals.expect("夹具带 totals");
        let summed = crate::usage::EventTotals::from_drafts(&usage.events);

        assert_eq!(summed.input_tokens, totals.input_tokens);
        assert_eq!(summed.output_tokens, totals.output_tokens);
        assert_eq!(summed.cache_creation_tokens, totals.cache_creation_tokens);
        assert_eq!(summed.cached_input_tokens, totals.cached_input_tokens);
        assert_eq!(usage.row_total_residual, 910, "opencode 那行的实测差额");
        assert_eq!(
            summed.total_tokens + usage.row_total_residual,
            totals.total_tokens,
            "每一 token 都必须有归属：事件或明示的差额"
        );
        assert_eq!(summed.cost_microunits, totals.cost_microunits.unwrap_or(0));
        assert_eq!(
            summed.events_without_cost, totals.unpriced_models,
            "没有价格的事件数必须正好等于 totals.unpricedModels 的条数"
        );
    }

    /// 差额只按「上游汇总 > 明细」计数；反过来（明细比汇总还多）是我们读错了，要报错。
    #[test]
    fn rejects_breakdowns_that_exceed_the_row_total() {
        let raw = r#"{
            "session": [{
                "agent": "codex", "period": "2026/09/01/rollout-x",
                "totalTokens": 5,
                "metadata": { "lastActivity": "2026-09-01T00:00:00.000Z" },
                "modelBreakdowns": [{
                    "modelName": "gpt-5.6-sol",
                    "inputTokens": 100, "outputTokens": 0,
                    "cacheCreationTokens": 0, "cacheReadTokens": 0, "cost": 0
                }]
            }]
        }"#;

        let error = normalize_session(&parse_report(raw).expect("解析"), PRICING_MODE_AUTO)
            .expect_err("明细超过行汇总必须报错");

        assert!(error.to_string().contains("100"), "{error}");
    }

    #[test]
    fn a_consistent_row_contributes_no_residual() {
        let usage = normalize(NOISY_COST_FIXTURE);

        assert_eq!(usage.row_total_residual, 0);
    }

    #[test]
    fn rejects_a_breakdown_that_contradicts_its_row_aggregate() {
        let raw = r#"{
            "session": [{
                "agent": "codex",
                "period": "2026/09/01/rollout-x",
                "inputTokens": 100,
                "outputTokens": 0,
                "cacheCreationTokens": 0,
                "cacheReadTokens": 0,
                "totalTokens": 100,
                "modelBreakdowns": [{
                    "modelName": "gpt-5.6-sol",
                    "inputTokens": 90, "outputTokens": 0,
                    "cacheCreationTokens": 0, "cacheReadTokens": 0, "cost": 0.1
                }]
            }]
        }"#;

        let error = normalize_session(&parse_report(raw).expect("解析"), PRICING_MODE_AUTO)
            .expect_err("行汇总与明细不一致必须报错");

        let text = error.to_string();
        assert!(
            text.contains("100") && text.contains("90"),
            "错误必须给出数字：{text}"
        );
    }

    #[test]
    fn rejects_missing_pricing_that_also_reports_a_cost() {
        let raw = r#"{
            "session": [{
                "agent": "zcode", "period": "sess-x",
                "metadata": { "lastActivity": "2026-09-01T00:00:00.000Z" },
                "modelBreakdowns": [{
                    "modelName": "GLM-5.3-Flash",
                    "inputTokens": 1, "outputTokens": 1,
                    "cacheCreationTokens": 0, "cacheReadTokens": 0,
                    "cost": 1.5, "missingPricing": true
                }]
            }]
        }"#;

        let error = normalize_session(&parse_report(raw).expect("解析"), PRICING_MODE_AUTO)
            .expect_err("自相矛盾的定价标记必须报错");

        assert!(error.to_string().contains("missingPricing"), "{error}");
    }

    #[test]
    fn tolerates_unknown_fields() {
        // ccusage 加字段不能让我们炸掉：serde 默认忽略未知字段，这里把它钉住。
        let raw = r#"{
            "schemaVersion": 7,
            "session": [{
                "agent": "codex",
                "period": "2026/09/01/rollout-x",
                "brandNewField": { "nested": [1, 2, 3] },
                "inputTokens": 10, "outputTokens": 5,
                "cacheCreationTokens": 0, "cacheReadTokens": 0,
                "totalTokens": 15,
                "metadata": {
                    "lastActivity": "2026-09-01T00:00:00.000Z",
                    "futureMetadata": "whatever"
                },
                "modelBreakdowns": [{
                    "modelName": "gpt-5.6-sol",
                    "inputTokens": 10, "outputTokens": 5,
                    "cacheCreationTokens": 0, "cacheReadTokens": 0,
                    "cost": 0.5, "anotherNewField": true
                }]
            }]
        }"#;

        let usage = normalize(raw);

        assert_eq!(usage.events.len(), 1);
        assert_eq!(usage.events[0].total_tokens, 15);
    }

    #[test]
    fn optional_token_fields_stay_null_instead_of_becoming_zero() {
        let raw = r#"{
            "session": [{
                "agent": "codex", "period": "2026/09/01/rollout-x",
                "metadata": { "lastActivity": "2026-09-01T00:00:00.000Z" },
                "modelBreakdowns": [{
                    "modelName": "gpt-5.6-sol",
                    "inputTokens": 10, "outputTokens": 5,
                    "cost": 0.5
                }]
            }]
        }"#;

        let usage = normalize(raw);
        let only = &usage.events[0];

        assert_eq!(only.cached_input_tokens, None, "缺字段是「未知」，不是 0");
        assert_eq!(only.cache_creation_tokens, 0, "NOT NULL 的列退化为 0");
        assert_eq!(only.total_tokens, 15, "总数只加真实存在的类别");
    }

    #[test]
    fn falls_back_to_the_period_date_when_last_activity_is_absent() {
        let raw = r#"{
            "session": [{
                "agent": "codex", "period": "2026/08/30/rollout-2026-08-30T13-30-39-x",
                "modelBreakdowns": [{
                    "modelName": "gpt-5.6-sol",
                    "inputTokens": 1, "outputTokens": 1,
                    "cacheCreationTokens": 0, "cacheReadTokens": 0, "cost": 0
                }]
            }]
        }"#;

        let usage = normalize(raw);
        let only = &usage.events[0];

        assert_eq!(only.occurred_at.as_deref(), Some("2026-08-30T00:00:00Z"));
        assert_eq!(only.occurred_at_source, "period");
        assert_eq!(only.day.as_deref(), Some("2026-08-30"));
        assert_eq!(usage.timestampless, 0);
    }

    #[test]
    fn leaves_occurred_at_null_when_it_cannot_be_derived() {
        let raw = r#"{
            "session": [{
                "agent": "zcode", "period": "sess_05fc9459-03e3-42f8-aca8-1defef0c0c0b",
                "modelBreakdowns": [{
                    "modelName": "GLM-5.3-Flash",
                    "inputTokens": 1, "outputTokens": 1,
                    "cacheCreationTokens": 0, "cacheReadTokens": 0, "cost": 0,
                    "missingPricing": true
                }]
            }]
        }"#;

        let usage = normalize(raw);
        let only = &usage.events[0];

        assert_eq!(only.occurred_at, None, "不得用导入时间冒充发生时间");
        assert_eq!(only.occurred_at_source, "unavailable");
        assert_eq!(only.day, None);
        assert_eq!(
            usage.timestampless, 1,
            "无时间戳的事件必须被计数，便于解释对账差额"
        );
    }

    #[test]
    fn rejects_a_last_activity_that_is_not_a_timestamp() {
        let raw = r#"{
            "session": [{
                "agent": "codex", "period": "2026/09/01/rollout-x",
                "metadata": { "lastActivity": "yesterday" },
                "modelBreakdowns": [{
                    "modelName": "gpt-5.6-sol",
                    "inputTokens": 1, "outputTokens": 1,
                    "cacheCreationTokens": 0, "cacheReadTokens": 0, "cost": 0
                }]
            }]
        }"#;

        let error = normalize_session(&parse_report(raw).expect("解析"), PRICING_MODE_AUTO)
            .expect_err("非法时间戳必须报错而不是降级成 NULL");

        assert!(error.to_string().contains("yesterday"), "{error}");
    }

    #[test]
    fn names_the_unsupported_shape_instead_of_silently_importing_nothing() {
        let error = parse_report(UNSUPPORTED_SHAPE_FIXTURE).expect_err("必须报错");

        match error {
            Error::UsageUnsupportedShape(message) => {
                assert!(
                    message.contains("sessionId"),
                    "错误必须点名看到的字段：{message}"
                );
            }
            other => panic!("必须是 UsageUnsupportedShape，实际：{other}"),
        }
    }

    #[test]
    fn a_daily_only_report_is_not_an_import_source() {
        let raw = r#"{
            "daily": [{ "agent": "all", "period": "2026-09-20", "inputTokens": 1 }],
            "totals": { "inputTokens": 1 }
        }"#;

        let error = parse_report(raw).expect_err("daily-only 报告不可导入");

        match error {
            Error::UsageUnsupportedShape(message) => {
                assert!(
                    message.contains("session"),
                    "要说明缺的是 session：{message}"
                );
            }
            other => panic!("必须是 UsageUnsupportedShape，实际：{other}"),
        }
    }

    #[test]
    fn refuses_a_session_that_has_tokens_but_no_breakdowns() {
        let raw = r#"{
            "session": [{
                "agent": "codex", "period": "2026/09/01/rollout-x",
                "inputTokens": 500, "totalTokens": 500,
                "modelBreakdowns": []
            }]
        }"#;

        let error = normalize_session(&parse_report(raw).expect("解析"), PRICING_MODE_AUTO)
            .expect_err("有 token 却没有明细 = 会丢数据，必须报错");

        let text = error.to_string();
        assert!(text.contains("500"), "错误必须给出会丢掉的量：{text}");
    }

    #[test]
    fn a_zero_token_session_without_breakdowns_contributes_nothing() {
        let raw = r#"{
            "session": [{
                "agent": "codex", "period": "2026/09/01/rollout-x",
                "inputTokens": 0, "totalTokens": 0,
                "modelBreakdowns": []
            }]
        }"#;

        let usage = normalize(raw);

        assert!(usage.events.is_empty(), "0 token 的会话没有可丢的数据");
        assert_eq!(usage.sessions_seen, 1);
    }

    #[test]
    fn an_empty_report_is_a_successful_empty_import() {
        let usage = normalize(EMPTY_FIXTURE);

        assert!(usage.events.is_empty());
        assert_eq!(usage.sessions_seen, 0);
        assert_eq!(usage.timestampless, 0);
        let totals = usage.totals.expect("空报告也有 totals");
        assert_eq!(totals.total_tokens, 0);
        assert_eq!(totals.cost_microunits, Some(0));
    }

    #[test]
    fn reports_truncated_json_as_malformed() {
        let error = parse_report("{\"session\": [{\"agent\": \"codex\",").expect_err("必须报错");

        match error {
            Error::UsageMalformed(message) => {
                assert!(!message.is_empty(), "必须带上 serde 的原始说明");
            }
            other => panic!("必须是 UsageMalformed，实际：{other}"),
        }
    }

    #[test]
    fn sections_reports_import_the_session_section_only() {
        let usage = normalize(SECTIONS_FIXTURE);

        assert_eq!(
            usage.events.len(),
            2,
            "只导入 session 分段；daily 也导会双计数"
        );
        assert_eq!(usage.sessions_seen, 2);
        let totals = usage.totals.expect("totals");
        assert_eq!(
            crate::usage::EventTotals::from_drafts(&usage.events).total_tokens,
            totals.total_tokens,
            "同一次调用的 totals 与 session 分段同口径"
        );
    }

    #[test]
    fn a_report_without_totals_has_no_reconciliation_target() {
        let raw = r#"{
            "session": [{
                "agent": "codex", "period": "2026/09/01/rollout-x",
                "metadata": { "lastActivity": "2026-09-01T00:00:00.000Z" },
                "modelBreakdowns": [{
                    "modelName": "gpt-5.6-sol",
                    "inputTokens": 1, "outputTokens": 1,
                    "cacheCreationTokens": 0, "cacheReadTokens": 0, "cost": 0
                }]
            }]
        }"#;

        let usage = normalize(raw);

        assert_eq!(usage.events.len(), 1);
        assert!(usage.totals.is_none(), "没有 totals 就必须如实说无法对账");
    }

    #[test]
    fn rejects_a_breakdown_without_a_model_name() {
        let raw = r#"{
            "session": [{
                "agent": "codex", "period": "2026/09/01/rollout-x",
                "modelBreakdowns": [{ "inputTokens": 1, "outputTokens": 1 }]
            }]
        }"#;

        assert!(
            parse_report(raw).is_err(),
            "没有 modelName 无法生成 natural key"
        );
    }

    #[test]
    fn rejects_a_session_row_without_agent_or_period() {
        let raw = r#"{
            "session": [{
                "modelBreakdowns": [{
                    "modelName": "gpt-5.6-sol",
                    "inputTokens": 1, "outputTokens": 1,
                    "cacheCreationTokens": 0, "cacheReadTokens": 0, "cost": 0
                }]
            }]
        }"#;

        let error = normalize_session(&parse_report(raw).expect("解析"), PRICING_MODE_AUTO)
            .expect_err("session 行没有 agent/period 就不能落库");

        assert!(matches!(error, Error::UsageUnsupportedShape(_)), "{error}");
    }

    #[test]
    fn report_totals_are_absent_when_the_source_omits_them() {
        let report = parse_report(r#"{"session": []}"#).expect("解析");

        assert!(report.totals.is_none());
        assert_eq!(
            normalize_session(&report, PRICING_MODE_AUTO)
                .expect("归一化")
                .events
                .len(),
            0
        );
    }

    #[test]
    fn unpriced_model_count_is_reported_for_lower_bound_costs() {
        let usage = normalize(SESSION_FIXTURE);
        let totals: ReportTotals = usage.totals.expect("totals");

        assert_eq!(totals.unpriced_models, 1);
        let summed = crate::usage::EventTotals::from_drafts(&usage.events);
        assert_eq!(summed.events_without_cost, 1);
    }
}
