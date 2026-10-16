//! Activity timeline 的**唯一**查询入口。
//!
//! 纪律：
//! - 签名只拿 `&Connection`：不起进程、不写库（ADR-0012）。
//! - 日期算术全部交给 SQLite（`datetime` / `strftime`），与 `usage/summary.rs` 同口径。
//! - **时区修饰符固定在 `?1`，范围参数从 `?2` 开始** —— 这是 `summary.rs:294-312`
//!   记录过的坑：`?N` 按出现顺序编号，把范围参数放在 `?1` 会让修饰符顺延错位。
//! - `usage` 与 `session` 分开取、分开计数，永不合并（ADR-0012 决策三）。
//! - 本地日 / 本地小时一律用 `strftime(..., occurred_at, <modifier>)` 算，不自己解析时间。

use rusqlite::{params_from_iter, Connection};

use crate::error::Result;
use crate::usage::summary::UsageRangeWindow;

use super::{
    ActivityDayBucket, ActivityEvent, ActivityEventKind, ActivityHourBucket, ActivityTimeline,
};

/// 事件表的范围过滤条件（占位符从 `?2` 起，`?1` 留给时区修饰符）。
///
/// 与 `usage/summary.rs:298-312` 同形：用 `datetime()` 比较，因为库里两种时间格式并存
/// （带毫秒与不带毫秒），直接字符串比较会在边界上判错。
fn range_filter(window: &UsageRangeWindow) -> (String, Vec<String>) {
    match (&window.start_utc, &window.end_utc) {
        (Some(start), Some(end)) => (
            // 注意：外层已经固定 `occurred_at IS NOT NULL`，这里只能接 `AND`。
            "AND datetime(occurred_at) >= datetime(?2) AND datetime(occurred_at) < datetime(?3)"
                .to_string(),
            vec![start.clone(), end.clone()],
        ),
        _ => (String::new(), Vec::new()),
    }
}

/// 会话表的范围过滤条件（按 `started_at`，与 `summary.rs:315-322` 同口径）。
fn session_range_filter(window: &UsageRangeWindow) -> String {
    if window.is_bounded() {
        "WHERE datetime(started_at) >= datetime(?2) AND datetime(started_at) < datetime(?3)"
            .to_string()
    } else {
        String::new()
    }
}

/// `?1` = 时区修饰符，后面跟范围参数（有界窗口才有两个）。
fn bind(window: &UsageRangeWindow, range_params: &[String]) -> Vec<String> {
    let mut all = vec![window.modifier_public()];
    all.extend(range_params.iter().cloned());
    all
}

/// usage 明细（`occurred_at` 落在窗口内；没有发生时间的事件不进场，也不被静默丢弃计数）。
///
/// `project_id` 来自 `hub_session_id → sessions.project_id` 的**可证明**反查：join 不上就
/// 留 NULL，不按 `source_session_id` 猜（`usage/importer.rs` 的同一原则）。
const SELECT_USAGE_EVENTS: &str = "
    SELECT strftime('%Y-%m-%d', usage_events.occurred_at, ?1) AS day,
           CAST(strftime('%H', usage_events.occurred_at, ?1) AS INTEGER) AS hour,
           usage_events.occurred_at,
           usage_events.harness,
           usage_events.hub_session_id,
           sessions.project_id,
           usage_events.source_session_id,
           usage_events.model,
           usage_events.total_tokens,
           usage_events.cost_microunits,
           usage_events.cost_source
      FROM usage_events
      LEFT JOIN sessions ON sessions.hub_session_id = usage_events.hub_session_id
     WHERE usage_events.occurred_at IS NOT NULL {range}
";

/// Hub 自己管理的会话（`started_at` 落在窗口内）—— managed sessions。
const SELECT_SESSION_EVENTS: &str = "
    SELECT strftime('%Y-%m-%d', sessions.started_at, ?1) AS day,
           CAST(strftime('%H', sessions.started_at, ?1) AS INTEGER) AS hour,
           sessions.started_at,
           sessions.harness_id,
           sessions.hub_session_id,
           sessions.project_id,
           sessions.status,
           sessions.launch_mode,
           sessions.cwd,
           CASE
             WHEN sessions.ended_at IS NULL THEN NULL
             WHEN strftime('%s', sessions.ended_at) IS NULL THEN NULL
             WHEN strftime('%s', sessions.started_at) IS NULL THEN NULL
             ELSE MAX(0, CAST((strftime('%s', sessions.ended_at) - strftime('%s', sessions.started_at)) AS INTEGER) * 1000)
           END AS duration_ms
      FROM sessions
     {range}
";

/// 按**本地日**聚合。`cost` / `unpriced` 只对 usage 行有意义（session 行恒 0 / NULL）。
const SELECT_DAY_BUCKETS: &str = "
    SELECT day,
           SUM(CASE WHEN kind = 'usage' THEN 1 ELSE 0 END) AS usage_events,
           SUM(CASE WHEN kind = 'session' THEN 1 ELSE 0 END) AS sessions,
           SUM(tokens) AS tokens,
           SUM(cost) AS cost,
           SUM(unpriced) AS unpriced
      FROM (
            SELECT strftime('%Y-%m-%d', usage_events.occurred_at, ?1) AS day,
                   'usage' AS kind,
                   usage_events.total_tokens AS tokens,
                   usage_events.cost_microunits AS cost,
                   CASE WHEN usage_events.cost_microunits IS NULL
                         OR usage_events.cost_source IN ('ccusage_missing_pricing', 'estimated')
                        THEN 1 ELSE 0 END AS unpriced
              FROM usage_events
             WHERE usage_events.occurred_at IS NOT NULL {usage_range}
            UNION ALL
            SELECT strftime('%Y-%m-%d', sessions.started_at, ?1) AS day,
                   'session' AS kind,
                   0 AS tokens,
                   NULL AS cost,
                   0 AS unpriced
              FROM sessions
             {session_range}
           )
     WHERE day IS NOT NULL
     GROUP BY day
     ORDER BY day ASC
";

/// 按**本地小时**聚合（0..=23）。
const SELECT_HOUR_BUCKETS: &str = "
    SELECT hour,
           SUM(CASE WHEN kind = 'usage' THEN 1 ELSE 0 END) AS usage_events,
           SUM(CASE WHEN kind = 'session' THEN 1 ELSE 0 END) AS sessions,
           SUM(tokens) AS tokens
      FROM (
            SELECT CAST(strftime('%H', usage_events.occurred_at, ?1) AS INTEGER) AS hour,
                   'usage' AS kind,
                   usage_events.total_tokens AS tokens
              FROM usage_events
             WHERE usage_events.occurred_at IS NOT NULL {usage_range}
            UNION ALL
            SELECT CAST(strftime('%H', sessions.started_at, ?1) AS INTEGER) AS hour,
                   'session' AS kind,
                   0 AS tokens
              FROM sessions
             {session_range}
           )
     WHERE hour IS NOT NULL
     GROUP BY hour
     ORDER BY hour ASC
";

/// 把窗口折叠成活动流 + 本地日 / 本地小时聚合。
///
/// `limit` 只约束 `events`（流要分页 / 截断），不约束 `days` / `hours`（聚合是全集）。
pub fn timeline(
    connection: &Connection,
    window: &UsageRangeWindow,
    limit: u32,
) -> Result<ActivityTimeline> {
    let (usage_range, range_params) = range_filter(window);
    let session_range = session_range_filter(window);
    let usage_params = bind(window, &range_params);
    let session_params = bind(window, &range_params);

    let mut events = Vec::new();

    let usage_sql = SELECT_USAGE_EVENTS.replace("{range}", &usage_range);
    let mut statement = connection.prepare(&usage_sql)?;
    let rows = statement.query_map(params_from_iter(usage_params.iter()), |row| {
        let cost_microunits: Option<i64> = row.get(9)?;
        let cost_source: Option<String> = row.get(10)?;
        Ok(ActivityEvent {
            kind: ActivityEventKind::Usage,
            day: row.get(0)?,
            hour: row.get::<_, i64>(1)?.clamp(0, 23) as u8,
            occurred_at: row.get(2)?,
            harness_id: row.get(3)?,
            hub_session_id: row.get(4)?,
            project_id: row.get(5)?,
            source_session_id: row.get(6)?,
            model: row.get(7)?,
            total_tokens: row.get::<_, i64>(8)?.max(0) as u64,
            cost_microunits,
            // `≥` 由后端判定，前端只格式化（ADR-0012 决策四）。
            cost_is_lower_bound: cost_microunits.is_none()
                || matches!(
                    cost_source.as_deref(),
                    Some("ccusage_missing_pricing") | Some("estimated")
                ),
            status: None,
            launch_mode: None,
            duration_ms: None,
            cwd: None,
        })
    })?;
    for event in rows {
        events.push(event?);
    }

    let session_sql = SELECT_SESSION_EVENTS.replace("{range}", &session_range);
    let mut statement = connection.prepare(&session_sql)?;
    let rows = statement.query_map(params_from_iter(session_params.iter()), |row| {
        let duration_ms: Option<i64> = row.get(9)?;
        Ok(ActivityEvent {
            kind: ActivityEventKind::Session,
            day: row.get(0)?,
            hour: row.get::<_, i64>(1)?.clamp(0, 23) as u8,
            occurred_at: row.get(2)?,
            harness_id: row.get(3)?,
            hub_session_id: Some(row.get(4)?),
            project_id: row.get(5)?,
            source_session_id: None,
            model: None,
            total_tokens: 0,
            cost_microunits: None,
            cost_is_lower_bound: false,
            status: row.get(6)?,
            launch_mode: row.get(7)?,
            duration_ms,
            cwd: row.get(8)?,
        })
    })?;
    for event in rows {
        events.push(event?);
    }

    // 最新在前；同一时刻用 kind 与 id 做稳定次序，避免翻页时抖动。
    events.sort_by(|left, right| {
        right
            .occurred_at
            .cmp(&left.occurred_at)
            .then_with(|| right.kind.as_str().cmp(left.kind.as_str()))
            .then_with(|| right.hub_session_id.cmp(&left.hub_session_id))
    });

    let usage_event_count = events
        .iter()
        .filter(|event| event.kind == ActivityEventKind::Usage)
        .count() as u64;
    let managed_session_count = events
        .iter()
        .filter(|event| event.kind == ActivityEventKind::Session)
        .count() as u64;

    let day_sql = SELECT_DAY_BUCKETS
        .replace("{usage_range}", &usage_range)
        .replace("{session_range}", &session_range);
    let mut days = Vec::new();
    {
        let mut statement = connection.prepare(&day_sql)?;
        let rows = statement.query_map(params_from_iter(session_params.iter()), |row| {
            let unpriced = row.get::<_, i64>(5)?.max(0);
            Ok(ActivityDayBucket {
                day: row.get(0)?,
                usage_event_count: row.get::<_, i64>(1)?.max(0) as u64,
                managed_session_count: row.get::<_, i64>(2)?.max(0) as u64,
                total_tokens: row.get::<_, i64>(3)?.max(0) as u64,
                cost_microunits: row.get(4)?,
                cost_is_lower_bound: unpriced > 0,
            })
        })?;
        for bucket in rows {
            days.push(bucket?);
        }
    }
    let active_days = days.len() as u64;
    let max_day_tokens = days
        .iter()
        .map(|bucket| bucket.total_tokens)
        .max()
        .unwrap_or(0);
    let total_tokens: u64 = days.iter().map(|bucket| bucket.total_tokens).sum();

    let hour_sql = SELECT_HOUR_BUCKETS
        .replace("{usage_range}", &usage_range)
        .replace("{session_range}", &session_range);
    let mut populated = Vec::new();
    {
        let mut statement = connection.prepare(&hour_sql)?;
        let rows = statement.query_map(params_from_iter(session_params.iter()), |row| {
            Ok(ActivityHourBucket {
                hour: row.get::<_, i64>(0)?.clamp(0, 23) as u8,
                usage_event_count: row.get::<_, i64>(1)?.max(0) as u64,
                managed_session_count: row.get::<_, i64>(2)?.max(0) as u64,
                total_tokens: row.get::<_, i64>(3)?.max(0) as u64,
            })
        })?;
        for bucket in rows {
            populated.push(bucket?);
        }
    }
    // 补齐没有事件的小时：前端画 24 格热力条不需要自己补零。
    let mut hours: Vec<ActivityHourBucket> = (0..24)
        .map(|hour| ActivityHourBucket {
            hour,
            usage_event_count: 0,
            managed_session_count: 0,
            total_tokens: 0,
        })
        .collect();
    for bucket in populated {
        let index = bucket.hour as usize;
        hours[index] = bucket;
    }
    let max_hour_tokens = hours
        .iter()
        .map(|bucket| bucket.total_tokens)
        .max()
        .unwrap_or(0);

    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let has_more = events.len() > limit;
    events.truncate(limit);

    Ok(ActivityTimeline {
        range: window.clone(),
        events,
        days,
        hours,
        usage_event_count,
        managed_session_count,
        active_days,
        total_tokens,
        max_day_tokens,
        max_hour_tokens,
        has_more,
    })
}
