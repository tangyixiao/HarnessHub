//! Session 持久化。
//!
//! 硬约定（docs/adr/0004）：`hub_session_id` 是 Harness Hub 自己的全局标识，
//! `source_session_id` 是外部 Harness 的原始标识，两者绝不可混用；
//! `(harness_id, source_session_id)` 唯一，是导入幂等的基础。

use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};

use crate::error::Result;

const SELECT_COLUMNS: &str = "hub_session_id, source_session_id, harness_id, project_id, \
     runtime_target_id, parent_session_id, status, launch_mode, cwd, worktree_path, \
     started_at, ended_at, exit_code";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    Exited,
    Failed,
    /// 进程已消失但拿不到退出码，或来源数据没给出终态。
    Unknown,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    fn from_db(value: &str) -> Self {
        match value {
            "running" => Self::Running,
            "exited" => Self::Exited,
            "failed" => Self::Failed,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    /// 在 Harness Hub 内新建 PTY 会话。
    Terminal,
    /// 恢复已有会话。
    Resume,
    /// 从外部数据导入的历史会话。
    Imported,
}

impl LaunchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Resume => "resume",
            Self::Imported => "imported",
        }
    }

    fn from_db(value: &str) -> Self {
        match value {
            "resume" => Self::Resume,
            "imported" => Self::Imported,
            _ => Self::Terminal,
        }
    }
}

/// 新建会话的输入。插入时 status 固定为 `running`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSession {
    pub hub_session_id: String,
    pub source_session_id: Option<String>,
    pub harness_id: String,
    pub project_id: Option<String>,
    pub runtime_target_id: String,
    pub parent_session_id: Option<String>,
    pub launch_mode: LaunchMode,
    pub cwd: Option<String>,
    pub worktree_path: Option<String>,
    pub started_at: String,
}

impl NewSession {
    /// 最小必填项构造；其余字段默认「无」，由调用方按需补齐。
    pub fn new(
        hub_session_id: impl Into<String>,
        harness_id: impl Into<String>,
        runtime_target_id: impl Into<String>,
        started_at: impl Into<String>,
    ) -> Self {
        Self {
            hub_session_id: hub_session_id.into(),
            source_session_id: None,
            harness_id: harness_id.into(),
            project_id: None,
            runtime_target_id: runtime_target_id.into(),
            parent_session_id: None,
            launch_mode: LaunchMode::Terminal,
            cwd: None,
            worktree_path: None,
            started_at: started_at.into(),
        }
    }

    pub fn with_source_session_id(mut self, source_session_id: impl Into<String>) -> Self {
        self.source_session_id = Some(source_session_id.into());
        self
    }

    pub fn with_project_id(mut self, project_id: impl Into<String>) -> Self {
        self.project_id = Some(project_id.into());
        self
    }

    pub fn with_cwd(mut self, cwd: impl Into<String>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn with_launch_mode(mut self, launch_mode: LaunchMode) -> Self {
        self.launch_mode = launch_mode;
        self
    }
}

/// 数据库中的一条会话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub hub_session_id: String,
    pub source_session_id: Option<String>,
    pub harness_id: String,
    pub project_id: Option<String>,
    pub runtime_target_id: String,
    pub parent_session_id: Option<String>,
    pub status: SessionStatus,
    pub launch_mode: LaunchMode,
    pub cwd: Option<String>,
    pub worktree_path: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub exit_code: Option<i32>,
}

/// Session 表的读写入口。
pub struct SessionStore<'conn> {
    conn: &'conn Connection,
}

impl<'conn> SessionStore<'conn> {
    pub fn new(conn: &'conn Connection) -> Self {
        Self { conn }
    }

    /// 写入一条新会话。重复的 `(harness_id, source_session_id)` 会被数据库唯一索引拒绝。
    pub fn insert(&self, session: &NewSession) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sessions (
                hub_session_id, source_session_id, harness_id, project_id, runtime_target_id,
                parent_session_id, status, launch_mode, cwd, worktree_path,
                started_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8, ?9, ?10, ?10, ?10)",
            params![
                session.hub_session_id,
                session.source_session_id,
                session.harness_id,
                session.project_id,
                session.runtime_target_id,
                session.parent_session_id,
                session.launch_mode.as_str(),
                session.cwd,
                session.worktree_path,
                session.started_at,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, hub_session_id: &str) -> Result<Option<SessionRecord>> {
        let sql = format!("SELECT {SELECT_COLUMNS} FROM sessions WHERE hub_session_id = ?1");
        let mut statement = self.conn.prepare(&sql)?;
        let mut rows = statement.query_map(params![hub_session_id], map_session_row)?;

        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// 最近会话，按开始时间倒序。
    pub fn list_recent(&self, limit: u32) -> Result<Vec<SessionRecord>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM sessions
             ORDER BY started_at DESC, hub_session_id ASC LIMIT ?1"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params![i64::from(limit)], map_session_row)?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    pub fn count(&self) -> Result<i64> {
        let count = self
            .conn
            .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))?;
        Ok(count)
    }

    /// 结束一次仍处于 `running` 的会话。
    ///
    /// `exit_code` 为 `Some(0)` 记为 exited，非 0 记为 failed，缺失则记为 unknown。
    /// 返回是否真的更新了行（幂等：重复调用返回 `false`）。
    pub fn finish(
        &self,
        hub_session_id: &str,
        exit_code: Option<i32>,
        ended_at: &str,
    ) -> Result<bool> {
        let status = match exit_code {
            Some(0) => SessionStatus::Exited,
            Some(_) => SessionStatus::Failed,
            None => SessionStatus::Unknown,
        };

        let updated = self.conn.execute(
            "UPDATE sessions
                SET status = ?2, ended_at = ?3, exit_code = ?4, updated_at = ?3
              WHERE hub_session_id = ?1 AND status = 'running'",
            params![hub_session_id, status.as_str(), ended_at, exit_code],
        )?;

        Ok(updated > 0)
    }
}

fn map_session_row(row: &Row<'_>) -> rusqlite::Result<SessionRecord> {
    let status: String = row.get(6)?;
    let launch_mode: String = row.get(7)?;

    Ok(SessionRecord {
        hub_session_id: row.get(0)?,
        source_session_id: row.get(1)?,
        harness_id: row.get(2)?,
        project_id: row.get(3)?,
        runtime_target_id: row.get(4)?,
        parent_session_id: row.get(5)?,
        status: SessionStatus::from_db(&status),
        launch_mode: LaunchMode::from_db(&launch_mode),
        cwd: row.get(8)?,
        worktree_path: row.get(9)?,
        started_at: row.get(10)?,
        ended_at: row.get(11)?,
        exit_code: row.get(12)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{seeded_db, HARNESS_ID, RUNTIME_TARGET_ID};

    fn session(id: &str, started_at: &str) -> NewSession {
        NewSession::new(id, HARNESS_ID, RUNTIME_TARGET_ID, started_at)
    }

    #[test]
    fn insert_then_get_roundtrip() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());

        let new_session = session("hub-1", "2026-01-01T10:00:00Z")
            .with_source_session_id("codex-abc")
            .with_cwd("D:/work/project");
        store.insert(&new_session).expect("插入会话");

        let stored = store.get("hub-1").expect("查询").expect("应存在");

        assert_eq!(stored.source_session_id.as_deref(), Some("codex-abc"));
        assert_eq!(stored.status, SessionStatus::Running);
        assert_eq!(stored.launch_mode, LaunchMode::Terminal);
        assert_eq!(stored.cwd.as_deref(), Some("D:/work/project"));
        assert_eq!(stored.runtime_target_id, RUNTIME_TARGET_ID);
        assert!(stored.ended_at.is_none());
    }

    #[test]
    fn get_missing_session_returns_none() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());

        assert!(store.get("nope").expect("查询").is_none());
    }

    #[test]
    fn list_recent_orders_by_started_at_desc() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("old", "2026-01-01T09:00:00Z"))
            .expect("插入");
        store
            .insert(&session("newest", "2026-01-01T11:00:00Z"))
            .expect("插入");
        store
            .insert(&session("middle", "2026-01-01T10:00:00Z"))
            .expect("插入");

        let ids: Vec<String> = store
            .list_recent(10)
            .expect("列出")
            .into_iter()
            .map(|record| record.hub_session_id)
            .collect();

        assert_eq!(ids, vec!["newest", "middle", "old"]);
        assert_eq!(store.count().expect("计数"), 3);
    }

    #[test]
    fn list_recent_honours_limit() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("a", "2026-01-01T09:00:00Z"))
            .expect("插入");
        store
            .insert(&session("b", "2026-01-01T10:00:00Z"))
            .expect("插入");

        assert_eq!(store.list_recent(1).expect("列出").len(), 1);
    }

    #[test]
    fn same_source_session_id_cannot_be_imported_twice() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        let first = session("hub-1", "2026-01-01T10:00:00Z").with_source_session_id("codex-abc");
        store.insert(&first).expect("首次插入");

        let duplicate =
            session("hub-2", "2026-01-01T11:00:00Z").with_source_session_id("codex-abc");
        let result = store.insert(&duplicate);

        assert!(
            result.is_err(),
            "同一 Harness 的同一 source session 不得重复入库"
        );
        assert_eq!(store.count().expect("计数"), 1);
    }

    #[test]
    fn sessions_without_source_id_are_not_deduplicated() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());

        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");
        store
            .insert(&session("hub-2", "2026-01-01T11:00:00Z"))
            .expect("插入");

        assert_eq!(store.count().expect("计数"), 2);
    }

    #[test]
    fn finish_marks_exit_status_and_time() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");

        assert!(store
            .finish("hub-1", Some(0), "2026-01-01T10:05:00Z")
            .expect("结束"));

        let stored = store.get("hub-1").expect("查询").expect("应存在");
        assert_eq!(stored.status, SessionStatus::Exited);
        assert_eq!(stored.ended_at.as_deref(), Some("2026-01-01T10:05:00Z"));
        assert_eq!(stored.exit_code, Some(0));
    }

    #[test]
    fn finish_with_nonzero_exit_code_marks_failed() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");

        store
            .finish("hub-1", Some(130), "2026-01-01T10:05:00Z")
            .expect("结束");

        assert_eq!(
            store.get("hub-1").expect("查询").expect("应存在").status,
            SessionStatus::Failed
        );
    }

    #[test]
    fn finish_without_exit_code_marks_unknown() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");

        store
            .finish("hub-1", None, "2026-01-01T10:05:00Z")
            .expect("结束");

        assert_eq!(
            store.get("hub-1").expect("查询").expect("应存在").status,
            SessionStatus::Unknown
        );
    }

    #[test]
    fn finish_is_idempotent() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");

        assert!(store
            .finish("hub-1", Some(0), "2026-01-01T10:05:00Z")
            .expect("第一次"));
        assert!(!store
            .finish("hub-1", Some(0), "2026-01-01T10:09:00Z")
            .expect("第二次"));
        assert_eq!(
            store
                .get("hub-1")
                .expect("查询")
                .expect("应存在")
                .ended_at
                .as_deref(),
            Some("2026-01-01T10:05:00Z"),
            "重复结束不得改写结束时间"
        );
    }
}
