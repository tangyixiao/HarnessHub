//! SQLite 主库：连接基线、迁移执行与健康自检。
//!
//! 数据库由 Rust Control Plane 独占持有；Python sidecar 不得直连
//! （见 docs/adr/0004-sqlite-strategy.md 与 AGENTS.md）。

pub mod migrations;

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;
use serde::Serialize;

use crate::error::Result;

/// 等待锁的最长时间。SQLite 只有一个写者，宁可稍等也不要直接失败。
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// 迁移完成后的健康快照，直接透给 Dashboard 作为「链路已通」的证据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbHealth {
    pub schema_version: u32,
    pub applied_migrations: Vec<String>,
    pub table_count: i64,
    pub foreign_keys_enabled: bool,
    pub journal_mode: String,
}

/// Harness Hub 主数据库句柄。
pub struct Database {
    conn: Connection,
}

impl Database {
    /// 打开（或创建）磁盘上的数据库，并应用所有未执行的迁移。
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// 内存数据库：仅用于测试。
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self> {
        let mut conn = conn;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        // journal_mode 会返回一行结果，必须用 query_row；pragma_update 只适用于无返回值的 pragma。
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrations::apply_pending(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn connection_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    pub fn schema_version(&self) -> Result<u32> {
        migrations::current_version(&self.conn)
    }

    /// 读取当前数据库状态。用于 Dashboard 与「崩溃后数据库是否完好」自检。
    pub fn health(&self) -> Result<DbHealth> {
        let schema_version = migrations::current_version(&self.conn)?;
        let applied_migrations = migrations::applied_migrations(&self.conn)?;

        let table_count: i64 = self.conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get(0),
        )?;

        let foreign_keys: i64 = self
            .conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;

        let journal_mode: String = self
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))?;

        Ok(DbHealth {
            schema_version,
            applied_migrations,
            table_count,
            foreign_keys_enabled: foreign_keys != 0,
            journal_mode,
        })
    }
}
