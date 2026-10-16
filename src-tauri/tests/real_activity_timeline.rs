//! 真机 Activity 数据链 E2E（§20 第 15 步）。
//!
//! 证明三件事：
//!
//! 1. 真机导入写进 SQLite 之后，`activity::query::timeline()` 的数字与**独立手写 SQL**
//!    逐项一致（事件流、按本地日聚合、按本地小时聚合三者互相自洽）；
//! 2. 关掉数据库再打开，Activity 的数字一字不变（数字完全由 SQLite 恢复）；
//! 3. 渲染路径**结构上**不可能起进程：`timeline()` 只接 `&Connection`。
//!
//! 本机没有 ccusage 时明确跳过，不伪装通过；ccusage 可用但来源报告为空同样跳过
//! （与 `real_dashboard_summary.rs` 共用 `common::precondition` 规则）。取数由
//! `common::ReplaySections::capture_once` 固定成同一份快照。
//!
//! 本文件不建任何 Hub 会话：`sessions` 表在真机环境下由 Terminal 运行时写入，
//! 因此 managed sessions 与 duration 的两态在单测里覆盖，这里只验证 usage 明细链。

use harness_hub_lib::activity::query;
use harness_hub_lib::db::Database;
use harness_hub_lib::harness::probe::SystemHostProbe;
use harness_hub_lib::usage::adapter::{CcusageAdapter, UsageSourceAdapter};
use harness_hub_lib::usage::runner::SystemCommandRunner;
use harness_hub_lib::usage::summary::{range_bounds, UsageRange};
use harness_hub_lib::usage::SourceStatus;

mod common;

/// 本地日聚合的独立 SQL（故意不用 `query.rs` 的 UNION ALL 口径）。
///
/// 返回 `(day, tokens)` 按 `day` 升序、**同一天只出现一次**：
/// `GROUP BY` 的就是现场算出来的本地日本身，所以这里天然一天一行。
fn independent_day_totals(connection: &rusqlite::Connection) -> Vec<(String, i64)> {
    let mut statement = connection
        .prepare(
            "SELECT strftime('%Y-%m-%d', occurred_at, '+480 minutes') AS day,
                    SUM(total_tokens) AS tokens
               FROM usage_events
              WHERE occurred_at IS NOT NULL
              GROUP BY strftime('%Y-%m-%d', occurred_at, '+480 minutes')
              ORDER BY day ASC",
        )
        .expect("独立 SQL");
    let rows = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("查询");
    rows.map(|row| row.expect("行")).collect()
}

#[test]
fn the_activity_timeline_matches_an_independent_sql_query() {
    let probe = SystemHostProbe::new();
    let executor = SystemCommandRunner::new();
    let detector = CcusageAdapter::new(&probe, &executor, None);

    if detector.detect().status != SourceStatus::Available {
        eprintln!("跳过：本机没有可用的 ccusage runner");
        return;
    }

    // 与 `real_dashboard_summary.rs` 同口径：真实跑一次，导入 replay 同一份输出。
    let (replayer, raw) =
        common::ReplaySections::capture_once(&probe, &executor, common::CaptureWindow::Full)
            .expect("detect 说可用就必须能解析出 runner");
    assert_eq!(raw.exit_code, 0, "ccusage 必须成功：{}", raw.stderr);
    let report: serde_json::Value = serde_json::from_str(&raw.stdout).expect("合法 JSON");
    let adapter = CcusageAdapter::new(&probe, &replayer, None);

    let directory = std::env::temp_dir().join(format!("hh-activity-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("临时目录");
    let database_file = directory.join("harness-hub.sqlite3");
    let _ = std::fs::remove_file(&database_file);

    let database = Database::open(&database_file).expect("打开数据库");
    let outcome = adapter.import(database.connection()).expect("真机导入");
    match common::precondition(&report, outcome.import.records_seen) {
        common::Precondition::NoDataInSource => {
            eprintln!(
                "跳过：ccusage 可用，但来源报告本身是空的（session/daily 都没有行）——\
                 对账断言只在有真实数据的机器上有意义"
            );
            drop(database);
            let _ = std::fs::remove_dir_all(&directory);
            return;
        }
        common::Precondition::SourceHadRowsButNothingImported => panic!(
            "来源报告里有行，导入却得到 0 条：adapter 丢行（不是机器没有数据）。\
             报告 session/daily 行数 = {}/{}",
            report["session"].as_array().map_or(0, Vec::len),
            report["daily"].as_array().map_or(0, Vec::len)
        ),
        common::Precondition::Proceed => {}
    }
    let now = harness_hub_lib::clock::now_rfc3339();

    let window = range_bounds(database.connection(), UsageRange::All, &now, 480).expect("窗口");
    let all = query::timeline(database.connection(), &window, 10_000).expect("时间线");

    assert_eq!(
        all.usage_event_count, outcome.import.records_seen,
        "活动流里的 usage 事件数必须等于刚导入的记录数"
    );
    assert!(
        all.usage_event_count > 0,
        "前提已由上面的「有真实数据」守卫保证：库里必须有事件"
    );

    // ---- 1. 与独立手写 SQL 交叉验证 -----------------------------------
    let (sql_tokens, sql_events): (i64, i64) = database
        .connection()
        .query_row(
            "SELECT (SELECT COALESCE(SUM(total_tokens), 0) FROM usage_events),
                    (SELECT COUNT(*) FROM usage_events)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("独立 SQL");

    assert_eq!(
        all.total_tokens, sql_tokens as u64,
        "token 总数必须与独立 SQL 一致"
    );
    assert_eq!(all.usage_event_count, sql_events as u64);

    // 事件流（未截断时）与按天聚合必须自洽：三个维度加总同一个总数。
    assert!(!all.has_more, "limit 给得足够大时不应报告截断");
    let event_tokens: u64 = all.events.iter().map(|event| event.total_tokens).sum();
    assert_eq!(
        event_tokens, all.total_tokens,
        "事件流之和 == 全局 token 数"
    );
    let day_tokens: u64 = all.days.iter().map(|bucket| bucket.total_tokens).sum();
    assert_eq!(day_tokens, all.total_tokens, "按天之和 == 全局");
    let hour_tokens: u64 = all.hours.iter().map(|bucket| bucket.total_tokens).sum();
    assert_eq!(hour_tokens, all.total_tokens, "按小时之和 == 全局");
    assert_eq!(all.hours.len(), 24, "按小时恒 24 项");
    assert_eq!(
        all.active_days,
        all.days.len() as u64,
        "active_days == days 长度"
    );
    for (index, hour) in all.hours.iter().enumerate() {
        assert_eq!(hour.hour as usize, index, "hours 必须按 hour 升序且无重复");
    }

    // 按天聚合与独立 SQL 逐日一致（独立 SQL 故意不用 query.rs 的 UNION ALL 口径）。
    //
    // 独立 SQL 的 day 是从 occurred_at 现场算的本地日，`query.rs` 也算本地日，因此两边
    // 的**日期集合与每一天的 token 数都必须逐项相同**。这不是巧合：两处都只用
    // `occurred_at + 时区偏移`，不碰库里那列导入时写死的 `day`（ccusage 按 UTC+0 算的，
    // 与本地日会差一天）。哪天这里对不上，就说明聚合偷偷用了错的那一列。
    let sql_days = independent_day_totals(database.connection());
    assert!(
        !sql_days.is_empty(),
        "前提已保证有真实数据：独立 SQL 必须至少给出一天"
    );
    let projected: Vec<&str> = all.days.iter().map(|bucket| bucket.day.as_str()).collect();
    let independent: Vec<&str> = sql_days.iter().map(|(day, _)| day.as_str()).collect();
    assert_eq!(
        projected, independent,
        "按天聚合的日期序列必须与独立 SQL 逐项相同"
    );
    for (bucket, (day, tokens)) in all.days.iter().zip(sql_days.iter()) {
        assert_eq!(
            bucket.total_tokens, *tokens as u64,
            "{day} 的 token 数必须与独立 SQL 一致"
        );
    }

    // ---- 2. 关库重开：数字一字不变 ------------------------------------
    drop(database);
    let reopened = Database::open(&database_file).expect("重新打开");
    let reopened_window =
        range_bounds(reopened.connection(), UsageRange::All, &now, 480).expect("窗口");
    let after_restart =
        query::timeline(reopened.connection(), &reopened_window, 10_000).expect("时间线");
    assert_eq!(
        after_restart, all,
        "重启后 Activity 的每一个数字都必须完全一致"
    );

    // ---- 3. 时间窗：本地今天随偏移变化，且是有界子集 -------------------
    let today_utc_plus_8 =
        range_bounds(reopened.connection(), UsageRange::Today, &now, 480).expect("窗口");
    let today_utc_plus_0 =
        range_bounds(reopened.connection(), UsageRange::Today, &now, 0).expect("窗口");
    assert_ne!(
        today_utc_plus_8.start_utc, today_utc_plus_0.start_utc,
        "不同时区的「今天」不是同一个 UTC 窗口"
    );
    let today = query::timeline(reopened.connection(), &today_utc_plus_8, 10_000).expect("时间线");
    let month = query::timeline(
        reopened.connection(),
        &range_bounds(reopened.connection(), UsageRange::Days30, &now, 480).expect("窗口"),
        10_000,
    )
    .expect("时间线");
    assert!(
        today.usage_event_count <= month.usage_event_count,
        "今天的 usage 事件数不可能超过 30 天（{} vs {}）",
        today.usage_event_count,
        month.usage_event_count
    );
    assert!(
        today.total_tokens <= month.total_tokens,
        "今天的 token 数不可能超过 30 天"
    );

    eprintln!(
        "Activity 对账通过：usage 事件 {} / tokens {} / 活跃本地日 {} / 今天 {} 条（30 天 {} 条）",
        all.usage_event_count,
        all.total_tokens,
        all.active_days,
        today.usage_event_count,
        month.usage_event_count
    );
    eprintln!(
        "说明：以上全部由 `timeline(&Connection)` 计算 —— 该函数签名里没有 runner / executor，\
         因此渲染路径结构上不可能启动 ccusage。"
    );

    drop(reopened);
    let _ = std::fs::remove_dir_all(&directory);
}
