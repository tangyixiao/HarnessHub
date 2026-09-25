//! 真机 ccusage E2E：真实调用 → 导入 → **同快照对账** → 重启后仍在。
//!
//! 这个测试的价值不是「跑通」，而是**把对账做成可证伪的断言**：
//!
//! 1. `Σ(事件 total) + unattributed == 同一次调用的 totals.totalTokens`（精确，不是近似）；
//! 2. 四类 token 逐项相等；
//! 3. 金额残差（每个事件独立舍入造成）必须 ≤ ⌈计价事件数 / 2⌉ 微单位，**并把具体数值打印出来**；
//! 4. `daily` 与 `session` 的口径差异必须**逐 agent 归因**，不允许出现无法解释的残差；
//! 5. 第二次导入 `inserted == 0`，且总量一分不变（幂等）；
//! 6. 关掉数据库重开，数字仍在（重启持久化）；
//! 7. 历史事件全部 `hub_session_id IS NULL`（不伪造 Harness Hub 会话）。
//!
//! 本机没有 ccusage 时测试会**明确跳过**并打印原因，而不是伪装通过。
//!
//! 同理，**ccusage 可用不等于这台机器有真实用量**：GitHub runner 上 `npx` 能装好 ccusage，
//! 但 home 里没有 Claude/Codex 数据，导入会**合法地**得到 0 条记录。那种机器上也明确跳过，
//! 有真实数据的机器上则必须完整跑完下面每一条对账断言。
//!
//! 跳过条件由报告**本身**决定（`common::precondition`）：报告 `session`/`daily` 都没有行
//! 才算「机器没有数据」；报告里有行却一条都没导入是 adapter 丢行，必须失败而不是跳过。
//!
//! 取数由 `common::ReplaySections` 固定成**同一份输出**：ccusage 的统计来自活的 rollout 文件，
//! 两次调用之间同一个 key 的数值会变大，所以报告与后续 import 必须复用同一次真实运行的输出。
//! 这条用例是**唯一**需要稳定窗口（`CaptureWindow::StablePast` → `--until <UTC 今天-2 天> -z UTC`）
//! 的：它要比较同一份报告内部的 `daily` 与 `session`，而进行中的日期在被读取期间还在增长，
//! 连单次调用内部的两个 section 都会互相不一致（实测同一份报告里 codex daily 比 session
//! 少 166271 / 84090，写入暂停时恰好 0）。
//! 断言没有放宽，也没有新增跳过：`[2]` 逐类 token、`[4]` codex 两口径、`[5]` 幂等在稳定快照下
//! 变成**精确**成立。
//!
//! **覆盖边界（必须如实读）**：这条对账针对「已结束的日期」，不含最近 ≥24 小时；代价是不再覆盖
//! 进行中的那一天，收益是同一份报告真的自洽。若本机只有最近两天的用量，报告为空 → 跳过。
//! 其余两条真机用例（`claude_usage` / `real_dashboard_summary`）用 `CaptureWindow::Full`：
//! 它们只需要 replay，不能因此丢掉今天的覆盖。

use std::collections::BTreeMap;

use harness_hub_lib::db::Database;
use harness_hub_lib::harness::probe::SystemHostProbe;
use harness_hub_lib::usage::adapter::{CcusageAdapter, UsageSourceAdapter};
use harness_hub_lib::usage::importer::UsageImporter;
use harness_hub_lib::usage::runner::{resolve_runner, SystemCommandRunner};
use harness_hub_lib::usage::{ImportStatus, SourceStatus};

mod common;

#[test]
fn real_ccusage_import_reconciles_against_its_own_totals() {
    let probe = SystemHostProbe::new();
    let executor = SystemCommandRunner::new();
    let detector = CcusageAdapter::new(&probe, &executor, None);

    let source = detector.detect();
    if source.status != SourceStatus::Available {
        eprintln!(
            "跳过：本机没有可用的 ccusage runner（{}）",
            source.reason.unwrap_or_default()
        );
        return;
    }
    let runner = resolve_runner(&probe, None).expect("detect 说可用就一定能解析出 runner");
    eprintln!("runner：{:?} → {}", runner.kind, runner.command.describe());

    // 真实跑**一次**，之后的 import（对账 + 幂等）全部复用这一份输出。
    //
    // 为什么必须复用：ccusage 的统计来自活的 rollout 文件（本机 codex 会话在持续追加），
    // 每次调用都会重新推导，同一个 key 的数值在两次调用之间会变大（实测 12s 内 +36920 token）。
    // 「同快照对账」要的是同一份数据，所以让被比较的双方由构造保证来自同一份输出。
    //
    // 这条用例**需要**稳定窗口（`StablePast`）：它要比较同一份报告里的 `daily` 与 `session`，
    // 而进行中的日期在被读取期间还在增长，连单次调用内部的两个 section 都会互相不一致。
    let (replayer, raw) =
        common::ReplaySections::capture_once(&probe, &executor, common::CaptureWindow::StablePast)
            .expect("detect 说可用就一定能解析出 runner");
    assert_eq!(raw.exit_code, 0, "ccusage 必须成功：{}", raw.stderr);
    let report: serde_json::Value = serde_json::from_str(&raw.stdout).expect("合法 JSON");
    let adapter = CcusageAdapter::new(&probe, &replayer, None);

    let directory = std::env::temp_dir().join(format!("hh-usage-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("临时目录");
    let database_file = directory.join("harness-hub.sqlite3");
    let _ = std::fs::remove_file(&database_file);

    // ---- 第一次导入 ----------------------------------------------------
    let database = Database::open(&database_file).expect("打开数据库");
    let outcome = adapter.import(database.connection()).expect("导入");
    let reconciliation = outcome.reconciliation;

    assert_eq!(outcome.import.status, ImportStatus::Succeeded);
    // 前提判定只看**报告本身**（见文件头与 `common::precondition`）：
    // 报告空 → 这台机器确实没有用量，明确跳过；报告有行却一条都没导入 → **硬失败**，
    // 因为那是 adapter 丢行，不是「机器没有数据」。
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
    assert_eq!(
        outcome.import.records_inserted, outcome.import.records_seen,
        "首次导入必须全部插入"
    );
    assert!(
        outcome
            .import
            .source_version
            .as_deref()
            .is_some_and(|version| version.starts_with("ccusage ")),
        "provenance 必须带真实版本：{:?}",
        outcome.import.source_version
    );

    let totals = UsageImporter::new(database.connection())
        .totals()
        .expect("库内汇总");

    // [1] token 恒等式：每一 token 都有归属（事件或明示的差额）
    let report_total_tokens = reconciliation
        .report_total_tokens
        .expect("totals.totalTokens");
    assert_eq!(
        totals.total_tokens + reconciliation.row_total_residual,
        report_total_tokens,
        "Σ事件({}) + 无法归因({}) 必须等于来源汇总({})",
        totals.total_tokens,
        reconciliation.row_total_residual,
        report_total_tokens
    );
    assert_eq!(reconciliation.tokens_identity_holds, Some(true));

    // [2] 四类 token 逐项相等
    let expected = |field: &str| -> u64 { report["totals"][field].as_u64().unwrap_or(0) };
    assert_eq!(totals.input_tokens, expected("inputTokens"), "input 对不上");
    assert_eq!(
        totals.output_tokens,
        expected("outputTokens"),
        "output 对不上"
    );
    assert_eq!(
        totals.cache_creation_tokens,
        expected("cacheCreationTokens"),
        "cacheCreation 对不上"
    );
    assert_eq!(
        totals.cached_input_tokens,
        expected("cacheReadTokens"),
        "cacheRead 对不上"
    );

    // [3] 金额残差：可证明的上界 + 打印具体数值
    let priced_events = outcome.import.records_seen - reconciliation.unpriced_events;
    let delta = reconciliation.cost_microunits_delta.expect("来源给了 cost");
    let bound = priced_events.div_ceil(2) as i64;
    eprintln!(
        "金额：Σ事件 - 来源 = {delta} 微单位（{} 个计价事件，上界 {bound}）",
        priced_events
    );
    assert!(
        delta.abs() <= bound,
        "金额残差 {delta} 超过独立舍入的可证明上界 {bound}"
    );

    // [4] daily vs session：差异必须逐 agent 归因
    assert_scope_differences_are_attributed(&report);

    // [5] 幂等：再导一次，不得新增
    let second = adapter.import(database.connection()).expect("重复导入");
    assert_eq!(second.import.records_inserted, 0, "重复导入不得新增事件");
    assert_eq!(
        second.import.records_skipped, second.import.records_seen,
        "重复导入应全部跳过"
    );
    let after = UsageImporter::new(database.connection())
        .totals()
        .expect("二次汇总");
    assert_eq!(after, totals, "重复导入不得改变任何汇总");

    // [6] 不伪造 hub session，且键唯一
    let (linked, distinct_keys, events) = database
        .connection()
        .query_row(
            "SELECT
                 (SELECT COUNT(*) FROM usage_events WHERE hub_session_id IS NOT NULL),
                 (SELECT COUNT(DISTINCT stable_source_key) FROM usage_events),
                 (SELECT COUNT(*) FROM usage_events)",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .expect("统计");
    assert_eq!(
        linked, 0,
        "本地没有任何会话，历史事件不得凭空关联 hub session"
    );
    assert_eq!(distinct_keys, events, "stable_source_key 必须两两不同");
    assert_eq!(events as u64, outcome.import.records_seen);

    // [7] 重启持久化：关掉再开，数字仍在
    drop(database);
    let reopened = Database::open(&database_file).expect("重新打开数据库");
    let persisted = UsageImporter::new(reopened.connection())
        .totals()
        .expect("重开后的汇总");
    assert_eq!(persisted, totals, "重启后数字必须一字不差");
    let history = UsageImporter::new(reopened.connection())
        .import_history("ccusage")
        .expect("导入历史");
    assert_eq!(history.len(), 2, "两次导入必须留下两条审计行");
    assert!(
        history
            .iter()
            .all(|record| record.status == ImportStatus::Succeeded),
        "两次都必须成功"
    );
    drop(reopened);

    eprintln!(
        "对账通过：事件 {} 条 / total {} / 无法归因 {} / 金额残差 {} 微单位 / 未定价 {} 条",
        outcome.import.records_seen,
        totals.total_tokens,
        reconciliation.row_total_residual,
        delta,
        reconciliation.unpriced_events
    );

    let _ = std::fs::remove_dir_all(&directory);
}

/// `daily` 与 `session` 的口径差必须**全部**归因到具体 agent。
///
/// 允许某个 agent 两边不等（上游聚合口径不同，实测 claude 就是这样），
/// 但不允许出现「谁都解释不了」的残差；我们主打的 codex 更必须逐 token 相等。
fn assert_scope_differences_are_attributed(report: &serde_json::Value) {
    let sessions = report["session"].as_array().expect("session 数组");
    let daily = report["daily"].as_array().expect("daily 数组");

    let mut by_session: BTreeMap<&str, u64> = BTreeMap::new();
    for row in sessions {
        let harness = row["agent"].as_str().expect("agent");
        if !harness.is_empty() {
            *by_session.entry(harness).or_default() += row["totalTokens"].as_u64().unwrap_or(0);
        }
    }

    let mut by_daily_agent: BTreeMap<&str, u64> = BTreeMap::new();
    for row in daily {
        if let Some(agents) = row["agents"].as_array() {
            for entry in agents {
                let harness = entry["agent"].as_str().expect("agent");
                *by_daily_agent.entry(harness).or_default() +=
                    entry["totalTokens"].as_u64().unwrap_or(0);
            }
        }
    }

    let daily_total: i128 = daily
        .iter()
        .map(|row| row["totalTokens"].as_u64().unwrap_or(0) as i128)
        .sum();
    let session_total: i128 = by_session.values().map(|value| *value as i128).sum();
    let overall = daily_total - session_total;

    let mut attributed: i128 = 0;
    let mut explained = Vec::new();
    for (harness, session_tokens) in &by_session {
        let daily_tokens = by_daily_agent.get(harness).copied().unwrap_or(0);
        let difference = daily_tokens as i128 - *session_tokens as i128;
        attributed += difference;
        if difference != 0 {
            explained.push(format!(
                "{harness}: daily {daily_tokens} vs session {session_tokens}（差 {difference}）"
            ));
        }
    }

    eprintln!("daily - session = {overall}；逐 agent 归因：{explained:?}");
    assert_eq!(
        attributed, overall,
        "daily 与 session 的差异必须逐 agent 完全归因，不允许留下无法解释的残差"
    );
    assert_eq!(
        by_session.get("codex").copied().unwrap_or(0),
        by_daily_agent.get("codex").copied().unwrap_or(0),
        "codex 是我们支持的主路径，两个口径必须逐 token 相等"
    );
    assert!(
        !explained.is_empty() || overall == 0,
        "没有差异时不该报告差异"
    );
}
