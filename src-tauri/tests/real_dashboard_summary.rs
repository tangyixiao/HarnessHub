//! 真机 Dashboard 数据链 E2E（ADR-0012）。
//!
//! 证明四件事：
//!
//! 1. Task 5 的真机导入写进 SQLite 之后，`summary()` 的数字与**独立手写 SQL**逐项一致；
//! 2. 关掉数据库再打开，Dashboard 的数字一字不变（数字完全由 SQLite 恢复）；
//! 3. 有界时间窗确实是 `[start, end)`，且「本地今天」随偏移变化；
//! 4. 渲染路径**结构上**不可能起进程：`summary()` 只接 `&Connection`。
//!
//! 本机没有 ccusage 时明确跳过，不伪装通过。
//!
//! 同理，**ccusage 可用不等于这台机器有真实用量**：GitHub runner 上 `npx` 能装好 ccusage，
//! 但 home 里没有数据，导入会**合法地**得到 0 条记录。跳过条件与 `real_ccusage_import.rs`
//! 共用同一条规则（`common::precondition`）：报告本身为空才算「机器没有数据」；
//! 报告里有行却一条都没导入是 adapter 丢行，必须失败。
//!
//! 取数同样由 `common::ReplaySections::capture_once` 固定成一份稳定快照（真实跑一次 +
//! `--until <UTC 今天-2 天> -z UTC`），导入 replay 它，库里的行与拿来判定的报告因此同源。
//! 覆盖边界：对账针对已结束的日期，不含最近 ≥24 小时。

use harness_hub_lib::db::Database;
use harness_hub_lib::harness::probe::SystemHostProbe;
use harness_hub_lib::usage::adapter::{CcusageAdapter, UsageSourceAdapter};
use harness_hub_lib::usage::runner::SystemCommandRunner;
use harness_hub_lib::usage::summary::{range_bounds, summary, UsageRange};
use harness_hub_lib::usage::SourceStatus;

mod common;

#[test]
fn the_dashboard_summary_matches_an_independent_sql_query() {
    let probe = SystemHostProbe::new();
    let executor = SystemCommandRunner::new();
    let detector = CcusageAdapter::new(&probe, &executor, None);

    if detector.detect().status != SourceStatus::Available {
        eprintln!("跳过：本机没有可用的 ccusage runner");
        return;
    }

    // 与 `real_ccusage_import.rs` 一样：真实跑**一次**并固定成一份稳定快照。
    // 前提判定要看报告本身是否为空（不能只看「导入了 0 条」，那会把 adapter 丢行一起吞掉），
    // 而导入必须 replay 同一份输出 —— 否则库里的行会来自另一次调用，跨快照比较没有意义。
    let (replayer, raw) = common::ReplaySections::capture_once(&probe, &executor)
        .expect("detect 说可用就必须能解析出 runner");
    assert_eq!(raw.exit_code, 0, "ccusage 必须成功：{}", raw.stderr);
    let report: serde_json::Value = serde_json::from_str(&raw.stdout).expect("合法 JSON");
    let adapter = CcusageAdapter::new(&probe, &replayer, None);

    let directory = std::env::temp_dir().join(format!("hh-dashboard-e2e-{}", std::process::id()));
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
    let all = summary(database.connection(), &window).expect("聚合");

    assert_eq!(
        all.event_count, outcome.import.records_seen,
        "库里的事件数必须等于刚导入的记录数"
    );
    assert!(
        all.event_count > 0,
        "前提已由上面的「有真实数据」守卫保证：库里必须有事件"
    );

    // ---- 1. 与独立手写 SQL 交叉验证 -----------------------------------
    let (sql_tokens, sql_cost, sql_events, sql_sessions): (i64, Option<i64>, i64, i64) = database
        .connection()
        .query_row(
            "SELECT
                 (SELECT COALESCE(SUM(total_tokens), 0) FROM usage_events),
                 (SELECT SUM(cost_microunits) FROM usage_events),
                 (SELECT COUNT(*) FROM usage_events),
                 (SELECT COUNT(*) FROM (
                      SELECT DISTINCT harness, source_session_id FROM usage_events
                  ))",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("独立 SQL");

    assert_eq!(
        all.totals.total_tokens, sql_tokens as u64,
        "token 总数必须一致"
    );
    assert_eq!(all.cost_microunits, sql_cost, "已知成本必须一致");
    assert_eq!(all.event_count, sql_events as u64);
    assert_eq!(all.usage_sessions, sql_sessions as u64);

    // 各维度之和 == 全局（GROUP BY 没漏行也没重复计数）
    let harness_tokens: u64 = all.by_harness.iter().map(|b| b.total_tokens).sum();
    let model_tokens: u64 = all.by_model.iter().map(|b| b.total_tokens).sum();
    let timeline_tokens: u64 = all.timeline.iter().map(|b| b.total_tokens).sum();
    assert_eq!(harness_tokens, all.totals.total_tokens);
    assert_eq!(model_tokens, all.totals.total_tokens);
    assert_eq!(
        timeline_tokens + all.timestampless_records,
        all.totals.total_tokens,
        "时间线之和 + 无时间戳事件 == 全局（无时间戳的进不了任何一天）"
    );

    // ---- 2. 关库重开：数字一字不变 ------------------------------------
    drop(database);
    let reopened = Database::open(&database_file).expect("重新打开");
    let reopened_window =
        range_bounds(reopened.connection(), UsageRange::All, &now, 480).expect("窗口");
    let after_restart = summary(reopened.connection(), &reopened_window).expect("聚合");
    assert_eq!(
        after_restart, all,
        "重启后 Dashboard 的每一个数字都必须完全一致"
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
    let today = summary(reopened.connection(), &today_utc_plus_8).expect("聚合");
    let month = summary(
        reopened.connection(),
        &range_bounds(reopened.connection(), UsageRange::Days30, &now, 480).expect("窗口"),
    )
    .expect("聚合");
    assert!(
        today.event_count <= month.event_count,
        "今天的事件数不可能超过 30 天（{} vs {}）",
        today.event_count,
        month.event_count
    );
    assert!(
        today.excluded_timestampless >= all.timestampless_records,
        "有界窗口必须报告被排除掉的无时间戳事件"
    );

    eprintln!(
        "Dashboard 对账通过：事件 {} / tokens {} / 已知成本 {:?} 微单位 / 下界 {} / \
         usage sessions {} / managed sessions {} / 今天 {} 条（30 天 {} 条）",
        all.event_count,
        all.totals.total_tokens,
        all.cost_microunits,
        all.cost_is_lower_bound,
        all.usage_sessions,
        all.managed_sessions,
        today.event_count,
        month.event_count
    );
    eprintln!(
        "说明：以上全部由 `summary(&Connection)` 计算 —— 该函数签名里没有 runner / executor，\
         因此渲染路径结构上不可能启动 ccusage（真正的「不起进程」断言在前端命令分派测试里）。"
    );

    drop(reopened);
    let _ = std::fs::remove_dir_all(&directory);
}
