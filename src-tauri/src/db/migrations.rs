//! 迁移执行器。
//!
//! 极简实现，刻意不引入 ORM / 迁移框架：
//!   * 迁移是**只增不改**的 `NNNN_*.sql` 文件，编译期内联进二进制；
//!   * 每条迁移在自己的事务里执行，失败即回滚（崩溃不会留下半截 schema）；
//!   * `schema_migrations` 记录已应用版本，重复执行是幂等的。

use rusqlite::{params, Connection};

use crate::error::{Error, Result};

/// 一条内联进二进制的迁移。
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub sql: &'static str,
    /// 是否需要在事务外临时关闭外键。
    ///
    /// SQLite 的「重建表」流程（12-step ALTER，例如修改 CHECK 约束）必须在
    /// 没有外键约束的情况下 `DROP TABLE`，否则会级联删掉子表数据；
    /// 而 `PRAGMA foreign_keys` 在事务内切换是 no-op，所以由执行器在事务外负责。
    /// 迁移结束后执行器会重新打开外键并跑 `PRAGMA foreign_key_check`。
    pub foreign_keys_off: bool,
}

/// 全部迁移，必须按 version 升序排列。
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "0001_init",
        sql: include_str!("migrations/0001_init.sql"),
        foreign_keys_off: false,
    },
    Migration {
        version: 2,
        name: "0002_session_created_and_harness_installations",
        sql: include_str!("migrations/0002_session_created_and_harness_installations.sql"),
        foreign_keys_off: true,
    },
    Migration {
        version: 3,
        name: "0003_session_installation_runtime_consistency",
        sql: include_str!("migrations/0003_session_installation_runtime_consistency.sql"),
        foreign_keys_off: true,
    },
    Migration {
        version: 4,
        name: "0004_session_termination_reason",
        sql: include_str!("migrations/0004_session_termination_reason.sql"),
        foreign_keys_off: false,
    },
    Migration {
        version: 5,
        name: "0005_session_pid",
        sql: include_str!("migrations/0005_session_pid.sql"),
        foreign_keys_off: false,
    },
    Migration {
        version: 6,
        name: "0006_usage_imports_and_events",
        sql: include_str!("migrations/0006_usage_imports_and_events.sql"),
        foreign_keys_off: false,
    },
    Migration {
        version: 7,
        name: "0007_usage_import_unattributed_tokens",
        sql: include_str!("migrations/0007_usage_import_unattributed_tokens.sql"),
        foreign_keys_off: false,
    },
    Migration {
        version: 8,
        name: "0008_permission_decisions",
        sql: include_str!("migrations/0008_permission_decisions.sql"),
        foreign_keys_off: false,
    },
    Migration {
        version: 9,
        name: "0009_plugins",
        sql: include_str!("migrations/0009_plugins.sql"),
        foreign_keys_off: false,
    },
    Migration {
        version: 10,
        name: "0010_git_snapshots",
        sql: include_str!("migrations/0010_git_snapshots.sql"),
        foreign_keys_off: false,
    },
];

const CREATE_TRACKING_TABLE: &str = "CREATE TABLE IF NOT EXISTS schema_migrations (
    version    INTEGER PRIMARY KEY,
    name       TEXT NOT NULL,
    applied_at TEXT NOT NULL
)";

fn ensure_tracking_table(conn: &Connection) -> Result<()> {
    conn.execute(CREATE_TRACKING_TABLE, [])?;
    Ok(())
}

/// 当前 schema 版本；空库为 0。
pub fn current_version(conn: &Connection) -> Result<u32> {
    ensure_tracking_table(conn)?;
    let version: Option<u32> =
        conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    Ok(version.unwrap_or(0))
}

/// 已应用的迁移名，按版本升序。
pub fn applied_migrations(conn: &Connection) -> Result<Vec<String>> {
    ensure_tracking_table(conn)?;
    let mut statement = conn.prepare("SELECT name FROM schema_migrations ORDER BY version ASC")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;

    let mut names = Vec::new();
    for row in rows {
        names.push(row?);
    }
    Ok(names)
}

/// 最新 schema 版本（`MIGRATIONS` 里的最大值）。
pub fn latest_version() -> u32 {
    MIGRATIONS
        .iter()
        .map(|item| item.version)
        .max()
        .unwrap_or(0)
}

/// 应用所有尚未执行的迁移，返回本次新应用的名字。
pub fn apply_pending(conn: &mut Connection) -> Result<Vec<String>> {
    apply_migrations(conn, MIGRATIONS, latest_version())
}

/// 应用到指定版本为止。
///
/// 生产代码只用 [`apply_pending`]；这个入口存在的意义是让迁移测试可以
/// 「先建到 v1、塞入旧数据、再升到 v2」，从而验证迁移本身而不是只验证最终 schema。
pub fn apply_until(conn: &mut Connection, target_version: u32) -> Result<Vec<String>> {
    apply_migrations(conn, MIGRATIONS, target_version)
}

/// 迁移序列由参数传入。
///
/// 抽成参数是为了让测试能注入一条**故意失败**的迁移，从而验证
/// `foreign_keys_off` 的失败路径：无论迁移成败，`PRAGMA foreign_keys` 都必须恢复为 ON。
fn apply_migrations(
    conn: &mut Connection,
    migrations: &[Migration],
    target_version: u32,
) -> Result<Vec<String>> {
    let current = current_version(conn)?;
    let mut applied = Vec::new();

    for migration in migrations
        .iter()
        .filter(|item| item.version > current && item.version <= target_version)
    {
        let foreign_keys_was_on = if migration.foreign_keys_off {
            let enabled: i64 = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
            if enabled != 0 {
                conn.pragma_update(None, "foreign_keys", "OFF")?;
                true
            } else {
                false
            }
        } else {
            false
        };

        let outcome = (|| -> Result<()> {
            let transaction = conn.transaction()?;
            transaction.execute_batch(migration.sql).map_err(|error| {
                Error::Migration(format!("{} 执行失败：{error}", migration.name))
            })?;
            transaction.execute(
                "INSERT INTO schema_migrations (version, name, applied_at)
                 VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![migration.version, migration.name],
            )?;
            transaction.commit()?;
            Ok(())
        })();

        if foreign_keys_was_on {
            // 必须**无条件**恢复外键：迁移失败不能让它永久停在 OFF。
            // 这一步失败是严重状态，必须显式报出来，不能静默早退。
            if let Err(restore_error) = conn.pragma_update(None, "foreign_keys", "ON") {
                return Err(Error::Migration(format!(
                    "{} 之后无法恢复外键约束（连接可能停在外键关闭状态）：{restore_error}",
                    migration.name
                )));
            }
        }

        // 先报迁移自身的失败原因，避免被下面的外键检查掩盖。
        outcome?;

        if foreign_keys_was_on {
            // 重建表之后必须确认没有留下悬空引用。
            let violations: i64 =
                conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get(0)
                })?;
            if violations > 0 {
                return Err(Error::Migration(format!(
                    "{} 之后存在 {violations} 条外键违规，迁移不可信",
                    migration.name
                )));
            }
        }

        applied.push(migration.name.to_string());
    }

    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    /// v0.1 核心表清单 —— 少一张都说明 schema 与规格不符。
    const EXPECTED_TABLES: &[&str] = &[
        "runtime_targets",
        "harnesses",
        "harness_installations",
        "projects",
        "project_harness_settings",
        "sessions",
        "turns",
        "messages",
        "models",
        "source_files",
        "usage_imports",
        "usage_events",
        "tool_calls",
        "file_events",
        "git_events",
        "permission_decisions",
        "plugins",
        "git_snapshots",
    ];

    fn table_names(db: &Database) -> Vec<String> {
        let mut statement = db
            .connection()
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .expect("prepare");
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query");
        rows.map(|row| row.expect("row")).collect()
    }

    fn count(db: &Database, sql: &str) -> i64 {
        db.connection()
            .query_row(sql, [], |row| row.get(0))
            .expect("计数")
    }

    #[test]
    fn fresh_database_applies_all_migrations() {
        let db = Database::open_in_memory().expect("打开内存库");

        assert_eq!(db.schema_version().expect("schema 版本"), latest_version());
        assert_eq!(
            applied_migrations(db.connection()).expect("已应用迁移"),
            MIGRATIONS
                .iter()
                .map(|item| item.name.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn migrations_create_every_core_table() {
        let db = Database::open_in_memory().expect("打开内存库");
        let tables = table_names(&db);

        for expected in EXPECTED_TABLES {
            assert!(
                tables.iter().any(|name| name == expected),
                "缺少表 {expected}，实际表：{tables:?}"
            );
        }
    }

    #[test]
    fn applying_pending_twice_is_idempotent() {
        let mut db = Database::open_in_memory().expect("打开内存库");

        let second_run = apply_pending(db.connection_mut()).expect("第二次迁移");

        assert!(second_run.is_empty(), "重复执行不应再应用任何迁移");
        assert_eq!(
            applied_migrations(db.connection())
                .expect("已应用迁移")
                .len(),
            MIGRATIONS.len()
        );
    }

    #[test]
    fn health_reports_pragma_baseline() {
        let db = Database::open_in_memory().expect("打开内存库");
        let health = db.health().expect("健康检查");

        assert!(health.foreign_keys_enabled, "外键约束必须开启");
        assert_eq!(health.schema_version, latest_version());
        assert!(
            health.table_count >= EXPECTED_TABLES.len() as i64,
            "表数量异常：{}",
            health.table_count
        );
    }

    /// 表重建类迁移必须自己收尾干净：外键仍开启，且没有悬空引用。
    #[test]
    fn table_rebuilding_migration_leaves_foreign_keys_on_and_clean() {
        let db = Database::open_in_memory().expect("打开内存库");

        let enabled: i64 = db
            .connection()
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign_keys");
        let violations = count(&db, "SELECT count(*) FROM pragma_foreign_key_check");

        assert_eq!(enabled, 1, "迁移结束后外键必须重新打开");
        assert_eq!(violations, 0, "迁移结束后不得有外键违规");
    }

    /// 0007：`usage_imports` 必须能记录「上游汇总里无法归因到任何模型的 token」。
    ///
    /// 实测依据（ADR-0011 决策十一）：222 行 session 里有 1 行（opencode）行 `totalTokens`
    /// 比它自己的模型明细多 910。这部分不能塞给任何模型，但必须被计数。
    #[test]
    fn migration_0007_adds_the_unattributed_token_counter_with_a_zero_backfill() {
        let mut conn = Connection::open_in_memory().expect("打开内存库");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("外键");

        apply_until(&mut conn, 6).expect("建到 v6");
        conn.execute(
            "INSERT INTO usage_imports (id, source, status, started_at)
             VALUES ('v6-import', 'ccusage', 'succeeded', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("v6 的导入行");

        let applied = apply_until(&mut conn, 7).expect("升到 v7");
        assert_eq!(
            applied,
            vec!["0007_usage_import_unattributed_tokens".to_string()]
        );

        let backfilled: i64 = conn
            .query_row(
                "SELECT unattributed_tokens FROM usage_imports WHERE id = 'v6-import'",
                [],
                |row| row.get(0),
            )
            .expect("老行必须被补成 0 而不是 NULL");
        assert_eq!(backfilled, 0);

        let negative = conn.execute(
            "UPDATE usage_imports SET unattributed_tokens = -1 WHERE id = 'v6-import'",
            [],
        );
        assert!(negative.is_err(), "负数必须被 CHECK 拒绝");
    }

    /// 迁移 0006 之后 `usage_events` 的合法插入语句。
    ///
    /// 刻意用 `occurred_at = NULL` + `occurred_at_source = 'unavailable'`：
    /// 这是「发生时间无法推导」唯一被允许的形态（ADR-0011 决策九）。
    const INSERT_USAGE_EVENT: &str = "INSERT INTO usage_events
        (id, stable_source_key, key_version, source, report_kind, harness, source_session_id,
         model, total_tokens, token_source, occurred_at_source, imported_at)
        VALUES (?1, ?2, 1, 'ccusage', 'session', 'codex', 'rollout-1', 'gpt-5.6-sol', 10,
                'ccusage_source_log', 'unavailable', '2026-01-01T00:00:00Z')";

    /// 迁移 0006 把 ADR-0011 之前的 usage 表重建为新契约。
    ///
    /// 为什么必须重建：0001 的 `usage_events` 是 `dedupe_key` + `cost_usd REAL` + `model_id` FK，
    /// 与「versioned key + 整数微单位 + 模型是自由文本」正面冲突，而已提交的迁移不得修改。
    #[test]
    fn migration_0006_rebuilds_the_usage_tables() {
        let mut conn = Connection::open_in_memory().expect("打开内存库");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("外键");

        apply_until(&mut conn, 5).expect("建到 v5");
        let legacy_columns: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('usage_events')
                 WHERE name IN ('dedupe_key', 'cost_usd', 'model_id')",
                [],
                |row| row.get(0),
            )
            .expect("v5 列检查");
        assert_eq!(legacy_columns, 3, "v5 的 usage_events 还是旧形状");

        let applied = apply_until(&mut conn, 6).expect("升到 v6");
        assert_eq!(applied, vec!["0006_usage_imports_and_events".to_string()]);

        let legacy_table: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'imports'",
                [],
                |row| row.get(0),
            )
            .expect("旧表");
        assert_eq!(legacy_table, 0, "旧的 imports 必须被 usage_imports 取代");

        let event_columns: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('usage_events') WHERE name IN
                 ('stable_source_key','key_version','report_kind','cost_microunits','currency',
                  'currency_source','token_source','cost_source','pricing_mode','occurred_at',
                  'occurred_at_source','cached_input_tokens','cache_creation_tokens',
                  'reasoning_tokens','import_id','raw_payload','provider')",
                [],
                |row| row.get(0),
            )
            .expect("新列检查");
        assert_eq!(event_columns, 17, "ADR-0011 的列必须齐全");

        let import_columns: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('usage_imports') WHERE name IN
                 ('source','source_version','runner','report_kind','status','records_seen',
                  'records_inserted','records_updated','records_skipped','records_timestampless',
                  'started_at','completed_at','error')",
                [],
                |row| row.get(0),
            )
            .expect("usage_imports 列检查");
        assert_eq!(import_columns, 13, "UsageImport 的列必须齐全");

        let enabled: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign_keys");
        let violations: i64 = conn
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .expect("外键检查");
        assert_eq!(enabled, 1);
        assert_eq!(violations, 0);
    }

    /// 0006 是**破坏性重建**：旧表一旦真的有行，迁移必须拒绝执行，而不是静默丢数据。
    ///
    /// 守卫写在 SQL 里（`CHECK (legacy_rows = 0)` + TEMP 表），因此失败会随迁移事务一起回滚。
    #[test]
    fn migration_0006_refuses_to_run_when_the_legacy_usage_table_has_rows() {
        let mut conn = Connection::open_in_memory().expect("打开内存库");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("外键");

        apply_until(&mut conn, 5).expect("建到 v5");
        conn.execute(
            "INSERT INTO harnesses (id, display_name, created_at, updated_at)
             VALUES ('codex', 'Codex', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("harness");
        conn.execute(
            "INSERT INTO usage_events
                (id, dedupe_key, harness_id, source, occurred_at, day, total_tokens, created_at)
             VALUES ('legacy-1', 'legacy-key', 'codex', 'ccusage', '2026-01-01T00:00:00Z',
                     '2026-01-01', 10, '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("旧 usage 行");

        let error = apply_until(&mut conn, 6).expect_err("旧表非空时迁移必须失败");

        assert!(
            error.to_string().contains("0006_usage_imports_and_events"),
            "错误必须指出是哪条迁移失败，实际：{error}"
        );
        assert_eq!(
            current_version(&conn).expect("版本"),
            5,
            "失败的迁移不得推进版本"
        );
        let survivors: i64 = conn
            .query_row("SELECT count(*) FROM usage_events", [], |row| row.get(0))
            .expect("计数");
        assert_eq!(survivors, 1, "拒绝执行时不得丢数据");
    }

    #[test]
    fn usage_events_reject_duplicate_stable_source_key() {
        let db = crate::test_support::empty_db();

        db.connection()
            .execute(INSERT_USAGE_EVENT, params!["a", "key-1"])
            .expect("首次导入");
        let duplicate = db
            .connection()
            .execute(INSERT_USAGE_EVENT, params!["b", "key-1"]);

        assert!(
            duplicate.is_err(),
            "重复 stable_source_key 必须被数据库拒绝"
        );
    }

    #[test]
    fn usage_events_reject_unknown_token_source() {
        let db = crate::test_support::empty_db();

        // 正向对照：合法值必须能写入（否则「拒绝」可能只是因为表/列根本不存在）
        db.connection()
            .execute(INSERT_USAGE_EVENT, params!["ok", "key-ok"])
            .expect("合法 token_source 必须接受");

        let invalid = db.connection().execute(
            "INSERT INTO usage_events
                (id, stable_source_key, key_version, source, report_kind, harness, source_session_id,
                 model, total_tokens, token_source, occurred_at_source, imported_at)
             VALUES ('x', 'key-x', 1, 'ccusage', 'session', 'codex', 'r', 'm', 1,
                     'made-up-source', 'unavailable', '2026-01-01T00:00:00Z')",
            [],
        );

        assert!(invalid.is_err(), "非法 token_source 必须被 CHECK 拒绝");
    }

    /// ADR-0011 决策四：外部聚合器读来的 token **不得**冒充 provider 直接上报。
    #[test]
    fn usage_events_refuse_provider_reported_tokens_from_ccusage() {
        let db = crate::test_support::empty_db();

        db.connection()
            .execute(INSERT_USAGE_EVENT, params!["ok", "key-ok"])
            .expect("合法 token_source 必须接受");

        let masquerading = db.connection().execute(
            "INSERT INTO usage_events
                (id, stable_source_key, key_version, source, report_kind, harness, source_session_id,
                 model, total_tokens, token_source, occurred_at_source, imported_at)
             VALUES ('x', 'key-x', 1, 'ccusage', 'session', 'codex', 'r', 'm', 1,
                     'provider_reported', 'unavailable', '2026-01-01T00:00:00Z')",
            [],
        );

        assert!(
            masquerading.is_err(),
            "ccusage 来的 token 不得标成 provider_reported"
        );
    }

    /// `occurred_at` 与 `occurred_at_source` 必须一致：不允许「有 source 没时间」或反过来。
    #[test]
    fn usage_events_pair_occurred_at_with_its_source() {
        let db = crate::test_support::empty_db();

        db.connection()
            .execute(INSERT_USAGE_EVENT, params!["ok", "key-ok"])
            .expect("unavailable + NULL 是合法组合");

        let claimed_but_missing = db.connection().execute(
            "INSERT INTO usage_events
                (id, stable_source_key, key_version, source, report_kind, harness, source_session_id,
                 model, total_tokens, token_source, occurred_at, occurred_at_source, imported_at)
             VALUES ('x', 'key-x', 1, 'ccusage', 'session', 'codex', 'r', 'm', 1,
                     'ccusage_source_log', NULL, 'source_record', '2026-01-01T00:00:00Z')",
            [],
        );
        assert!(
            claimed_but_missing.is_err(),
            "声称来自源记录却没有时间戳，必须拒绝"
        );

        let undated_but_claimed = db.connection().execute(
            "INSERT INTO usage_events
                (id, stable_source_key, key_version, source, report_kind, harness, source_session_id,
                 model, total_tokens, token_source, occurred_at, occurred_at_source, day, imported_at)
             VALUES ('y', 'key-y', 1, 'ccusage', 'session', 'codex', 'r', 'm', 1,
                     'ccusage_source_log', '2026-01-01T00:00:00Z', 'unavailable', NULL,
                     '2026-01-01T00:00:00Z')",
            [],
        );
        assert!(
            undated_but_claimed.is_err(),
            "有时间戳却标成不可推导，必须拒绝"
        );
    }

    #[test]
    fn sessions_reject_unknown_status() {
        let db = crate::test_support::seeded_db();

        let invalid = db.connection().execute(
            "INSERT INTO sessions
                (hub_session_id, harness_id, runtime_target_id, status, launch_mode, started_at, created_at, updated_at)
             VALUES ('s1', 'codex', 'local', 'not-a-status', 'terminal', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        );

        assert!(invalid.is_err(), "非法 status 必须被 CHECK 约束拒绝");
    }

    /// `created` 是合法状态：登记会话但还没有进程。
    #[test]
    fn sessions_accept_created_status() {
        let db = crate::test_support::seeded_db();

        db.connection()
            .execute(
                "INSERT INTO sessions
                    (hub_session_id, harness_id, runtime_target_id, status, launch_mode, started_at, created_at, updated_at)
                 VALUES ('s-created', 'codex', 'local', 'created', 'terminal', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .expect("created 必须是合法状态");
    }

    /// 迁移测试：先建到 v1、塞入「旧 schema」数据、再升到 v2，验证数据被正确搬运。
    ///
    /// 只验证最终 schema 是不够的 —— 那样搬家逻辑写错了也发现不了。
    ///
    /// 注意必须用**裸 Connection**：`Database::open_*` 会一次性把迁移跑到最新，
    /// 想验证 v1 → v2 的搬运就得自己控制版本。
    #[test]
    fn migration_0002_splits_harnesses_and_backfills_installations() {
        let mut conn = Connection::open_in_memory().expect("打开内存库");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("外键");

        // 建到 v1（模拟老版本用户升级）
        apply_until(&mut conn, 1).expect("建到 v1");
        assert_eq!(current_version(&conn).expect("版本"), 1);

        // v1 的 harnesses 把定义与安装混在一起
        conn.execute(
            "INSERT INTO harnesses
                (id, display_name, installed, binary_path, version, capabilities_json,
                 data_paths_json, detected_at, created_at, updated_at)
             VALUES ('codex', 'Codex', 1, 'D:/npm-global/codex.cmd', '0.152.1', '{\"launch\":true}',
                     '[\"C:/Users/dev/.codex\"]', '2026-09-22T10:00:00Z', '2026-09-01T00:00:00Z', '2026-09-22T10:00:00Z')",
            [],
        )
        .expect("写入 v1 harness");
        conn.execute(
            "INSERT INTO runtime_targets (id, kind, display_name, created_at)
             VALUES ('local', 'local', '本机', '2026-09-01T00:00:00Z')",
            [],
        )
        .expect("写入 runtime target");
        conn.execute(
            "INSERT INTO sessions
                (hub_session_id, harness_id, runtime_target_id, status, launch_mode, cwd, started_at, created_at, updated_at)
             VALUES ('legacy-running', 'codex', 'local', 'running', 'terminal', 'D:/work',
                     '2026-09-22T10:00:00Z', '2026-09-22T10:00:00Z', '2026-09-22T10:00:00Z')",
            [],
        )
        .expect("写入 v1 session");

        // 升级到 v2
        let applied = apply_until(&mut conn, 2).expect("升到 v2");
        assert_eq!(
            applied,
            vec!["0002_session_created_and_harness_installations".to_string()]
        );
        assert_eq!(current_version(&conn).expect("版本"), 2);

        // 定义表只剩身份信息，安装信息已搬到 harness_installations
        let definition_columns: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('harnesses') WHERE name IN
                 ('installed','binary_path','version','detected_at')",
                [],
                |row| row.get(0),
            )
            .expect("列检查");
        assert_eq!(definition_columns, 0, "harnesses 不应再保留安装字段");

        let (installation_id, binary, availability): (String, String, String) = conn
            .query_row(
                "SELECT id, binary_path, availability FROM harness_installations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("安装行必须被搬过来");
        assert_eq!(installation_id, "codex@local");
        assert_eq!(binary, "D:/npm-global/codex.cmd");
        assert_eq!(availability, "available");

        // 旧 session 的 running 其实是「没有进程」，应降级为 created 并关联安装
        let (status, linked_installation): (String, Option<String>) = conn
            .query_row(
                "SELECT status, installation_id FROM sessions WHERE hub_session_id = 'legacy-running'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("旧 session 必须保留");
        assert_eq!(status, "created", "无进程的旧 running 必须降级为 created");
        assert_eq!(linked_installation.as_deref(), Some("codex@local"));

        let sessions: i64 = conn
            .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
            .expect("计数");
        assert_eq!(sessions, 1, "不得丢会话");

        // 表重建之后外键必须仍然开启且无悬空引用
        let enabled: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign_keys");
        let violations: i64 = conn
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .expect("外键检查");
        assert_eq!(enabled, 1);
        assert_eq!(violations, 0);
    }

    /// 迁移 0010：v9 老库升级后 `git_snapshots` 可用，且约束真实生效。
    ///
    /// 必须用裸 `Connection` + `apply_until(9)` 控制版本（AGENTS.md 冷启动纪律）：
    /// 真实用户是从 v9（alpha.4 的 schema）升上来的。
    #[test]
    fn migration_0010_adds_git_snapshots_to_a_v9_database() {
        let mut conn = Connection::open_in_memory().expect("打开内存库");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("外键");

        apply_until(&mut conn, 9).expect("建到 v9");
        // v9 确实还没有这张表 —— 否则下面的断言可能只是「表本来就在」。
        let missing: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'git_snapshots'",
                [],
                |row| row.get(0),
            )
            .expect("v9 表检查");
        assert_eq!(missing, 0);

        // v9 已支持的最小会话行（外键需要 harness / runtime target）。
        conn.execute(
            "INSERT INTO harnesses (id, display_name, created_at, updated_at)
             VALUES ('codex', 'Codex', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("harness");
        conn.execute(
            "INSERT INTO runtime_targets (id, kind, display_name, created_at)
             VALUES ('local', 'local', '本机', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("runtime target");
        conn.execute(
            "INSERT INTO sessions
                (hub_session_id, harness_id, runtime_target_id, status, launch_mode,
                 started_at, created_at, updated_at)
             VALUES ('hub-1', 'codex', 'local', 'created', 'terminal',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("会话");

        let applied = apply_pending(&mut conn).expect("升级到最新");
        assert_eq!(applied, vec!["0010_git_snapshots".to_string()]);
        assert_eq!(current_version(&conn).expect("版本"), 10);

        const INSERT_SNAPSHOT: &str = "INSERT INTO git_snapshots
            (hub_session_id, phase, captured_at, worktree_root, head,
             staged_files, unstaged_files, untracked_files, conflicted_files)
            VALUES (?1, ?2, '2026-01-01T00:00:01Z', 'D:/work/demo', ?3, 1, 2, 3, 0)";

        conn.execute(INSERT_SNAPSHOT, params!["hub-1", "start", "a".repeat(40)])
            .expect("start 快照必须可写");

        // 同一 (session, phase) 只允许一行 —— 终态路径可能重复触发补拍。
        let duplicate = conn.execute(INSERT_SNAPSHOT, params!["hub-1", "start", "b".repeat(40)]);
        assert!(duplicate.is_err(), "重复 (session, phase) 必须被主键拒绝");

        let bad_phase = conn.execute(INSERT_SNAPSHOT, params!["hub-1", "middle", "c".repeat(40)]);
        assert!(bad_phase.is_err(), "非法 phase 必须被 CHECK 拒绝");

        let orphan = conn.execute(INSERT_SNAPSHOT, params!["nope", "end", "d".repeat(40)]);
        assert!(orphan.is_err(), "不存在的会话必须被外键拒绝");

        let negative = conn.execute(
            "INSERT INTO git_snapshots
                (hub_session_id, phase, captured_at, worktree_root, head,
                 staged_files, unstaged_files, untracked_files, conflicted_files)
             VALUES ('hub-1', 'end', '2026-01-01T00:00:02Z', 'D:/work/demo', NULL, -1, 0, 0, 0)",
            [],
        );
        assert!(negative.is_err(), "负数计数必须被 CHECK 拒绝");

        // 合法的 unborn HEAD（空仓库）为 NULL。
        conn.execute(
            "INSERT INTO git_snapshots
                (hub_session_id, phase, captured_at, worktree_root, head,
                 staged_files, unstaged_files, untracked_files, conflicted_files)
             VALUES ('hub-1', 'end', '2026-01-01T00:05:00Z', 'D:/work/demo', NULL, 0, 0, 0, 0)",
            [],
        )
        .expect("unborn HEAD 快照必须可写");

        // 会话删除时快照随之消失：快照不允许比会话活得久。
        conn.execute("DELETE FROM sessions WHERE hub_session_id = 'hub-1'", [])
            .expect("删除会话");
        let remaining: i64 = conn
            .query_row("SELECT count(*) FROM git_snapshots", [], |row| row.get(0))
            .expect("计数");
        assert_eq!(remaining, 0, "会话删除后快照必须级联删除");
    }

    /// 故意失败的迁移：先建一张表、插一行，再访问不存在的表。
    const BROKEN_MIGRATION: Migration = Migration {
        version: 99,
        name: "0099_broken",
        sql: "CREATE TABLE broken_marker (id TEXT NOT NULL);
              INSERT INTO broken_marker (id) VALUES ('x');
              INSERT INTO definitely_missing_table (id) VALUES (1);",
        foreign_keys_off: true,
    };

    /// **失败路径回归测试**（最容易出事的地方）。
    ///
    /// 表重建必须在事务外关外键；如果失败时忘了恢复，连接就会永久停在
    /// `foreign_keys = OFF` —— 之后所有外键约束静默失效。
    #[test]
    fn a_failed_table_rebuilding_migration_still_restores_foreign_keys() {
        let mut conn = Connection::open_in_memory().expect("打开内存库");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("外键");

        let migrations = [
            Migration {
                version: 1,
                name: "0001_init",
                sql: include_str!("migrations/0001_init.sql"),
                foreign_keys_off: false,
            },
            BROKEN_MIGRATION,
        ];

        let error = apply_migrations(&mut conn, &migrations, 99).expect_err("必须失败");

        assert!(
            error.to_string().contains("0099_broken"),
            "错误信息必须指出是哪条迁移失败，实际：{error}"
        );

        let enabled: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign_keys");
        assert_eq!(enabled, 1, "迁移失败后外键必须恢复 ON");

        // 迁移整体回滚：不得留下半截 schema，也不得写入版本号
        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .expect("版本");
        assert_eq!(version, 1, "失败的迁移不得写进 schema_migrations");

        let marker: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'broken_marker'",
                [],
                |row| row.get(0),
            )
            .expect("标记表");
        assert_eq!(marker, 0, "失败迁移建的表必须随事务回滚");

        // 不只是「声明为 ON」，还要真的在生效
        let violating = conn.execute(
            "INSERT INTO sessions
                (hub_session_id, harness_id, runtime_target_id, status, launch_mode, started_at, created_at, updated_at)
             VALUES ('x', 'does-not-exist', 'local', 'created', 'terminal', 't', 't', 't')",
            [],
        );
        assert!(violating.is_err(), "外键必须真的重新生效");
    }

    /// 组合一致性：installation 与 runtime target 必须属于同一个 runtime。
    #[test]
    fn sessions_reject_an_installation_from_a_different_runtime_target() {
        let db = crate::test_support::empty_db();
        let conn = db.connection();

        conn.execute(
            "INSERT INTO harnesses (id, display_name, created_at, updated_at)
             VALUES ('codex', 'Codex', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("harness 定义");
        for target in ["local", "wsl"] {
            conn.execute(
                "INSERT INTO runtime_targets (id, kind, display_name, created_at)
                 VALUES (?1, 'local', ?1, '2026-01-01T00:00:00Z')",
                params![target],
            )
            .expect("runtime target");
            conn.execute(
                "INSERT INTO harness_installations
                    (id, harness_id, runtime_target_id, availability, first_detected_at, last_seen_at)
                 VALUES (?1, 'codex', ?2, 'available', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                params![format!("codex@{target}"), target],
            )
            .expect("installation");
        }

        // 两个外键各自都合法（codex@local 存在、wsl 存在），但组合矛盾
        let contradictory = conn.execute(
            "INSERT INTO sessions
                (hub_session_id, harness_id, installation_id, runtime_target_id, status, launch_mode, started_at, created_at, updated_at)
             VALUES ('bad', 'codex', 'codex@local', 'wsl', 'created', 'terminal', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        );
        assert!(
            contradictory.is_err(),
            "installation 属于 local，却把 session 挂到 wsl —— 必须被复合外键拒绝"
        );

        // 一致的组合仍然可以写入
        conn.execute(
            "INSERT INTO sessions
                (hub_session_id, harness_id, installation_id, runtime_target_id, status, launch_mode, started_at, created_at, updated_at)
             VALUES ('good', 'codex', 'codex@local', 'local', 'created', 'terminal', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("一致的组合必须允许");
    }

    /// 导入的历史会话可以没有 installation（复合外键在含 NULL 时视为满足）。
    #[test]
    fn sessions_accept_a_null_installation_id() {
        let db = crate::test_support::seeded_db();

        db.connection()
            .execute(
                "INSERT INTO sessions
                    (hub_session_id, harness_id, installation_id, runtime_target_id, status, launch_mode, started_at, created_at, updated_at)
                 VALUES ('imported', 'codex', NULL, 'local', 'created', 'imported', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .expect("没有 installation 的会话必须允许");
    }

    /// 迁移 0004：能可靠反推的存量终态行必须补上 termination_reason，
    /// 分不清原因的（旧 'failed'）保持 NULL 表示「原因未记录」，而不是硬塞一个值。
    #[test]
    fn migration_0004_backfills_only_the_reasons_it_can_derive() {
        let mut conn = Connection::open_in_memory().expect("打开内存库");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("外键");

        apply_until(&mut conn, 3).expect("建到 v3");
        conn.execute(
            "INSERT INTO harnesses (id, display_name, created_at, updated_at)
             VALUES ('codex', 'Codex', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("harness");
        conn.execute(
            "INSERT INTO runtime_targets (id, kind, display_name, created_at)
             VALUES ('local', 'local', '本机', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("runtime target");
        for (id, status) in [
            ("legacy-exited", "exited"),
            ("legacy-failed", "failed"),
            ("legacy-unknown", "unknown"),
            ("legacy-running", "running"),
        ] {
            conn.execute(
                "INSERT INTO sessions
                    (hub_session_id, harness_id, installation_id, runtime_target_id, status, launch_mode, started_at, created_at, updated_at)
                 VALUES (?1, 'codex', NULL, 'local', ?2, 'terminal', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                params![id, status],
            )
            .expect("旧会话");
        }

        apply_until(&mut conn, 4).expect("升到 v4");

        let reason_of = |id: &str| -> Option<String> {
            conn.query_row(
                "SELECT termination_reason FROM sessions WHERE hub_session_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .expect("读取原因")
        };

        assert_eq!(
            reason_of("legacy-exited").as_deref(),
            Some("natural_exit"),
            "正常结束可以可靠反推"
        );
        assert_eq!(
            reason_of("legacy-unknown").as_deref(),
            Some("lost"),
            "没有终态信息的算 lost"
        );
        assert_eq!(
            reason_of("legacy-failed"),
            None,
            "旧的 failed 分不清启动失败还是运行失败 —— 必须留 NULL 而不是猜"
        );
        assert_eq!(reason_of("legacy-running"), None, "running 没有终止原因");
    }

    /// 表重建迁移不得在最终 schema 里留下临时表名。
    #[test]
    fn migrated_schema_has_no_leftover_temporary_names() {
        let db = crate::test_support::empty_db();

        for table in ["sessions", "harnesses"] {
            let sql: String = db
                .connection()
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    params![table],
                    |row| row.get(0),
                )
                .expect("读取表定义");
            assert!(
                !sql.contains("_new"),
                "{table} 的表定义里还残留临时表名：{sql}"
            );
        }

        let sessions_sql: String = db
            .connection()
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'sessions'",
                [],
                |row| row.get(0),
            )
            .expect("读取 sessions");
        assert!(
            sessions_sql.contains("harness_installations"),
            "sessions 必须保留指向 harness_installations 的复合外键：{sessions_sql}"
        );
    }

    /// 0008 + 0009：**老库升级路径**（AGENTS.md 冷启动纪律）。
    ///
    /// 必须用裸 `Connection` + `apply_until` 控制版本：`Database::open_*` 会一次性把迁移
    /// 跑到最新，那样只能验证最终 schema —— 而真实用户是从 v7 升上来的。
    /// 这条用例同时是「插件两条迁移被正确注册、按序执行」的证据；后续迁移
    /// （0010 起）出现在断言里即代表它们同样落在这条升级路径上。
    #[test]
    fn migration_0008_and_0009_upgrade_a_v7_database_to_latest() {
        let mut conn = Connection::open_in_memory().expect("打开内存库");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("外键");

        // 模拟已发布的 v7 用户库：只有 0001..0007 的内容。
        apply_until(&mut conn, 7).expect("建到 v7");
        assert_eq!(current_version(&conn).expect("版本"), 7);
        // 老库里这两张表**确实还不存在** —— 否则下面的断言可能只是「表本来就在」。
        let plugin_tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'table'
                 AND name IN ('permission_decisions', 'plugins')",
                [],
                |row| row.get(0),
            )
            .expect("v7 表检查");
        assert_eq!(plugin_tables, 0, "v7 不该有插件相关表");

        // 用生产入口升级，顺序与命名都必须可核对。
        let applied = apply_pending(&mut conn).expect("升级到最新");
        assert_eq!(
            applied,
            vec![
                "0008_permission_decisions".to_string(),
                "0009_plugins".to_string(),
                // 0010 起的后续迁移同样在这条 v7 升级路径上按序执行；
                // 每新增一条迁移都要在这里追加（断言过期就说明这条测试没跟上）。
                "0010_git_snapshots".to_string(),
            ]
        );
        assert_eq!(current_version(&conn).expect("版本"), 10);

        // 升级之后的库必须真的可用，而不只是「表存在」。
        conn.execute(
            "INSERT INTO plugins
                (id, name, version, api_version, path, capabilities_json,
                 skills_cache_json, status, registered_at)
             VALUES ('usage-connect', 'Usage Connect', '0.1.0', '1', 'D:/plugins/usage-connect',
                     '{\"usage\":true}', NULL, 'ready', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("升级后必须能写插件行");
        conn.execute(
            "INSERT INTO permission_decisions (plugin_id, api, decision, reason, args_json, created_at)
             VALUES ('usage-connect', 'host/log', 'observed', NULL, '{}', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("升级后必须能写决策行");

        // CHECK 约束真的在生效（不是「列名存在」而已）。
        let bad_status = conn.execute(
            "INSERT INTO plugins
                (id, name, version, api_version, path, capabilities_json,
                 skills_cache_json, status, registered_at)
             VALUES ('bad', 'Bad', '0.1.0', '1', 'D:/p/bad', '{}', NULL, 'enabled', 't')",
            [],
        );
        assert!(bad_status.is_err(), "非法 status 必须被 CHECK 拒绝");

        let bad_decision = conn.execute(
            "INSERT INTO permission_decisions (plugin_id, api, decision, args_json, created_at)
             VALUES ('usage-connect', 'host/log', 'allowed', '{}', 't')",
            [],
        );
        assert!(
            bad_decision.is_err(),
            "v0.1 的 decision 只允许 'observed'，'allowed' 必须被拒绝"
        );

        // 没有外键违规：这两张表是故意不加外键的，但不该顺手破坏别处。
        let violations: i64 = conn
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .expect("外键检查");
        assert_eq!(violations, 0);
    }

    /// 卸载插件后，它当年的权限决策必须留下（这就是 0008 不加外键的理由）。
    ///
    /// 如果 `permission_decisions.plugin_id` 是 `REFERENCES plugins(id)`，
    /// 这条断言在 CASCADE 下会悄悄清零、在 RESTRICT 下插件根本删不掉。
    #[test]
    fn permission_decisions_survive_plugin_uninstall() {
        let db = crate::test_support::empty_db();
        let conn = db.connection();

        conn.execute(
            "INSERT INTO plugins
                (id, name, version, api_version, path, capabilities_json,
                 skills_cache_json, status, registered_at)
             VALUES ('hello-skill', 'Hello Skill', '0.1.0', '1', 'D:/plugins/hello-skill',
                     '{\"skills\":true}', '[]', 'ready', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("登记插件");
        conn.execute(
            "INSERT INTO permission_decisions (plugin_id, api, decision, args_json, created_at)
             VALUES ('hello-skill', 'host/log', 'observed', '{\"message\":\"hi\"}',
                     '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("记录决策");

        conn.execute("DELETE FROM plugins WHERE id = 'hello-skill'", [])
            .expect("插件可被卸载（有审计记录也不该被拦）");

        let survivors: i64 = conn
            .query_row(
                "SELECT count(*) FROM permission_decisions WHERE plugin_id = 'hello-skill'",
                [],
                |row| row.get(0),
            )
            .expect("计数");
        assert_eq!(survivors, 1, "卸载插件不得销毁审计证据");
    }
}
