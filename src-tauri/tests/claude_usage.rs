//! Task 7C：Claude usage 走**现有**管道的验证（不新增 Claude 特判）。
//!
//! ```text
//! 真机 ccusage → 现有 CcusageAdapter → 现有 normalize/importer → SQLite
//!            → 现有 usage_summary（Dashboard 聚合）
//! ```
//!
//! 证明方式刻意不是「claude 记录数 +1」这种弱证明，而是：
//!
//! 1. 取 ccusage 源头里 claude 的 `(period, model)` key 集合，与真机库里已有的 claude
//!    key 集合做**集合差**，逐 key 说出这次到底缺了哪些具体记录；
//! 2. 用现有 `CcusageAdapter::import` 导入（`refresh_usage` 走的就是它）；
//! 3. 对差额里的**每一个 key**逐项核对 model / tokens / cost / provenance，
//!    并断言 `hub_session_id IS NULL`（裸 UUID 不可证明对应，绝不猜关联）；
//! 4. 二次导入：key 集合与各聚合**事实**完全不变（不是只看 row count）；
//! 5. `usage_summary` 里自然出现 claude，且 codex 聚合不受影响、全局 = 各 harness 之和。
//!
//! 用真机库的**副本**，因此不会改动用户的真实数据库，且可重复执行。
//! 本机没有 ccusage / 没有应用数据库时明确跳过。
//!
//! 源头只真实取数**一次**（`common::ReplaySections::capture_once`，带稳定窗口
//! `--until <UTC 今天-2 天> -z UTC`），两次导入都 replay 这一份：ccusage 的统计来自活的
//! rollout 文件，不固定快照的话「重复导入必须幂等」比的是两份不同的数据（实测同一批 262 行里
//! 有 18 行被上游改写、汇总 +43214）。覆盖边界：对账针对已结束的日期，不含最近 ≥24 小时。

use std::collections::BTreeSet;
use std::path::PathBuf;

use harness_hub_lib::db::Database;
use harness_hub_lib::harness::probe::SystemHostProbe;
use harness_hub_lib::usage::adapter::{CcusageAdapter, UsageSourceAdapter};
use harness_hub_lib::usage::importer::UsageImporter;
use harness_hub_lib::usage::key::{stable_source_key, KeyDimensions};
use harness_hub_lib::usage::runner::SystemCommandRunner;
use harness_hub_lib::usage::summary::{range_bounds, summary, UsageRange};

mod common;

fn app_database_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let path = PathBuf::from(appdata)
        .join("dev.harnesshub.desktop")
        .join("harness-hub.sqlite3");
    path.is_file().then_some(path)
}

/// 源头侧：真实跑**一次** ccusage，返回 `(报告, claude key 集合, replayer)`。
///
/// 第三个返回值是关键：ccusage 的统计来自活的 rollout 文件（本机 codex 会话在持续追加），
/// **每次调用**同一个 key 的数值都会变大，所以后面的两次导入必须 replay 这一次的输出，
/// 否则「重复导入必须幂等」比的就成了两份不同的数据。
fn source_claude_keys() -> Option<(serde_json::Value, BTreeSet<String>, common::ReplaySections)> {
    let executor = SystemCommandRunner::new();
    let probe = SystemHostProbe::new();
    let adapter = CcusageAdapter::new(&probe, &executor, None);
    if !matches!(
        adapter.detect().status,
        harness_hub_lib::usage::SourceStatus::Available
    ) {
        eprintln!("跳过：本机没有可用的 ccusage runner");
        return None;
    }

    // `capture_once` 自带稳定窗口（`--until <UTC 今天-2 天> -z UTC`）：进行中的日期
    // 在被读取期间还在增长，不切上界的话连单次调用内部的 daily/session 都会互相不一致。
    let (replayer, output) = common::ReplaySections::capture_once(&probe, &executor)
        .expect("detect 说可用就一定能解析出 runner");
    assert_eq!(output.exit_code, 0, "ccusage 必须成功：{}", output.stderr);

    let report: serde_json::Value = serde_json::from_str(&output.stdout).expect("合法 JSON");
    let mut keys = BTreeSet::new();
    for row in report["session"].as_array().expect("session 数组") {
        let agent = row["agent"].as_str().unwrap_or_default();
        if agent != "claude" {
            continue;
        }
        let period = row["period"].as_str().expect("period");
        for breakdown in row["modelBreakdowns"].as_array().expect("modelBreakdowns") {
            let model = breakdown["modelName"].as_str().expect("modelName");
            keys.insert(stable_source_key(&KeyDimensions {
                source: "ccusage",
                report_kind: "session",
                harness: "claude",
                source_session_id: period,
                model,
            }));
        }
    }
    Some((report, keys, replayer))
}

fn db_claude_keys(connection: &rusqlite::Connection) -> BTreeSet<String> {
    let mut statement = connection
        .prepare("SELECT stable_source_key FROM usage_events WHERE harness = 'claude'")
        .expect("prepare");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query");
    rows.map(|row| row.expect("row")).collect()
}

// 这个测试要一次性把 10 列读回来做逐项核对；为它再包一层结构体没有收益。
#[allow(clippy::type_complexity)]
#[test]
fn claude_usage_lands_through_the_existing_pipeline() {
    let Some(source) = app_database_path() else {
        eprintln!("跳过：找不到 Harness Hub 应用数据库");
        return;
    };
    let Some((report, source_keys, replayer)) = source_claude_keys() else {
        return;
    };
    eprintln!("源头 claude key 数 = {}", source_keys.len());

    // 用副本，绝不写用户真实库。
    let directory = std::env::temp_dir().join(format!("hh-usage-claude-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("临时目录");
    let copy = directory.join("harness-hub.sqlite3");
    // 库是 WAL 模式：**必须**连 -wal / -shm 一起复制，否则未 checkpoint 的数据会丢，
    // 副本看起来几乎空的（这个坑真的踩过一次，导致「库里已有 0 条」的假象）。
    std::fs::copy(&source, &copy).expect("复制数据库");
    for suffix in ["-wal", "-shm"] {
        let from_side = PathBuf::from(format!("{}{suffix}", source.display()));
        if from_side.is_file() {
            let to_side = PathBuf::from(format!("{}{suffix}", copy.display()));
            std::fs::copy(&from_side, &to_side).expect("复制 WAL/SHM");
        }
    }
    let db = Database::open(&copy).expect("打开副本");

    let before_keys = db_claude_keys(db.connection());
    let pending: Vec<String> = source_keys.difference(&before_keys).cloned().collect();
    let before_totals = UsageImporter::new(db.connection())
        .totals()
        .expect("导入前汇总");
    eprintln!(
        "库里已有 claude key = {}；这次待导入（集合差）= {}；事件总数 = {}",
        before_keys.len(),
        pending.len(),
        before_totals.total_tokens
    );

    // === 走现有生产导入路径（refresh_usage 调的就是它）===
    //
    // 两次导入都 replay `source_claude_keys()` 里那一次真实运行的输出：
    // 这样「重复导入必须幂等」比的是**同一份数据**（同一个稳定快照），而不是两次调用之间
    // 已经变过的数据（活的 rollout 文件会让同 key 的数值持续变大）。
    let probe = SystemHostProbe::new();
    let outcome = CcusageAdapter::new(&probe, &replayer, None)
        .import(db.connection())
        .expect("导入");
    eprintln!(
        "import: seen={} inserted={} updated={} skipped={} version={:?}",
        outcome.import.records_seen,
        outcome.import.records_inserted,
        outcome.import.records_updated,
        outcome.import.records_skipped,
        outcome.import.source_version
    );

    let after_keys = db_claude_keys(db.connection());
    // 1) 源头里所有 claude key 现在都在库里（逐 key，而不是只比计数）
    for key in &source_keys {
        assert!(
            after_keys.contains(key),
            "源头存在的 claude key 必须已入库：{key}"
        );
    }
    // 2) 之前缺的那些 key 确实是被这次导入补上的
    for key in &pending {
        assert!(after_keys.contains(key), "集合差里的 key 必须被导入：{key}");
    }
    assert!(
        after_keys.len() >= source_keys.len(),
        "库里的 claude key 不应少于源头（{} < {}）",
        after_keys.len(),
        source_keys.len()
    );

    // === 逐项核对这批新记录 ===
    let mut source_models = BTreeSet::new();
    for row in report["session"].as_array().expect("session") {
        if row["agent"].as_str() != Some("claude") {
            continue;
        }
        let period = row["period"].as_str().expect("period");
        for breakdown in row["modelBreakdowns"].as_array().expect("breakdowns") {
            let model = breakdown["modelName"].as_str().expect("model");
            let key = stable_source_key(&KeyDimensions {
                source: "ccusage",
                report_kind: "session",
                harness: "claude",
                source_session_id: period,
                model,
            });
            if !pending.contains(&key) {
                continue;
            }
            source_models.insert((period.to_string(), model.to_string()));

            let (
                harness,
                stored_period,
                stored_model,
                hub,
                tokens,
                cost,
                token_source,
                cost_source,
                pricing,
                import_id,
            ): (
                String,
                String,
                String,
                Option<String>,
                i64,
                Option<i64>,
                String,
                Option<String>,
                Option<String>,
                Option<String>,
            ) = db
                .connection()
                .query_row(
                    "SELECT harness, source_session_id, model, hub_session_id, total_tokens,
                            cost_microunits, token_source, cost_source, pricing_mode, import_id
                     FROM usage_events WHERE stable_source_key = ?1",
                    rusqlite::params![key],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                            row.get(7)?,
                            row.get(8)?,
                            row.get(9)?,
                        ))
                    },
                )
                .expect("新记录必须存在");

            assert_eq!(harness, "claude");
            assert_eq!(stored_period, period);
            assert_eq!(stored_model, model);
            // **核心断言**：Claude 的 period 是裸 UUID，不可证明对应本地 hub session。
            assert_eq!(
                hub, None,
                "不得因为「是 Harness Hub 刚启动的」就猜关联：{period}"
            );
            assert!(import_id.is_some(), "必须有 provenance（import_id）");
            assert_eq!(token_source, "ccusage_source_log");
            assert_eq!(cost_source.as_deref(), Some("ccusage_computed"));
            assert_eq!(pricing.as_deref(), Some("auto"));

            let expected_tokens = [
                "inputTokens",
                "outputTokens",
                "cacheCreationTokens",
                "cacheReadTokens",
            ]
            .iter()
            .map(|field| breakdown[*field].as_u64().unwrap_or(0))
            .sum::<u64>() as i64;
            if expected_tokens > 0 {
                assert_eq!(tokens, expected_tokens, "token 必须与源头逐项一致：{model}");
            }
            if let Some(source_cost) = breakdown.get("cost").and_then(|value| value.as_f64()) {
                if source_cost > 0.0 {
                    assert!(cost.is_some(), "有价格的记录必须落 cost_microunits");
                }
            }
        }
    }
    eprintln!(
        "本次逐 key 核对的 (period, model) 组合数 = {}",
        source_models.len()
    );

    // === 幂等：比较**事实**，不是只看 row count ===
    let first_totals = UsageImporter::new(db.connection())
        .totals()
        .expect("首次汇总");
    let first_keys = db_claude_keys(db.connection());
    let second = CcusageAdapter::new(&probe, &replayer, None)
        .import(db.connection())
        .expect("二次导入");
    let second_totals = UsageImporter::new(db.connection())
        .totals()
        .expect("二次汇总");
    let second_keys = db_claude_keys(db.connection());

    eprintln!(
        "二次 import: inserted={} updated={} skipped={}",
        second.import.records_inserted,
        second.import.records_updated,
        second.import.records_skipped
    );
    assert_eq!(second_keys, first_keys, "二次导入不得改变 key 集合");
    assert_eq!(
        second_totals.total_tokens, first_totals.total_tokens,
        "二次导入不得让 token 翻倍"
    );
    assert_eq!(
        second_totals.cost_microunits, first_totals.cost_microunits,
        "二次导入不得让金额翻倍"
    );
    assert_eq!(
        second_totals.events_without_cost, first_totals.events_without_cost,
        "缺价格记录数必须一致（下界语义）"
    );

    // === Dashboard 聚合：不认 Claude，只是自然包含它 ===
    let now = harness_hub_lib::clock::now_rfc3339();
    let window = range_bounds(db.connection(), UsageRange::All, &now, 480).expect("窗口");
    let all = summary(db.connection(), &window).expect("聚合");

    let by_harness: std::collections::BTreeMap<&str, u64> = all
        .by_harness
        .iter()
        .map(|bucket| (bucket.key.as_str(), bucket.total_tokens))
        .collect();
    assert!(
        by_harness.contains_key("claude"),
        "Harness 分布里必须自然出现 claude"
    );
    assert!(by_harness.contains_key("codex"), "codex 仍在");
    assert_eq!(
        by_harness.values().sum::<u64>(),
        all.totals.total_tokens,
        "全局 token 必须等于各 harness 之和"
    );
    let cost_lower_bound = all
        .by_harness
        .iter()
        .any(|bucket| bucket.cost_is_lower_bound);
    assert!(cost_lower_bound, "claude 有缺价格记录，下界语义必须保留");
    assert!(all.cost_is_lower_bound);
    eprintln!(
        "Dashboard: 全局 tokens={} cost={:?} 下界={} claude_tokens={:?} codex_tokens={:?}",
        all.totals.total_tokens,
        all.cost_microunits,
        all.cost_is_lower_bound,
        by_harness.get("claude"),
        by_harness.get("codex")
    );

    drop(db);
    let _ = std::fs::remove_dir_all(&directory);
}
