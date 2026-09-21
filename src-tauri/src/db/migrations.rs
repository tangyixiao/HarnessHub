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
}

/// 全部迁移，必须按 version 升序排列。
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "0001_init",
    sql: include_str!("migrations/0001_init.sql"),
}];

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

/// 应用所有尚未执行的迁移，返回本次新应用的名字。
pub fn apply_pending(conn: &mut Connection) -> Result<Vec<String>> {
    let current = current_version(conn)?;
    let mut applied = Vec::new();

    for migration in MIGRATIONS.iter().filter(|item| item.version > current) {
        let transaction = conn.transaction()?;
        transaction
            .execute_batch(migration.sql)
            .map_err(|error| Error::Migration(format!("{} 执行失败：{error}", migration.name)))?;
        transaction.execute(
            "INSERT INTO schema_migrations (version, name, applied_at)
             VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![migration.version, migration.name],
        )?;
        transaction.commit()?;
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

    #[test]
    fn fresh_database_applies_all_migrations() {
        let db = Database::open_in_memory().expect("打开内存库");

        assert_eq!(db.schema_version().expect("schema 版本"), 1);
        assert_eq!(
            applied_migrations(db.connection()).expect("已应用迁移"),
            vec!["0001_init".to_string()]
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
            1
        );
    }

    #[test]
    fn health_reports_pragma_baseline() {
        let db = Database::open_in_memory().expect("打开内存库");
        let health = db.health().expect("健康检查");

        assert!(health.foreign_keys_enabled, "外键约束必须开启");
        assert_eq!(health.schema_version, 1);
        assert!(
            health.table_count >= EXPECTED_TABLES.len() as i64,
            "表数量异常：{}",
            health.table_count
        );
    }

    #[test]
    fn usage_events_reject_duplicate_dedupe_key() {
        let db = Database::open_in_memory().expect("打开内存库");
        let conn = db.connection();

        conn.execute(
            "INSERT INTO harnesses (id, display_name, created_at, updated_at)
             VALUES ('codex', 'Codex', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("插入 harness");

        let insert = "INSERT INTO usage_events
             (id, dedupe_key, harness_id, source, occurred_at, day, total_tokens, created_at)
             VALUES (?1, 'dup', 'codex', 'ccusage', '2026-01-01T00:00:00Z', '2026-01-01', 10, '2026-01-01T00:00:00Z')";

        conn.execute(insert, params!["a"]).expect("首次导入");
        let duplicate = conn.execute(insert, params!["b"]);

        assert!(duplicate.is_err(), "重复 dedupe_key 必须被数据库拒绝");
    }

    #[test]
    fn sessions_reject_unknown_status() {
        let db = Database::open_in_memory().expect("打开内存库");
        let conn = db.connection();

        conn.execute(
            "INSERT INTO harnesses (id, display_name, created_at, updated_at)
             VALUES ('codex', 'Codex', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("插入 harness");
        conn.execute(
            "INSERT INTO runtime_targets (id, kind, display_name, created_at)
             VALUES ('local', 'local', '本机', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("插入 runtime target");

        let invalid = conn.execute(
            "INSERT INTO sessions
                (hub_session_id, harness_id, runtime_target_id, status, launch_mode, started_at, created_at, updated_at)
             VALUES ('s1', 'codex', 'local', 'not-a-status', 'terminal', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        );

        assert!(invalid.is_err(), "非法 status 必须被 CHECK 约束拒绝");
    }
}
