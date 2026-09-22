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
    apply_until(conn, latest_version())
}

/// 应用到指定版本为止。
///
/// 生产代码只用 [`apply_pending`]；这个入口存在的意义是让迁移测试可以
/// 「先建到 v1、塞入旧数据、再升到 v2」，从而验证迁移本身而不是只验证最终 schema。
pub fn apply_until(conn: &mut Connection, target_version: u32) -> Result<Vec<String>> {
    let current = current_version(conn)?;
    let mut applied = Vec::new();

    for migration in MIGRATIONS
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
            conn.pragma_update(None, "foreign_keys", "ON")?;

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

        outcome?;
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
        "imports",
        "usage_events",
        "tool_calls",
        "file_events",
        "git_events",
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

    #[test]
    fn usage_events_reject_duplicate_dedupe_key() {
        let db = crate::test_support::seeded_db();

        let insert = "INSERT INTO usage_events
             (id, dedupe_key, harness_id, source, occurred_at, day, total_tokens, created_at)
             VALUES (?1, 'dup', 'codex', 'ccusage', '2026-01-01T00:00:00Z', '2026-01-01', 10, '2026-01-01T00:00:00Z')";

        db.connection()
            .execute(insert, params!["a"])
            .expect("首次导入");
        let duplicate = db.connection().execute(insert, params!["b"]);

        assert!(duplicate.is_err(), "重复 dedupe_key 必须被数据库拒绝");
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
}
