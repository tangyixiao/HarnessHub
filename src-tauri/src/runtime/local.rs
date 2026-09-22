//! v0.1 唯一的运行目标：本机。
//!
//! 为什么放进表里而不是硬编码常量：ADR-0021 要求所有 Harness 运行都关联
//! `runtime_target_id`，接口必须为远程 / 容器运行时预留。

use rusqlite::{params, Connection};

use crate::error::Result;

/// v0.1 唯一的运行目标 id。
pub const LOCAL_TARGET_ID: &str = "local";

/// 确保 `local` 运行目标存在。
///
/// 幂等：重复调用只更新展示名，不会插入第二行，也能容忍已经存在的行
/// （例如旧版本写入过、或用户手工插入过）。
pub fn ensure_local_target(conn: &Connection) -> Result<String> {
    conn.execute(
        "INSERT INTO runtime_targets (id, kind, display_name, created_at)
         VALUES (?1, 'local', '本机', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         ON CONFLICT (id) DO UPDATE SET display_name = excluded.display_name",
        params![LOCAL_TARGET_ID],
    )?;

    Ok(LOCAL_TARGET_ID.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::empty_db;

    #[test]
    fn creates_the_local_target_on_an_empty_database() {
        let db = empty_db();

        let id = ensure_local_target(db.connection()).expect("引导 runtime target");

        assert_eq!(id, LOCAL_TARGET_ID);
        let (kind, display_name): (String, String) = db
            .connection()
            .query_row(
                "SELECT kind, display_name FROM runtime_targets WHERE id = ?1",
                [LOCAL_TARGET_ID],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("local target 必须存在");
        assert_eq!(kind, "local");
        assert_eq!(display_name, "本机");
    }

    #[test]
    fn is_idempotent() {
        let db = empty_db();

        ensure_local_target(db.connection()).expect("首次引导");
        ensure_local_target(db.connection()).expect("重复引导");

        let count: i64 = db
            .connection()
            .query_row("SELECT count(*) FROM runtime_targets", [], |row| row.get(0))
            .expect("计数");
        assert_eq!(count, 1, "重复引导不得插入第二行");
    }

    #[test]
    fn works_on_a_database_that_already_has_the_local_target() {
        // seeded_db 已经插入过 id = local 的 runtime target（模拟升级/重启）。
        let db = crate::test_support::seeded_db();

        let id = ensure_local_target(db.connection()).expect("应容忍已存在的行");

        assert_eq!(id, LOCAL_TARGET_ID);
        let count: i64 = db
            .connection()
            .query_row(
                "SELECT count(*) FROM runtime_targets WHERE id = ?1",
                [LOCAL_TARGET_ID],
                |row| row.get(0),
            )
            .expect("计数");
        assert_eq!(count, 1);
    }
}
