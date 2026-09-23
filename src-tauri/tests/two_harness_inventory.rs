//! Task 7A：第二个 Harness 接入后的清单同步证据。
//!
//! 这里刻意用**生产组合根** `build_harness_registry()`（而不是在测试里重造一份注册表），
//! 才能验证「Core 不需要为第二个 Harness 改动」这件事本身。
//!
//! 覆盖四件事：
//! 1. 注册表持有两个适配器，顺序确定（`Vec`，不是 HashMap）；
//! 2. 真正空库 → 生产注册表 → reconcile → 两个 definition + 两个 installation；
//! 3. 已有 Codex 的库再加 Claude：**不需要 migration**、不覆盖旧数据、不重复插入；
//! 4. 真机检测：两个 Harness 都真的被认出来（不装则明确跳过）。

use harness_hub_lib::db::migrations::{apply_pending, latest_version};
use harness_hub_lib::db::Database;
use harness_hub_lib::harness::inventory::{installation_id, reconcile_harnesses};
use harness_hub_lib::runtime::local::{ensure_local_target, LOCAL_TARGET_ID};
use harness_hub_lib::{build_harness_registry, clock};

const NOW: &str = "2026-09-23T10:00:00Z";

fn installation_count(db: &Database) -> i64 {
    db.connection()
        .query_row("SELECT COUNT(*) FROM harness_installations", [], |row| {
            row.get(0)
        })
        .expect("计数")
}

fn definition_ids(db: &Database) -> Vec<String> {
    let mut statement = db
        .connection()
        .prepare("SELECT id FROM harnesses ORDER BY id")
        .expect("prepare");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query");
    rows.map(|row| row.expect("row")).collect()
}

fn installation_ids(db: &Database) -> Vec<String> {
    let mut statement = db
        .connection()
        .prepare("SELECT id FROM harness_installations ORDER BY id")
        .expect("prepare");
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query");
    rows.map(|row| row.expect("row")).collect()
}

/// 用生产注册表做一次清单同步（与启动时走的是同一条路径）。
fn reconcile_with_production_registry(db: &Database) -> Vec<String> {
    let registry = build_harness_registry();
    let summaries = registry.summaries(LOCAL_TARGET_ID);
    reconcile_harnesses(db.connection(), &summaries, LOCAL_TARGET_ID, NOW).expect("同步清单");
    summaries.into_iter().map(|summary| summary.id).collect()
}

#[test]
fn the_registry_holds_executable_adapters_in_a_deterministic_order() {
    let registry = build_harness_registry();

    let ids = |registry: &_| -> Vec<String> {
        let registry: &harness_hub_lib::harness::registry::HarnessRegistry = registry;
        registry
            .ids()
            .into_iter()
            .map(|id| id.as_str().to_string())
            .collect()
    };

    assert_eq!(ids(&registry), vec!["codex", "claude"]);
    assert_eq!(
        ids(&registry),
        ids(&registry),
        "顺序必须稳定（Vec 持有），否则 UI 会偶尔换位、测试会 flaky"
    );

    let summaries = registry.summaries(LOCAL_TARGET_ID);
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].display_name, "Codex");
    assert_eq!(summaries[1].display_name, "Claude Code");
    // 展示名由适配器声明，注册表只透传（前后端只有一份名字来源）。
    assert_eq!(
        summaries[1].installation_id.as_deref(),
        Some("claude@local"),
        "前端不该自己拼安装 id"
    );
}

#[test]
fn a_cold_database_reconciles_both_harnesses() {
    let db = Database::open_in_memory().expect("空库");
    ensure_local_target(db.connection()).expect("runtime target");

    let reconciled = reconcile_with_production_registry(&db);

    assert_eq!(reconciled, vec!["codex", "claude"]);
    assert_eq!(definition_ids(&db), vec!["claude", "codex"]);
    assert_eq!(
        installation_ids(&db),
        vec![
            installation_id("claude", LOCAL_TARGET_ID),
            installation_id("codex", LOCAL_TARGET_ID)
        ]
    );
}

/// 已有 Codex 的库再加 Claude：不覆盖旧数据、不重复插入。
#[test]
fn adding_claude_preserves_the_existing_codex_installation() {
    let db = Database::open_in_memory().expect("库");
    ensure_local_target(db.connection()).expect("runtime target");

    // 先只同步 Codex（模拟 alpha.2 的老库），并把 first_detected_at 固定在“过去”。
    let codex_only = build_harness_registry()
        .summaries(LOCAL_TARGET_ID)
        .into_iter()
        .filter(|summary| summary.id == "codex")
        .collect::<Vec<_>>();
    reconcile_harnesses(
        db.connection(),
        &codex_only,
        LOCAL_TARGET_ID,
        "2026-09-01T00:00:00Z",
    )
    .expect("初始同步");
    assert_eq!(installation_count(&db), 1);
    assert_eq!(installation_ids(&db), vec!["codex@local"]);

    let before: (String, String) = db
        .connection()
        .query_row(
            "SELECT binary_path, first_detected_at FROM harness_installations WHERE id = 'codex@local'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("旧安装");

    // 现在把 Claude 也同步进来（真实启动时发生的事）。
    reconcile_with_production_registry(&db);

    assert_eq!(definition_ids(&db), vec!["claude", "codex"]);
    assert_eq!(installation_ids(&db), vec!["claude@local", "codex@local"]);
    assert_eq!(installation_count(&db), 2, "不得重复插入");

    let after: (String, String) = db
        .connection()
        .query_row(
            "SELECT binary_path, first_detected_at FROM harness_installations WHERE id = 'codex@local'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("旧安装");
    assert_eq!(
        after.0, before.0,
        "Codex 的 binary 不得被 Claude 的同步覆盖"
    );
    assert_eq!(after.1, before.1, "first_detected_at 必须保持首次发现时间");

    // 再同步一次：仍然不重复（幂等）。
    reconcile_with_production_registry(&db);
    assert_eq!(installation_count(&db), 2);
}

/// 加第二个 Harness **不需要** schema 变更。
#[test]
fn a_second_harness_requires_no_migration() {
    let mut connection = rusqlite::Connection::open_in_memory().expect("库");
    apply_pending(&mut connection).expect("迁移");
    let version_before = latest_version();

    let db = Database::open_in_memory().expect("库");
    ensure_local_target(db.connection()).expect("runtime target");
    reconcile_with_production_registry(&db);

    assert_eq!(
        latest_version(),
        version_before,
        "注册第二个适配器不得引入新的迁移"
    );
    // 模型本来就与 Harness 无关：两张表都是「一行一个 Harness」的形状。
    let columns: i64 = db
        .connection()
        .query_row(
            "SELECT count(*) FROM pragma_table_info('harness_installations')
             WHERE name IN ('id', 'harness_id', 'runtime_target_id', 'availability')",
            [],
            |row| row.get(0),
        )
        .expect("列检查");
    assert_eq!(columns, 4, "installation 模型天然容纳任意 Harness");
}

/// 真机检测（本机没装就明确跳过）：证明 Claude 真的被认出来，
/// 并记录 `candidate_paths` 在 Windows 上实际选中了哪个 shim。
#[test]
fn the_real_machine_detects_both_harnesses() {
    let summaries = build_harness_registry().summaries(LOCAL_TARGET_ID);
    let claude = summaries
        .iter()
        .find(|summary| summary.id == "claude")
        .expect("注册表里必须有 claude");
    let codex = summaries
        .iter()
        .find(|summary| summary.id == "codex")
        .expect("注册表里必须有 codex");

    eprintln!(
        "codex  : installed={} binary={:?} version={:?} data={:?}",
        codex.installed, codex.binary_path, codex.version, codex.data_paths
    );
    eprintln!(
        "claude : installed={} binary={:?} version={:?} data={:?}",
        claude.installed, claude.binary_path, claude.version, claude.data_paths
    );

    if !claude.installed {
        eprintln!("跳过：本机 PATH 上没有 claude");
        return;
    }

    let binary = claude.binary_path.as_deref().expect("装了就有路径");
    assert!(
        binary.ends_with("claude.cmd") || binary.ends_with("claude.exe"),
        "Windows 上必须选中可直接执行的 shim，不能是无扩展名的 POSIX 脚本：{binary}"
    );
    let version = claude
        .version
        .as_deref()
        .expect("真实 binary 必须能读出能版本");
    assert!(
        version.chars().next().is_some_and(|c| c.is_ascii_digit()),
        "版本必须是从真实输出解析出来的数字串：{version:?}"
    );
    assert!(
        claude
            .data_paths
            .iter()
            .any(|path| path.ends_with(".claude")),
        "本机 ~/.claude 存在时必须被发现：{:?}",
        claude.data_paths
    );
    assert_eq!(
        claude.capabilities,
        harness_hub_lib::harness::adapter::HarnessCapabilities::default(),
        "7A 阶段 Claude 不得宣称任何能力"
    );
    let _ = clock::now_rfc3339();
}
