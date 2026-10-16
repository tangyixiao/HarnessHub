//! `activity` 模块的单测。
//!
//! 纪律：
//! - 前置状态一律由**生产代码**填充（`UsageImporter` / `SessionStore` / `ProjectStore`），
//!   不使用 `seeded_db()` 之类的夹具造假 FK 前置（AGENTS.md「测试纪律」）。
//! - 窗口常量沿用 `usage/summary.rs` 的习惯：`NOW = 2026-09-22T23:30:00Z`、
//!   `UTC_PLUS_8 = 480`（本地 = 2026-09-23 07:30），用来锁「本地日边界」。

use super::*;
use crate::db::Database;
use crate::usage::importer::{request_from, UsageImporter};
use crate::usage::summary::{parse_range, range_bounds, UsageRange};
use crate::usage::{NormalizedUsage, UsageEventDraft};

use crate::project::store::ProjectStore;
use crate::session::store::{NewSession, SessionStore};
use crate::session::TerminationReason;

const NOW: &str = "2026-09-22T23:30:00Z";
const UTC_PLUS_8: i32 = 480;

/// 与 `usage/summary.rs` 的 `draft()` 同形，便于对齐两边口径。
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

/// 冷启动建库（生产引导路径），再用**生产导入路径**灌 usage 明细。
fn db_with_usage(events: Vec<UsageEventDraft>) -> Database {
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

fn timeline(db: &Database, range: UsageRange) -> ActivityTimeline {
    let window = range_bounds(db.connection(), range, NOW, UTC_PLUS_8).expect("窗口");
    super::query::timeline(db.connection(), &window, 100).expect("时间线")
}

fn runtime_target_id() -> String {
    crate::runtime::local::LOCAL_TARGET_ID.to_string()
}

// ---- Step 1：窗口与本地日 ---------------------------------------------

/// 本地 23:30 的事件必须落进**本地**第二天，而不是 UTC 当天。
#[test]
fn local_day_boundary_uses_the_viewer_offset() {
    let db = db_with_usage(vec![draft(
        "k1",
        "codex",
        "s1",
        "gpt-5.6-sol",
        120,
        Some(1_000),
        Some("2026-09-22T23:30:00Z"),
    )]);

    let timeline = timeline(&db, UsageRange::Today);
    let event = timeline.events.first().expect("有一条事件");
    assert_eq!(event.day, "2026-09-23", "UTC+8 下应落到本地 09-23");
    assert_eq!(event.hour, 7, "本地 07:30");
    assert_eq!(event.kind, ActivityEventKind::Usage);
    assert_eq!(timeline.days.len(), 1, "today 窗口只含一个本地日");
    assert_eq!(timeline.days[0].day, "2026-09-23");
}

/// `all` 窗口把 `start_utc` / `end_utc` 留空，事件全部返回。
#[test]
fn all_range_has_no_bounds_and_keeps_every_event() {
    let db = db_with_usage(vec![
        draft(
            "k1",
            "codex",
            "s1",
            "m",
            10,
            None,
            Some("2020-01-01T00:00:00Z"),
        ),
        draft("k2", "codex", "s1", "m", 20, None, Some(NOW)),
    ]);

    let timeline = timeline(&db, UsageRange::All);
    assert!(!timeline.range.is_bounded(), "all 不设边界");
    assert_eq!(timeline.range.timezone_offset_minutes, UTC_PLUS_8);
    assert_eq!(timeline.events.len(), 2);
    assert_eq!(timeline.total_tokens, 30);
    assert_eq!(timeline.days.len(), 2, "两个不同的本地日");
    assert_eq!(timeline.days[0].day, "2020-01-01", "按本地日升序");
}

/// `7d` 窗口只收窗口内的事件，窗外的事件不得泄漏。
#[test]
fn bounded_window_excludes_events_outside_the_range() {
    let db = db_with_usage(vec![
        draft(
            "in",
            "codex",
            "s1",
            "m",
            10,
            None,
            Some("2026-09-20T01:00:00Z"),
        ),
        draft(
            "out",
            "codex",
            "s1",
            "m",
            99,
            None,
            Some("2026-09-01T01:00:00Z"),
        ),
    ]);

    let timeline = timeline(&db, UsageRange::Days7);
    assert!(timeline.range.is_bounded());
    assert_eq!(timeline.usage_event_count, 1, "窗外事件不计入");
    assert_eq!(timeline.events.len(), 1);
    assert_eq!(timeline.events[0].total_tokens, 10);
}

// ---- Step 2：事件流 ---------------------------------------------------

/// usage 与 session 两条 SQL 汇成一条流，且按时间倒序。
#[test]
fn usage_and_session_events_share_one_reverse_chronological_stream() {
    let mut db = db_with_usage(vec![draft(
        "k1",
        "codex",
        "s1",
        "gpt-5.6-sol",
        120,
        Some(1_000),
        Some("2026-09-22T10:00:00Z"),
    )]);
    SessionStore::new(db.connection_mut())
        .insert(
            &NewSession::new(
                "hub-1",
                "codex",
                runtime_target_id(),
                "2026-09-22T20:00:00Z",
            )
            .with_source_session_id("s1")
            .with_cwd("D:/HarnessHub"),
        )
        .expect("插入会话");

    let timeline = timeline(&db, UsageRange::All);
    assert_eq!(timeline.events.len(), 2);
    assert_eq!(
        timeline.events[0].kind,
        ActivityEventKind::Session,
        "20:00 的 session 应排在 10:00 的 usage 之前"
    );
    assert_eq!(timeline.events[1].kind, ActivityEventKind::Usage);
    assert_eq!(timeline.usage_event_count, 1);
    assert_eq!(timeline.managed_session_count, 1, "两者永不合并");
}

/// `duration_ms`：未结束为 `None`，已结束为精确毫秒；`ended_at` 早于 `started_at` 不得出现负值。
#[test]
fn session_duration_is_absent_until_the_session_finishes() {
    let mut db = crate::test_support::cold_start_db();
    let store = SessionStore::new(db.connection_mut());
    store
        .insert(
            &NewSession::new(
                "hub-running",
                "codex",
                runtime_target_id(),
                "2026-09-22T10:00:00Z",
            )
            .with_cwd("D:/HarnessHub"),
        )
        .expect("插入 running 会话");
    store
        .insert(
            &NewSession::new(
                "hub-finished",
                "codex",
                runtime_target_id(),
                "2026-09-22T10:00:00Z",
            )
            .with_cwd("D:/HarnessHub"),
        )
        .expect("插入已结束会话");

    // running：没有 ended_at。
    store
        .mark_running("hub-running", Some(4242), "2026-09-22T10:00:01Z")
        .expect("标记 running");
    // finished：先 mark_running 再 finish（finish 只接受 running）。
    store
        .mark_running("hub-finished", Some(4243), "2026-09-22T10:00:01Z")
        .expect("标记 running");
    store
        .finish(
            "hub-finished",
            Some(0),
            TerminationReason::NaturalExit,
            "2026-09-22T10:00:30Z",
        )
        .expect("结束会话");

    let timeline = timeline(&db, UsageRange::All);
    let finished = timeline
        .events
        .iter()
        .find(|event| event.hub_session_id.as_deref() == Some("hub-finished"))
        .expect("已结束会话");
    let running = timeline
        .events
        .iter()
        .find(|event| event.hub_session_id.as_deref() == Some("hub-running"))
        .expect("running 会话");

    assert_eq!(finished.duration_ms, Some(30_000), "10:00:00 → 10:00:30");
    assert_eq!(finished.status.as_deref(), Some("exited"));
    assert_eq!(finished.launch_mode.as_deref(), Some("terminal"));
    assert_eq!(finished.cwd.as_deref(), Some("D:/HarnessHub"));
    assert_eq!(running.duration_ms, None, "未结束的会话不得编造时长");
    assert_eq!(running.status.as_deref(), Some("running"));
    assert_eq!(running.total_tokens, 0, "session 事件不带 token");
    assert_eq!(running.cost_microunits, None);
}

/// `project_id` 只能来自 `sessions.project_id` 反查，不按 `source_session_id` 猜。
#[test]
fn usage_project_comes_only_from_the_joined_session_row() {
    let mut db = db_with_usage(vec![
        draft(
            "linked",
            "codex",
            "s-linked",
            "m",
            10,
            None,
            Some("2026-09-22T10:00:00Z"),
        ),
        draft(
            "unlinked",
            "codex",
            "s-unlinked",
            "m",
            20,
            None,
            Some("2026-09-22T11:00:00Z"),
        ),
    ]);

    // 用生产注册路径建项目（ProjectStore），再用生产 SessionStore 关联它。
    let project_id = {
        let store = ProjectStore::new(db.connection_mut());
        store
            .insert(
                &uuid::Uuid::new_v4().to_string(),
                "harness-hub",
                "D:/HarnessHub",
                "2026-01-01T00:00:00Z",
            )
            .expect("插入项目");
        store
            .list_recent(10)
            .expect("列出项目")
            .first()
            .expect("有一个项目")
            .id
            .clone()
    };

    SessionStore::new(db.connection_mut())
        .insert(
            &NewSession::new(
                "hub-1",
                "codex",
                runtime_target_id(),
                "2026-09-22T10:00:00Z",
            )
            .with_source_session_id("s-linked")
            .with_project_id(project_id.clone()),
        )
        .expect("插入会话");

    // importer 只在**导入时**能证明匹配才回填 hub_session_id；这里事件先落库、
    // 会话后插入，因此 hub_session_id 仍是 NULL —— 归属自然拿不到，也不该被猜出来。
    let timeline = timeline(&db, UsageRange::All);
    let linked = timeline
        .events
        .iter()
        .find(|event| event.source_session_id.as_deref() == Some("s-linked"))
        .expect("可归属的事件");
    let unlinked = timeline
        .events
        .iter()
        .find(|event| event.source_session_id.as_deref() == Some("s-unlinked"))
        .expect("不可归属的事件");

    assert_eq!(
        linked.project_id, None,
        "hub_session_id 都没匹配上，不得反推项目归属"
    );
    assert_eq!(unlinked.project_id, None);
    assert_eq!(linked.hub_session_id, None);
    assert_eq!(unlinked.hub_session_id, None);
}

/// 会话**先于**导入存在时，importer 会回填 `hub_session_id`，项目归属随之而来。
#[test]
fn usage_project_is_filled_when_the_session_predates_the_import() {
    let mut db = crate::test_support::cold_start_db();
    let project_id = {
        let store = ProjectStore::new(db.connection_mut());
        store
            .insert(
                &uuid::Uuid::new_v4().to_string(),
                "harness-hub",
                "D:/HarnessHub",
                "2026-01-01T00:00:00Z",
            )
            .expect("插入项目");
        store
            .list_recent(10)
            .expect("列出项目")
            .first()
            .expect("有一个项目")
            .id
            .clone()
    };
    SessionStore::new(db.connection_mut())
        .insert(
            &NewSession::new(
                "hub-1",
                "codex",
                runtime_target_id(),
                "2026-09-22T10:00:00Z",
            )
            .with_source_session_id("s-linked")
            .with_project_id(project_id.clone()),
        )
        .expect("插入会话");

    // 现在才导入：`(harness_id, source_session_id)` 唯一命中，importer 回填 hub_session_id。
    let normalized = NormalizedUsage {
        events: vec![draft(
            "linked",
            "codex",
            "s-linked",
            "m",
            10,
            None,
            Some("2026-09-22T10:30:00Z"),
        )],
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

    let timeline = timeline(&db, UsageRange::All);
    let linked = timeline
        .events
        .iter()
        .find(|event| event.source_session_id.as_deref() == Some("s-linked"))
        .expect("可归属的事件");

    assert_eq!(linked.hub_session_id.as_deref(), Some("hub-1"));
    assert_eq!(
        linked.project_id.as_deref(),
        Some(project_id.as_str()),
        "项目归属只能来自 sessions.project_id 反查"
    );
}

/// 缺价格的事件：`cost_is_lower_bound` 由后端判定，前端只格式化（ADR-0012 决策四）。
#[test]
fn missing_pricing_makes_the_cost_a_lower_bound() {
    let db = db_with_usage(vec![
        draft("priced", "codex", "s1", "m", 10, Some(1_000), Some(NOW)),
        draft("unpriced", "codex", "s1", "m", 20, None, Some(NOW)),
    ]);

    let timeline = timeline(&db, UsageRange::Today);
    let priced = timeline
        .events
        .iter()
        .find(|event| event.source_session_id.as_deref() == Some("s1") && event.total_tokens == 10)
        .expect("有价格事件");
    let unpriced = timeline
        .events
        .iter()
        .find(|event| event.source_session_id.as_deref() == Some("s1") && event.total_tokens == 20)
        .expect("缺价格事件");

    assert_eq!(priced.cost_microunits, Some(1_000));
    assert!(!priced.cost_is_lower_bound);
    assert_eq!(unpriced.cost_microunits, None);
    assert!(
        unpriced.cost_is_lower_bound,
        "缺价格必须由后端标成下界，而不是前端猜"
    );
    assert!(timeline.days[0].cost_is_lower_bound, "按天聚合同样标下界");
}

// ---- Step 3：聚合 -----------------------------------------------------

/// `hours` 恒为 24 项，且按 hour 升序。
#[test]
fn hours_always_cover_the_whole_day() {
    let db = db_with_usage(vec![draft(
        "k1",
        "codex",
        "s1",
        "m",
        10,
        None,
        Some("2026-09-22T23:30:00Z"),
    )]);

    let timeline = timeline(&db, UsageRange::Today);
    assert_eq!(timeline.hours.len(), 24, "前端画 24 格不需要补零");
    for (index, bucket) in timeline.hours.iter().enumerate() {
        assert_eq!(bucket.hour as usize, index);
    }
    let populated = timeline
        .hours
        .iter()
        .find(|bucket| bucket.usage_event_count == 1)
        .expect("本地 07 点有事件");
    assert_eq!(populated.hour, 7);
    assert_eq!(populated.total_tokens, 10);
    assert_eq!(populated.managed_session_count, 0);
}

/// `days` 与 `hours` 的 token 合计必须等于 `total_tokens`（自洽，不是两套口径）。
#[test]
fn day_and_hour_buckets_sum_to_the_same_total() {
    let db = db_with_usage(vec![
        draft(
            "k1",
            "codex",
            "s1",
            "m",
            10,
            None,
            Some("2026-09-22T23:30:00Z"),
        ),
        draft(
            "k2",
            "codex",
            "s1",
            "m",
            32,
            None,
            Some("2026-09-22T02:00:00Z"),
        ),
    ]);

    let timeline = timeline(&db, UsageRange::All);
    let day_sum: u64 = timeline.days.iter().map(|bucket| bucket.total_tokens).sum();
    let hour_sum: u64 = timeline
        .hours
        .iter()
        .map(|bucket| bucket.total_tokens)
        .sum();
    assert_eq!(day_sum, timeline.total_tokens);
    assert_eq!(hour_sum, timeline.total_tokens);
    assert_eq!(timeline.total_tokens, 42);
    assert_eq!(timeline.max_day_tokens, 32);
    assert_eq!(timeline.active_days, 2, "两个有事件的本地日");
}

/// 没有发生时间的 usage 事件不得悄悄消失：既不进流，也不进聚合。
#[test]
fn timestampless_events_stay_out_of_the_stream() {
    let db = db_with_usage(vec![draft(
        "timestampless",
        "codex",
        "s1",
        "m",
        10,
        None,
        None,
    )]);

    let timeline = timeline(&db, UsageRange::Today);
    assert!(timeline.events.is_empty(), "无时间戳的事件不能排进时间线");
    assert!(timeline.days.is_empty());
    assert_eq!(timeline.usage_event_count, 0);
    assert_eq!(timeline.total_tokens, 0);
}

// ---- Step 4：limit / has_more -----------------------------------------

/// 事件数超过 limit 时 `has_more = true`，且 `events.len() <= limit`。
#[test]
fn has_more_flags_truncation_without_lying_about_completeness() {
    let db = db_with_usage(vec![
        draft(
            "k1",
            "codex",
            "s1",
            "m",
            10,
            None,
            Some("2026-09-22T10:00:00Z"),
        ),
        draft(
            "k2",
            "codex",
            "s1",
            "m",
            10,
            None,
            Some("2026-09-22T11:00:00Z"),
        ),
        draft(
            "k3",
            "codex",
            "s1",
            "m",
            10,
            None,
            Some("2026-09-22T12:00:00Z"),
        ),
    ]);

    let window = range_bounds(db.connection(), UsageRange::All, NOW, UTC_PLUS_8).expect("窗口");
    let timeline = super::query::timeline(db.connection(), &window, 2).expect("时间线");

    assert_eq!(timeline.events.len(), 2, "不得超过 limit");
    assert!(timeline.has_more, "被截断必须显式告知前端");
    assert_eq!(
        timeline.events[0].occurred_at, "2026-09-22T12:00:00Z",
        "截断后仍是最新在前"
    );
}

/// 未截断时 `has_more = false`。
#[test]
fn has_more_is_false_when_nothing_was_dropped() {
    let db = db_with_usage(vec![draft(
        "k1",
        "codex",
        "s1",
        "m",
        10,
        None,
        Some("2026-09-22T23:30:00Z"),
    )]);

    let timeline = timeline(&db, UsageRange::Today);
    assert!(!timeline.has_more);
    assert_eq!(timeline.events.len(), 1);
}

/// 空库：一切为 0 / 空，`days` 与 `hours` 仍是合法的空壳（hours 恒 24 项）。
#[test]
fn empty_database_yields_a_well_formed_empty_timeline() {
    let db = crate::test_support::cold_start_db();
    let timeline = timeline(&db, UsageRange::Today);

    assert!(timeline.events.is_empty());
    assert!(timeline.days.is_empty());
    assert_eq!(timeline.hours.len(), 24);
    assert_eq!(timeline.usage_event_count, 0);
    assert_eq!(timeline.managed_session_count, 0);
    assert_eq!(timeline.total_tokens, 0);
    assert_eq!(timeline.max_day_tokens, 0);
    assert_eq!(timeline.max_hour_tokens, 0);
    assert!(!timeline.has_more);
}

/// 冷启动纪律：这条用例从**真正空的库**开始，前置状态全部由生产代码创建。
#[test]
fn cold_start_populates_everything_through_production_code() {
    // 不是 seeded_db()：这里是 open_in_memory()，harness / runtime target 行
    // 由 reconcile_harnesses + ensure_local_target 这两条生产引导路径创建。
    let db = db_with_usage(vec![draft(
        "k1",
        "codex",
        "s1",
        "m",
        7,
        None,
        Some("2026-09-22T23:30:00Z"),
    )]);
    // 上面已经覆盖「空库 → 导入 → 查询」的完整生产链路；这里只断言它真的跑通了。
    let timeline = timeline(&db, UsageRange::Today);
    assert_eq!(timeline.usage_event_count, 1);
    assert_eq!(timeline.total_tokens, 7);
    assert_eq!(timeline.hours.len(), 24);
}

// ---- Step 5：契约 -----------------------------------------------------

/// 跨 IPC 契约：顶层键名与嵌套形状被锁死，且没有 snake_case 孪生键。
#[test]
fn activity_timeline_serializes_with_camel_case_keys() {
    let mut db = db_with_usage(vec![draft(
        "k1",
        "codex",
        "s1",
        "m",
        10,
        Some(1_000),
        Some("2026-09-22T23:30:00Z"),
    )]);
    SessionStore::new(db.connection_mut())
        .insert(
            &NewSession::new(
                "hub-1",
                "codex",
                runtime_target_id(),
                "2026-09-22T23:00:00Z",
            )
            .with_cwd("D:/HarnessHub"),
        )
        .expect("插入会话");

    let timeline = timeline(&db, UsageRange::Today);
    let json = serde_json::to_value(&timeline).expect("序列化");
    let object = json.as_object().expect("顶层是对象");

    assert_eq!(object.len(), ActivityTimeline::keys().len());
    for key in ActivityTimeline::keys() {
        assert!(object.contains_key(*key), "缺少键 {key}");
    }
    // 只检查 activity 自己定义的键没有 snake_case 孪生键；`range` 是复用的
    // UsageRangeWindow（同样 camelCase，已由 usage 的契约测试锁定）。
    for snake in ["usage_event_count", "cost_microunits", "has_more"] {
        assert!(json.get(snake).is_none(), "不得泄漏 {snake}");
    }

    let events = json["events"].as_array().expect("events 是数组");
    let event = events
        .first()
        .expect("有一条事件")
        .as_object()
        .expect("对象");
    assert_eq!(event.len(), ActivityEvent::keys().len());
    for key in ActivityEvent::keys() {
        assert!(event.contains_key(*key), "event 缺少键 {key}");
    }
    for snake in [
        "occurred_at",
        "total_tokens",
        "cost_microunits",
        "hub_session_id",
    ] {
        assert!(event.get(snake).is_none(), "event 不得泄漏 {snake}");
    }

    let days = json["days"].as_array().expect("days 是数组");
    let day = days.first().expect("有一天").as_object().expect("对象");
    assert_eq!(day.len(), ActivityDayBucket::keys().len());
    for key in ActivityDayBucket::keys() {
        assert!(day.contains_key(*key), "day 缺少键 {key}");
    }

    let hours = json["hours"].as_array().expect("hours 是数组");
    let hour = hours.first().expect("有小时").as_object().expect("对象");
    assert_eq!(hour.len(), ActivityHourBucket::keys().len());
    for key in ActivityHourBucket::keys() {
        assert!(hour.contains_key(*key), "hour 缺少键 {key}");
    }

    assert_eq!(json["range"]["kind"], "today");
    assert_eq!(json["range"]["timezoneOffsetMinutes"], 480);
    assert_eq!(events.first().expect("有事件")["day"], "2026-09-23");
}

/// `parse_range` 的非法输入必须报错，不得退化成「全部时间」（ADR-0012）。
#[test]
fn unknown_range_is_an_error_not_a_silent_all() {
    let error = parse_range("yesterday").expect_err("必须报错");
    assert!(
        error.to_string().contains("未知的时间范围"),
        "错误文案：{error}"
    );
}
