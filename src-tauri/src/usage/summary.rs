//! Dashboard 的聚合投影（ADR-0012）。
//!
//! 三条不可动摇的规则：
//!
//! 1. **只读**：本模块只接 `&Connection`，不接 runner / executor / adapter，
//!    因此渲染路径结构上**不可能**起 ccusage 或写数据库。
//! 2. **从明细算**：聚合只读 `usage_events` / `sessions`，不读任何缓存总计，
//!    更不读 ccusage 的 `totals`（那是 Task 5 的对账证据，不是展示事实）。
//! 3. **两个会话数分开**：`usage_sessions`（外部历史里的会话）与
//!    `managed_sessions`（Harness Hub 自己管理的会话）不是一个概念，不得合并。

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// 时间范围。参数解析失败**必须报错**，不得退化成「全部时间」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageRange {
    Today,
    Days7,
    Days30,
    All,
}

impl UsageRange {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "today" => Some(UsageRange::Today),
            "7d" => Some(UsageRange::Days7),
            "30d" => Some(UsageRange::Days30),
            "all" => Some(UsageRange::All),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            UsageRange::Today => "today",
            UsageRange::Days7 => "7d",
            UsageRange::Days30 => "30d",
            UsageRange::All => "all",
        }
    }

    /// 窗口包含多少个本地日（`all` 为 `None`）。
    fn local_days(self) -> Option<i64> {
        match self {
            UsageRange::Today => Some(1),
            UsageRange::Days7 => Some(7),
            UsageRange::Days30 => Some(30),
            UsageRange::All => None,
        }
    }
}

/// 解析出来的时间窗（左闭右开 `[start, end)`，UTC RFC3339）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRangeWindow {
    pub kind: UsageRange,
    pub start_utc: Option<String>,
    pub end_utc: Option<String>,
    /// 加到 UTC 上得到本地时间的分钟数（UTC+8 → 480）。
    pub timezone_offset_minutes: i32,
    pub now_utc: String,
}

impl UsageRangeWindow {
    pub fn is_bounded(&self) -> bool {
        self.start_utc.is_some() && self.end_utc.is_some()
    }

    /// SQLite 的时区修饰符，例如 `+480 minutes`。
    fn modifier(&self) -> String {
        format!("{:+} minutes", self.timezone_offset_minutes)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotalsSummary {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cache_creation_tokens: u64,
    /// 仅在单模型行上有值（ADR-0011 决策十二），因此这是**下界**。
    pub reasoning_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBucket {
    pub key: String,
    pub total_tokens: u64,
    /// `None` = 该分组内没有任何有价格的记录（不是 0）。
    pub cost_microunits: Option<i64>,
    pub cost_is_lower_bound: bool,
    pub event_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDayBucket {
    /// **本地**日（按 `timezone_offset_minutes` 换算）。
    pub day: String,
    pub total_tokens: u64,
    pub cost_microunits: Option<i64>,
    pub cost_is_lower_bound: bool,
    pub event_count: u64,
}

/// 跨 IPC 的稳定 DTO。前端只负责格式化，不做任何判定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub range: UsageRangeWindow,
    pub totals: UsageTotalsSummary,
    /// `None` = 一条有价格的记录都没有（**不是** 0）。
    pub cost_microunits: Option<i64>,
    /// 有金额时恒为 `USD`（ADR-0011 决策五）。
    pub currency: Option<String>,
    /// 由**后端**判定：窗口内存在缺价格记录时为 true。
    /// 前端只有两种渲染：精确金额 / `≥ 金额`。
    pub cost_is_lower_bound: bool,
    pub missing_pricing_records: u64,
    /// 窗口内没有发生时间的事件数（有界窗口恒为 0，它们被排除在外）。
    pub timestampless_records: u64,
    /// 因为「没有发生时间」而被有界窗口排除掉的事件数。
    pub excluded_timestampless: u64,
    pub event_count: u64,
    /// 明细里 distinct `(harness, source_session_id)`：外部历史里的会话。
    pub usage_sessions: u64,
    /// `sessions` 表的行数：Harness Hub 真正启动/管理过的会话。
    pub managed_sessions: u64,
    pub by_harness: Vec<UsageBucket>,
    pub by_model: Vec<UsageBucket>,
    /// v0.1 里 `project_id` 恒为 NULL，因此通常为空 —— 不伪造归属。
    pub by_project: Vec<UsageBucket>,
    pub timeline: Vec<UsageDayBucket>,
}

/// 由 `range + timezone_offset_minutes + now` 计算 `[start, end)`。
///
/// 日期算术交给 SQLite（`datetime()`），不自己实现日历：DST / 月长 / 闰年这类
/// 逻辑是最容易写错又最容易被忽略的一类代码。
///
/// 做法：先把 UTC 时刻按偏移**平移**成本地墙上时间，取本地零点，再平移回 UTC：
///
/// ```text
/// datetime(now, '+480 minutes', 'start of day', '-480 minutes')
/// ```
pub fn range_bounds(
    connection: &Connection,
    range: UsageRange,
    now_utc: &str,
    timezone_offset_minutes: i32,
) -> Result<UsageRangeWindow> {
    let window = UsageRangeWindow {
        kind: range,
        start_utc: None,
        end_utc: None,
        timezone_offset_minutes,
        now_utc: now_utc.to_string(),
    };

    let Some(days) = range.local_days() else {
        return Ok(window);
    };

    let to_local = window.modifier();
    let to_utc = format!("{:+} minutes", -timezone_offset_minutes);
    let back = format!("-{} days", days - 1);

    let start: String = connection.query_row(
        "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', datetime(?1, ?2, 'start of day', ?3, ?4))",
        params![now_utc, to_local, back, to_utc],
        |row| row.get(0),
    )?;
    let end: String = connection.query_row(
        "SELECT strftime('%Y-%m-%dT%H:%M:%SZ', datetime(?1, ?2, 'start of day', '+1 day', ?3))",
        params![now_utc, to_local, to_utc],
        |row| row.get(0),
    )?;

    Ok(UsageRangeWindow {
        start_utc: Some(start),
        end_utc: Some(end),
        ..window
    })
}

/// 聚合。纯读，不看 ccusage，不看缓存总计。
pub fn summary(connection: &Connection, window: &UsageRangeWindow) -> Result<UsageSummary> {
    let (filter, range_params) = range_filter(window, 1)?;

    let totals = connection.query_row(
        &format!(
            "SELECT
                 COALESCE(SUM(input_tokens), 0),
                 COALESCE(SUM(output_tokens), 0),
                 COALESCE(SUM(cached_input_tokens), 0),
                 COALESCE(SUM(cache_creation_tokens), 0),
                 COALESCE(SUM(reasoning_tokens), 0),
                 COALESCE(SUM(total_tokens), 0),
                 SUM(cost_microunits),
                 COUNT(*) FILTER (WHERE cost_microunits IS NULL),
                 COUNT(*) FILTER (WHERE occurred_at IS NULL),
                 COUNT(*)
             FROM usage_events {filter}"
        ),
        rusqlite::params_from_iter(range_params.iter()),
        |row| {
            Ok((
                UsageTotalsSummary {
                    input_tokens: row.get::<_, i64>(0)? as u64,
                    output_tokens: row.get::<_, i64>(1)? as u64,
                    cached_input_tokens: row.get::<_, i64>(2)? as u64,
                    cache_creation_tokens: row.get::<_, i64>(3)? as u64,
                    reasoning_tokens: row.get::<_, i64>(4)? as u64,
                    total_tokens: row.get::<_, i64>(5)? as u64,
                },
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, i64>(7)? as u64,
                row.get::<_, i64>(8)? as u64,
                row.get::<_, i64>(9)? as u64,
            ))
        },
    )?;
    let (totals, summed_cost, missing_pricing_records, timestampless_records, event_count) = totals;

    // 多币种时**不能**把微单位相加（那是无意义的数字），因此宁可如实说「不给金额」。
    let currencies = distinct_currencies(connection, &filter, &range_params)?;
    let mut cost_microunits = summed_cost;
    let currency = match currencies.len() {
        0 => None,
        1 => Some(currencies[0].clone()),
        _ => None,
    };
    if currencies.len() > 1 {
        cost_microunits = None;
    }

    let usage_sessions: i64 = connection.query_row(
        &format!(
            "SELECT COUNT(*) FROM (
                 SELECT DISTINCT harness, source_session_id FROM usage_events {filter}
             )"
        ),
        rusqlite::params_from_iter(range_params.iter()),
        |row| row.get(0),
    )?;

    let managed_filter = managed_session_filter(window);
    let managed_sessions: i64 = connection.query_row(
        &format!("SELECT COUNT(*) FROM sessions {managed_filter}"),
        rusqlite::params_from_iter(range_params.iter()),
        |row| row.get(0),
    )?;

    // 有界窗口里「没有发生时间」的事件根本进不来，因此 `timestampless_records` 天然为 0；
    // 它们必须被单独计数，否则「All 比各窗口之和多」就成了无法解释的数字。
    let excluded_timestampless: i64 = if window.is_bounded() {
        connection.query_row(
            "SELECT COUNT(*) FROM usage_events WHERE occurred_at IS NULL",
            [],
            |row| row.get(0),
        )?
    } else {
        0
    };

    Ok(UsageSummary {
        range: window.clone(),
        totals,
        cost_microunits,
        currency,
        cost_is_lower_bound: missing_pricing_records > 0 || currencies.len() > 1,
        missing_pricing_records,
        timestampless_records,
        excluded_timestampless: excluded_timestampless as u64,
        event_count,
        usage_sessions: usage_sessions as u64,
        managed_sessions: managed_sessions as u64,
        by_harness: buckets(connection, window, "harness", false)?,
        by_model: buckets(connection, window, "model", false)?,
        by_project: buckets(connection, window, "project_id", true)?,
        timeline: timeline(connection, window)?,
    })
}

/// 事件表的范围过滤条件。
///
/// `first_index` 是占位符编号起点：timeline 的 SQL 把时区修饰符放在 `?1`，
/// 因此范围参数必须从 `?2` 开始 —— 否则两处都会绑到 `?1` 上（真被这个坑过一次）。
fn range_filter(window: &UsageRangeWindow, first_index: usize) -> Result<(String, Vec<String>)> {
    match (&window.start_utc, &window.end_utc) {
        (Some(start), Some(end)) => Ok((
            // 用 `datetime()` 比较：库里两种时间格式并存（带毫秒的 lastActivity 与
            // 不带毫秒的 period 回退值），直接字符串比较会在边界上判错。
            format!(
                "WHERE datetime(occurred_at) >= datetime(?{first_index}) \
                 AND datetime(occurred_at) < datetime(?{next})",
                next = first_index + 1
            ),
            vec![start.clone(), end.clone()],
        )),
        _ => Ok((String::new(), Vec::new())),
    }
}

/// 会话表的范围过滤（按 `started_at`）。
fn managed_session_filter(window: &UsageRangeWindow) -> String {
    if window.is_bounded() {
        "WHERE datetime(started_at) >= datetime(?1) AND datetime(started_at) < datetime(?2)"
            .to_string()
    } else {
        String::new()
    }
}

fn distinct_currencies(
    connection: &Connection,
    filter: &str,
    range_params: &[String],
) -> Result<Vec<String>> {
    let mut statement = connection.prepare(&format!(
        "SELECT DISTINCT currency FROM usage_events {filter}
         {}",
        if filter.is_empty() {
            "WHERE currency IS NOT NULL"
        } else {
            "AND currency IS NOT NULL"
        }
    ))?;
    let rows = statement.query_map(rusqlite::params_from_iter(range_params.iter()), |row| {
        row.get::<_, String>(0)
    })?;
    let mut currencies = Vec::new();
    for row in rows {
        currencies.push(row?);
    }
    Ok(currencies)
}

fn buckets(
    connection: &Connection,
    window: &UsageRangeWindow,
    column: &str,
    only_assigned: bool,
) -> Result<Vec<UsageBucket>> {
    let (filter, range_params) = range_filter(window, 1)?;
    let extra = match (filter.is_empty(), only_assigned) {
        (true, true) => "WHERE project_id IS NOT NULL".to_string(),
        (true, false) => String::new(),
        (false, true) => "AND project_id IS NOT NULL".to_string(),
        (false, false) => String::new(),
    };

    let mut statement = connection.prepare(&format!(
        "SELECT {column} AS bucket_key,
                COALESCE(SUM(total_tokens), 0),
                SUM(cost_microunits),
                COUNT(*) FILTER (WHERE cost_microunits IS NULL),
                COUNT(*)
         FROM usage_events {filter} {extra}
         GROUP BY bucket_key
         ORDER BY SUM(total_tokens) DESC, bucket_key ASC"
    ))?;
    let rows = statement.query_map(rusqlite::params_from_iter(range_params.iter()), |row| {
        let key: Option<String> = row.get(0)?;
        let missing: i64 = row.get(3)?;
        Ok(UsageBucket {
            key: key.unwrap_or_default(),
            total_tokens: row.get::<_, i64>(1)? as u64,
            cost_microunits: row.get(2)?,
            cost_is_lower_bound: missing > 0,
            event_count: row.get::<_, i64>(4)? as u64,
        })
    })?;

    let mut buckets = Vec::new();
    for row in rows {
        let bucket = row?;
        if bucket.key.is_empty() {
            continue;
        }
        buckets.push(bucket);
    }
    Ok(buckets)
}

fn timeline(connection: &Connection, window: &UsageRangeWindow) -> Result<Vec<UsageDayBucket>> {
    // 修饰符占 ?1，因此范围参数从 ?2 开始。
    let (filter, range_params) = range_filter(window, 2)?;
    let modifier = window.modifier();
    let time_filter = if filter.is_empty() {
        "WHERE occurred_at IS NOT NULL".to_string()
    } else {
        "AND occurred_at IS NOT NULL".to_string()
    };

    let mut statement = connection.prepare(&format!(
        "SELECT strftime('%Y-%m-%d', occurred_at, ?1) AS local_day,
                COALESCE(SUM(total_tokens), 0),
                SUM(cost_microunits),
                COUNT(*) FILTER (WHERE cost_microunits IS NULL),
                COUNT(*)
         FROM usage_events {filter} {time_filter}
         GROUP BY local_day
         ORDER BY local_day ASC"
    ))?;

    // 时间修饰符是第一个参数，范围参数跟在后面。
    let mut all_params: Vec<String> = vec![modifier];
    all_params.extend(range_params);
    let rows = statement.query_map(rusqlite::params_from_iter(all_params.iter()), |row| {
        let missing: i64 = row.get(3)?;
        Ok(UsageDayBucket {
            day: row.get(0)?,
            total_tokens: row.get::<_, i64>(1)? as u64,
            cost_microunits: row.get(2)?,
            cost_is_lower_bound: missing > 0,
            event_count: row.get::<_, i64>(4)? as u64,
        })
    })?;

    let mut buckets = Vec::new();
    for row in rows {
        buckets.push(row?);
    }
    Ok(buckets)
}

/// 参数解析：`range` 不合法必须报错。
pub fn parse_range(text: &str) -> Result<UsageRange> {
    UsageRange::parse(text).ok_or_else(|| {
        Error::InvalidInput(format!(
            "未知的时间范围 {text:?}；合法值：today / 7d / 30d / all"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::usage::importer::{request_from, ImportRequest, UsageImporter};
    use crate::usage::{NormalizedUsage, ReportTotals, UsageEventDraft};

    const NOW: &str = "2026-09-22T23:30:00Z";
    /// UTC+8：上面的 NOW 在本地是 2026-09-23 07:30。
    const UTC_PLUS_8: i32 = 480;

    fn draft(
        key: &str,
        harness: &str,
        period: &str,
        model: &str,
        total: u64,
        cost: Option<i64>,
        occurred_at: Option<&str>,
    ) -> UsageEventDraft {
        UsageEventDraft {
            stable_source_key: key.to_string(),
            key_version: 1,
            source: "ccusage".to_string(),
            report_kind: "session".to_string(),
            harness: harness.to_string(),
            source_session_id: period.to_string(),
            model: model.to_string(),
            provider: None,
            input_tokens: total,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cached_input_tokens: Some(0),
            reasoning_tokens: None,
            total_tokens: total,
            cost_microunits: cost,
            currency: Some("USD".to_string()),
            currency_source: Some("ccusage_contract".to_string()),
            token_source: "ccusage_source_log".to_string(),
            cost_source: Some(match cost {
                Some(_) => "ccusage_computed".to_string(),
                None => "ccusage_missing_pricing".to_string(),
            }),
            pricing_mode: Some("auto".to_string()),
            occurred_at: occurred_at.map(str::to_string),
            occurred_at_source: if occurred_at.is_some() {
                "source_record".to_string()
            } else {
                "unavailable".to_string()
            },
            day: occurred_at.map(|value| value[..10].to_string()),
            raw_payload: None,
        }
    }

    /// 用**生产写入路径**（importer）填数据，而不是手写 INSERT。
    ///
    /// 库用**生产引导路径**（`cold_start_db`）建：写 `sessions` 需要 harness 与
    /// runtime target 行，而这两行本来就该由生产代码创建。
    fn db_with(events: Vec<UsageEventDraft>) -> Database {
        let db = crate::test_support::cold_start_db();
        let normalized = NormalizedUsage {
            events,
            totals: None,
            sessions_seen: 0,
            timestampless: 0,
            row_total_residual: 0,
        };
        UsageImporter::new(db.connection())
            .import(&request_from(
                &normalized,
                "ccusage",
                Some("ccusage 20.0.24"),
                None,
                "session",
                "2026-09-22T10:00:00Z",
            ))
            .expect("导入");
        db
    }

    fn window(db: &Database, range: UsageRange) -> UsageRangeWindow {
        range_bounds(db.connection(), range, NOW, UTC_PLUS_8).expect("窗口")
    }

    fn summarize(db: &Database, range: UsageRange) -> UsageSummary {
        summary(db.connection(), &window(db, range)).expect("聚合")
    }

    // ---- Step 1：窗口 --------------------------------------------------

    /// 本地 23:30 的 UTC 时刻必须落在**本地**今天里，而不是 UTC 今天。
    #[test]
    fn today_uses_the_local_day_boundary_converted_to_utc() {
        let db = crate::test_support::empty_db();

        let today = window(&db, UsageRange::Today);

        assert_eq!(today.start_utc.as_deref(), Some("2026-09-22T16:00:00Z"));
        assert_eq!(today.end_utc.as_deref(), Some("2026-09-23T16:00:00Z"));
        assert_eq!(today.timezone_offset_minutes, UTC_PLUS_8);
        assert_eq!(today.now_utc, NOW);
    }

    #[test]
    fn seven_and_thirty_days_are_left_closed_right_open_windows() {
        let db = crate::test_support::empty_db();

        let week = window(&db, UsageRange::Days7);
        assert_eq!(week.start_utc.as_deref(), Some("2026-09-16T16:00:00Z"));
        assert_eq!(week.end_utc.as_deref(), Some("2026-09-23T16:00:00Z"));

        let month = window(&db, UsageRange::Days30);
        assert_eq!(month.start_utc.as_deref(), Some("2026-08-24T16:00:00Z"));
        assert_eq!(month.end_utc.as_deref(), Some("2026-09-23T16:00:00Z"));
    }

    #[test]
    fn all_has_no_bounds() {
        let db = crate::test_support::empty_db();

        let all = window(&db, UsageRange::All);

        assert_eq!(all.start_utc, None);
        assert_eq!(all.end_utc, None);
        assert!(!all.is_bounded());
    }

    #[test]
    fn a_negative_offset_shifts_the_other_way() {
        let db = crate::test_support::empty_db();

        // UTC-5（纽约冬令时）：NOW 本地是 2026-09-22 18:30 → 本地今天从 05:00Z 开始。
        let today = range_bounds(db.connection(), UsageRange::Today, NOW, -300).expect("窗口");

        assert_eq!(today.start_utc.as_deref(), Some("2026-09-22T05:00:00Z"));
        assert_eq!(today.end_utc.as_deref(), Some("2026-09-23T05:00:00Z"));
    }

    #[test]
    fn an_unknown_range_is_rejected_instead_of_defaulting_to_all() {
        assert!(parse_range("yesterday").is_err());
        assert!(parse_range("").is_err());
        assert_eq!(parse_range("7d").expect("合法"), UsageRange::Days7);
    }

    // ---- Step 2：聚合 --------------------------------------------------

    #[test]
    fn an_empty_database_returns_zeros_and_no_cost_not_an_error() {
        let db = crate::test_support::empty_db();

        let summary = summarize(&db, UsageRange::All);

        assert_eq!(summary.totals.total_tokens, 0);
        assert_eq!(summary.event_count, 0);
        assert_eq!(summary.usage_sessions, 0);
        assert_eq!(summary.managed_sessions, 0);
        assert_eq!(summary.cost_microunits, None, "没有金额不是 0 元");
        assert_eq!(summary.currency, None);
        assert!(!summary.cost_is_lower_bound, "没有数据不是「下界」");
        assert!(summary.by_model.is_empty());
        assert!(summary.timeline.is_empty());
    }

    #[test]
    fn one_event_is_counted_in_tokens_cost_and_one_usage_session() {
        let db = db_with(vec![draft(
            "key-1",
            "codex",
            "rollout-a",
            "gpt-5.6-sol",
            100,
            Some(1_500),
            Some("2026-09-22T10:00:00Z"),
        )]);

        let summary = summarize(&db, UsageRange::All);

        assert_eq!(summary.totals.total_tokens, 100);
        assert_eq!(summary.totals.input_tokens, 100);
        assert_eq!(summary.cost_microunits, Some(1_500));
        assert_eq!(summary.currency.as_deref(), Some("USD"));
        assert!(!summary.cost_is_lower_bound);
        assert_eq!(summary.usage_sessions, 1);
        assert_eq!(summary.event_count, 1);
        assert_eq!(summary.missing_pricing_records, 0);
    }

    /// 防双计数：一个会话里两个模型 → **1** 个 usage session，2 个事件。
    #[test]
    fn two_models_in_one_session_count_as_one_usage_session() {
        let db = db_with(vec![
            draft(
                "key-1",
                "codex",
                "rollout-a",
                "gpt-5.6-sol",
                100,
                Some(1_000),
                Some("2026-09-22T10:00:00Z"),
            ),
            draft(
                "key-2",
                "codex",
                "rollout-a",
                "gpt-5.6-terra",
                200,
                Some(2_000),
                Some("2026-09-22T10:00:00Z"),
            ),
        ]);

        let summary = summarize(&db, UsageRange::All);

        assert_eq!(summary.usage_sessions, 1, "同一 period 只算一个会话");
        assert_eq!(summary.event_count, 2);
        assert_eq!(
            summary.totals.total_tokens, 300,
            "token 必须按 breakdown 求和，不能既算 aggregate 又算明细"
        );
        assert_eq!(summary.cost_microunits, Some(3_000));
        assert_eq!(summary.by_model.len(), 2);
    }

    /// 各维度之和必须等于全局，否则说明某个 GROUP BY 漏了行或重复计数。
    #[test]
    fn every_breakdown_sums_to_the_global_totals() {
        let db = db_with(vec![
            draft(
                "key-1",
                "codex",
                "rollout-a",
                "gpt-5.6-sol",
                100,
                Some(1_000),
                Some("2026-09-22T10:00:00Z"),
            ),
            draft(
                "key-2",
                "codex",
                "rollout-b",
                "gpt-5.6-terra",
                200,
                Some(2_000),
                Some("2026-09-21T10:00:00Z"),
            ),
            draft(
                "key-3",
                "claude",
                "session-c",
                "deepseek-v4-pro",
                300,
                Some(3_000),
                Some("2026-08-01T10:00:00Z"),
            ),
        ]);

        let summary = summarize(&db, UsageRange::All);

        assert_eq!(summary.totals.total_tokens, 600);
        for (name, buckets) in [
            ("harness", &summary.by_harness),
            ("model", &summary.by_model),
        ] {
            let tokens: u64 = buckets.iter().map(|bucket| bucket.total_tokens).sum();
            let cost: i64 = buckets
                .iter()
                .filter_map(|bucket| bucket.cost_microunits)
                .sum();
            let events: u64 = buckets.iter().map(|bucket| bucket.event_count).sum();
            assert_eq!(tokens, 600, "{name} 的 token 之和必须等于全局");
            assert_eq!(cost, 6_000, "{name} 的金额之和必须等于全局");
            assert_eq!(events, 3, "{name} 的事件数之和必须等于全局");
        }

        let timeline_tokens: u64 = summary.timeline.iter().map(|b| b.total_tokens).sum();
        let timeline_events: u64 = summary.timeline.iter().map(|b| b.event_count).sum();
        assert_eq!(timeline_tokens, 600, "timeline 的 token 之和必须等于全局");
        assert_eq!(timeline_events, 3);
        assert_eq!(summary.by_harness.len(), 2);
        assert_eq!(summary.by_model.len(), 3);
        assert_eq!(summary.usage_sessions, 3);
    }

    #[test]
    fn missing_pricing_makes_the_cost_a_lower_bound() {
        let db = db_with(vec![draft(
            "key-1",
            "zcode",
            "sess-z",
            "GLM-5.3-Flash",
            500,
            None,
            Some("2026-09-22T10:00:00Z"),
        )]);

        let summary = summarize(&db, UsageRange::All);

        assert_eq!(summary.totals.total_tokens, 500, "token 照样要算");
        assert_eq!(summary.cost_microunits, None, "一条有价格的都没有");
        assert!(summary.cost_is_lower_bound, "缺价格必须标成下界");
        assert_eq!(summary.missing_pricing_records, 1);
        assert_eq!(summary.currency.as_deref(), Some("USD"));
        assert_eq!(summary.by_model[0].cost_microunits, None);
        assert!(summary.by_model[0].cost_is_lower_bound);
    }

    /// 混价：只加已知部分，同时保留下界标记（哪怕已知部分是 0）。
    #[test]
    fn known_and_missing_cost_mix_sums_only_the_known_part() {
        let db = db_with(vec![
            draft(
                "key-1",
                "codex",
                "rollout-a",
                "gpt-5.6-sol",
                100,
                Some(2_500),
                Some("2026-09-22T10:00:00Z"),
            ),
            draft(
                "key-2",
                "zcode",
                "sess-z",
                "GLM-5.3-Flash",
                50,
                None,
                Some("2026-09-22T10:00:00Z"),
            ),
        ]);

        let summary = summarize(&db, UsageRange::All);

        assert_eq!(summary.cost_microunits, Some(2_500), "只加已知部分");
        assert!(summary.cost_is_lower_bound);
        assert_eq!(summary.missing_pricing_records, 1);
        assert_eq!(summary.totals.total_tokens, 150);
    }

    /// 已知价格为 0 且存在缺价格记录时，`cost_microunits` 仍是 `Some(0)`，
    /// 但 `cost_is_lower_bound` 为 true —— 前端必须显示 `≥ $0.00`。
    #[test]
    fn a_known_zero_cost_is_still_a_lower_bound_when_pricing_is_missing() {
        let db = db_with(vec![
            draft(
                "key-1",
                "opencode",
                "ses-free",
                "nemotron-free",
                10,
                Some(0),
                Some("2026-09-22T10:00:00Z"),
            ),
            draft(
                "key-2",
                "zcode",
                "sess-z",
                "GLM-5.3-Flash",
                10,
                None,
                Some("2026-09-22T10:00:00Z"),
            ),
        ]);

        let summary = summarize(&db, UsageRange::All);

        assert_eq!(summary.cost_microunits, Some(0));
        assert!(summary.cost_is_lower_bound, "有缺价格记录就必须是下界");
    }

    /// 导入的历史用量只增加 `usage_sessions`，**不**增加 `managed_sessions`。
    #[test]
    fn imported_history_raises_usage_sessions_but_not_managed_sessions() {
        let db = db_with(vec![
            draft(
                "key-1",
                "codex",
                "rollout-a",
                "gpt-5.6-sol",
                100,
                Some(1_000),
                Some("2026-09-22T10:00:00Z"),
            ),
            draft(
                "key-2",
                "codex",
                "rollout-b",
                "gpt-5.6-sol",
                100,
                Some(1_000),
                Some("2026-09-22T10:00:00Z"),
            ),
        ]);

        let summary = summarize(&db, UsageRange::All);

        assert_eq!(summary.usage_sessions, 2);
        assert_eq!(summary.managed_sessions, 0, "Harness Hub 没启动过任何会话");

        // 真的启动过一条会话之后，两个数字才各自独立地变化。
        db.connection()
            .execute(
                "INSERT INTO sessions
                    (hub_session_id, harness_id, runtime_target_id, source_session_id, status,
                     launch_mode, started_at, created_at, updated_at)
                 VALUES ('hub-1', 'codex', 'local', 'rollout-a', 'exited', 'terminal',
                         '2026-09-22T09:00:00Z', '2026-09-22T09:00:00Z', '2026-09-22T09:00:00Z')",
                [],
            )
            .expect("会话");

        let after = summarize(&db, UsageRange::All);
        assert_eq!(after.usage_sessions, 2, "管理会话不改变外部历史会话数");
        assert_eq!(after.managed_sessions, 1);
    }

    /// 本地日分组：UTC 的 2026-09-22T16:30Z 在 UTC+8 是 09-23。
    #[test]
    fn timeline_groups_by_local_day() {
        let db = db_with(vec![
            draft(
                "key-1",
                "codex",
                "rollout-a",
                "gpt-5.6-sol",
                100,
                Some(1_000),
                Some("2026-09-22T15:30:00Z"),
            ),
            draft(
                "key-2",
                "codex",
                "rollout-b",
                "gpt-5.6-sol",
                200,
                Some(2_000),
                Some("2026-09-22T16:30:00Z"),
            ),
        ]);

        let summary = summarize(&db, UsageRange::All);

        let days: Vec<(&str, u64)> = summary
            .timeline
            .iter()
            .map(|bucket| (bucket.day.as_str(), bucket.total_tokens))
            .collect();
        assert_eq!(
            days,
            vec![("2026-09-22", 100), ("2026-09-23", 200)],
            "16:30Z 在 UTC+8 已经是第二天"
        );
    }

    #[test]
    fn the_range_filter_is_left_closed_right_open() {
        let db = db_with(vec![
            draft(
                "key-start",
                "codex",
                "rollout-a",
                "gpt-5.6-sol",
                10,
                Some(10),
                Some("2026-09-22T16:00:00Z"),
            ),
            draft(
                "key-end",
                "codex",
                "rollout-b",
                "gpt-5.6-sol",
                20,
                Some(20),
                Some("2026-09-23T16:00:00Z"),
            ),
            draft(
                "key-outside",
                "codex",
                "rollout-c",
                "gpt-5.6-sol",
                40,
                Some(40),
                Some("2026-09-23T16:00:01Z"),
            ),
        ]);

        let summary = summarize(&db, UsageRange::Today);

        assert_eq!(
            summary.totals.total_tokens, 10,
            "左闭（16:00:00Z 属于本地今天）右开（次日 16:00:00Z 不属于）"
        );
        assert_eq!(summary.event_count, 1);
    }

    #[test]
    fn timestampless_events_are_counted_and_excluded_from_bounded_ranges() {
        let db = db_with(vec![
            draft(
                "key-1",
                "codex",
                "rollout-a",
                "gpt-5.6-sol",
                100,
                Some(1_000),
                // UTC+8 的「本地今天」= [2026-09-22T16:00Z, 2026-09-23T16:00Z)，
                // 这个时刻必须落在其中，否则测的就不是「排除无时间戳」这件事。
                Some("2026-09-22T20:00:00Z"),
            ),
            draft("key-2", "zcode", "sess-z", "GLM-5.3-Flash", 50, None, None),
        ]);

        let all = summarize(&db, UsageRange::All);
        assert_eq!(all.event_count, 2, "All 必须包含无时间戳的事件");
        assert_eq!(all.timestampless_records, 1);
        assert_eq!(all.excluded_timestampless, 0);

        let today = summarize(&db, UsageRange::Today);
        assert_eq!(today.event_count, 1, "有界窗口排除无法归日的事件");
        assert_eq!(today.timestampless_records, 0);
        assert_eq!(
            today.excluded_timestampless, 1,
            "被排除的必须被计数，否则「All > 各窗口之和」无法解释"
        );
    }

    #[test]
    fn the_range_filter_uses_occurred_at_not_imported_at() {
        let db = crate::test_support::empty_db();
        // 用旧导入时间写入一条 2026 年 1 月的事件：按 occurred_at 应落在 30 天窗口之外。
        let normalized = NormalizedUsage {
            events: vec![draft(
                "key-old",
                "codex",
                "rollout-old",
                "gpt-5.6-sol",
                100,
                Some(1_000),
                Some("2026-01-01T00:00:00Z"),
            )],
            totals: None,
            sessions_seen: 0,
            timestampless: 0,
            row_total_residual: 0,
        };
        UsageImporter::new(db.connection())
            .import(&ImportRequest {
                source: "ccusage",
                source_version: None,
                runner: None,
                report_kind: "session",
                events: &normalized.events,
                report_totals: None,
                row_total_residual: 0,
                started_at: "2026-09-22T10:00:00Z",
            })
            .expect("导入");

        let month = summarize(&db, UsageRange::Days30);

        assert_eq!(
            month.event_count, 0,
            "按 occurred_at 过滤：1 月的事件不在 30 天窗口里"
        );
        assert_eq!(summarize(&db, UsageRange::All).event_count, 1);
    }

    #[test]
    fn project_breakdown_stays_empty_because_v01_never_infers_a_project() {
        let db = db_with(vec![draft(
            "key-1",
            "codex",
            "rollout-a",
            "gpt-5.6-sol",
            100,
            Some(1_000),
            Some("2026-09-22T10:00:00Z"),
        )]);

        let summary = summarize(&db, UsageRange::All);

        assert!(
            summary.by_project.is_empty(),
            "不得为了让界面好看而伪造项目归属"
        );
    }

    /// 汇总自带 totals 时必须与明细一致（Dashboard 只信明细）。
    #[test]
    fn the_summary_ignores_report_totals_and_recomputes_from_detail() {
        let db = crate::test_support::empty_db();
        let normalized = NormalizedUsage {
            events: vec![draft(
                "key-1",
                "codex",
                "rollout-a",
                "gpt-5.6-sol",
                100,
                Some(1_000),
                Some("2026-09-22T10:00:00Z"),
            )],
            totals: Some(ReportTotals {
                total_tokens: 999_999,
                cost_microunits: Some(999_999),
                ..ReportTotals::default()
            }),
            sessions_seen: 0,
            timestampless: 0,
            row_total_residual: 0,
        };
        UsageImporter::new(db.connection())
            .import(&ImportRequest {
                source: "ccusage",
                source_version: None,
                runner: None,
                report_kind: "session",
                events: &normalized.events,
                report_totals: normalized.totals,
                row_total_residual: 0,
                started_at: "2026-09-22T10:00:00Z",
            })
            .expect("导入");

        let summary = summarize(&db, UsageRange::All);

        assert_eq!(
            summary.totals.total_tokens, 100,
            "必须从明细重算，不能采用来源给的总计"
        );
        assert_eq!(summary.cost_microunits, Some(1_000));
    }

    /// 跨 IPC 契约测试：Dashboard 依赖这些键名（前端 `normalizeUsageSummary` 同形）。
    #[test]
    fn usage_summary_serializes_with_camel_case_keys() {
        let db = db_with(vec![
            draft(
                "key-1",
                "codex",
                "rollout-a",
                "gpt-5.6-sol",
                100,
                Some(1_000),
                Some("2026-09-22T20:00:00Z"),
            ),
            draft(
                "key-2",
                "zcode",
                "sess-z",
                "GLM-5.3-Flash",
                50,
                None,
                Some("2026-09-22T20:05:00Z"),
            ),
        ]);

        let json = serde_json::to_value(summarize(&db, UsageRange::All)).expect("序列化");

        assert_eq!(json["range"]["kind"], "all");
        assert_eq!(json["range"]["timezoneOffsetMinutes"], 480);
        assert_eq!(json["range"]["startUtc"], serde_json::Value::Null);
        assert_eq!(json["range"]["nowUtc"], NOW);
        assert_eq!(json["totals"]["inputTokens"], 150);
        assert_eq!(json["totals"]["totalTokens"], 150);
        assert_eq!(json["totals"]["reasoningTokens"], 0);
        assert_eq!(json["costMicrounits"], 1_000);
        assert_eq!(json["currency"], "USD");
        assert_eq!(json["costIsLowerBound"], true);
        assert_eq!(json["missingPricingRecords"], 1);
        assert_eq!(json["timestamplessRecords"], 0);
        assert_eq!(json["excludedTimestampless"], 0);
        assert_eq!(json["eventCount"], 2);
        assert_eq!(json["usageSessions"], 2);
        assert_eq!(json["managedSessions"], 0);
        assert_eq!(json["byHarness"][0]["key"], "codex");
        assert_eq!(json["byHarness"][0]["costIsLowerBound"], false);
        assert_eq!(json["byModel"].as_array().map(Vec::len), Some(2));
        assert_eq!(json["byProject"].as_array().map(Vec::len), Some(0));
        assert_eq!(json["timeline"][0]["day"], "2026-09-23");
        assert!(json.get("usage_sessions").is_none(), "不得两套契约并存");
    }
}
