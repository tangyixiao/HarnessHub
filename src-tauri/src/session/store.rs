//! Session 持久化与状态机。
//!
//! 硬约定（docs/adr/0004）：`hub_session_id` 是 Harness Hub 自己的全局标识，
//! `source_session_id` 是外部 Harness 的原始标识，两者绝不可混用；
//! `(harness_id, source_session_id)` 唯一，是导入幂等的基础。
//!
//! 状态机（docs/adr/0006）—— **没有进程就不能是 running**：
//!
//! ```text
//! created ──mark_running──▶ running ──finish(exit, reason)──▶ exited / failed / unknown
//!    └──fail(launch_failed)──▶ failed
//! ```
//!
//! `unknown` 只用于「拿不到退出码」「宿主关闭」「失去联系」。
//! 终态还额外记录 **termination_reason**（为什么结束），与 `exit_code` 正交 ——
//! 非零退出不等于同一种失败（见 migration 0004 与 `TerminationReason`）。

use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};

use crate::error::Result;

const SELECT_COLUMNS: &str = "hub_session_id, source_session_id, harness_id, installation_id, \
     project_id, runtime_target_id, parent_session_id, status, launch_mode, cwd, worktree_path, \
     started_at, ended_at, exit_code, termination_reason";

/// **为什么**会话结束了。与 `exit_code` 正交（见 migration 0004）。
///
/// 非零退出码不等于同一种失败：用户主动 kill、CLI 参数错误、Agent 真的工作失败、
/// Harness Hub 自己管理进程失败，含义完全不同。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    /// 进程自己结束（退出码可能是 0 也可能是非 0）。
    NaturalExit,
    /// 用户主动结束。
    UserKilled,
    /// 启动就没成功（spawn 失败、binary 缺失等）。
    LaunchFailed,
    /// 运行期出错（Harness Hub 侧观测到的运行时故障）。
    RuntimeError,
    /// 宿主（Harness Hub）关闭导致的终止。
    HostShutdown,
    /// 失去联系 / 无法确定（含重启后发现残留的 running）。
    Lost,
}

impl TerminationReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NaturalExit => "natural_exit",
            Self::UserKilled => "user_killed",
            Self::LaunchFailed => "launch_failed",
            Self::RuntimeError => "runtime_error",
            Self::HostShutdown => "host_shutdown",
            Self::Lost => "lost",
        }
    }

    fn from_db(value: &str) -> Option<Self> {
        match value {
            "natural_exit" => Some(Self::NaturalExit),
            "user_killed" => Some(Self::UserKilled),
            "launch_failed" => Some(Self::LaunchFailed),
            "runtime_error" => Some(Self::RuntimeError),
            "host_shutdown" => Some(Self::HostShutdown),
            "lost" => Some(Self::Lost),
            _ => None,
        }
    }

    /// 终止原因 + 进程退出码 → 会话终态。
    ///
    /// 刻意让「用户主动结束」落到 `Exited` 而不是 `Failed`：
    /// 那是我们让进程停的，不是 Agent 工作失败。
    pub fn terminal_status(self, exit_code: Option<i32>) -> SessionStatus {
        match self {
            Self::NaturalExit => match exit_code {
                Some(0) => SessionStatus::Exited,
                Some(_) => SessionStatus::Failed,
                None => SessionStatus::Unknown,
            },
            Self::UserKilled => SessionStatus::Exited,
            Self::LaunchFailed | Self::RuntimeError => SessionStatus::Failed,
            Self::HostShutdown | Self::Lost => SessionStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// 已登记，但还没有启动任何进程。
    Created,
    Running,
    Exited,
    Failed,
    /// 进程已消失但拿不到退出码，或来源数据没给出终态。
    Unknown,
}

impl SessionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Exited => "exited",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    fn from_db(value: &str) -> Self {
        match value {
            "created" => Self::Created,
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

/// 新建会话的输入。插入时 status 固定为 `created`（不是 `running`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSession {
    pub hub_session_id: String,
    pub source_session_id: Option<String>,
    pub harness_id: String,
    /// 关联到具体安装（`harness_installations.id`）。导入的历史会话可以为空。
    pub installation_id: Option<String>,
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
            installation_id: None,
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

    pub fn with_installation_id(mut self, installation_id: impl Into<String>) -> Self {
        self.installation_id = Some(installation_id.into());
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
    pub installation_id: Option<String>,
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
    /// 终止原因；`created` / `running` 阶段以及迁移前的存量终态行为 `None`（原因未记录）。
    pub termination_reason: Option<TerminationReason>,
}

/// Session 表的读写入口。
pub struct SessionStore<'conn> {
    conn: &'conn Connection,
}

impl<'conn> SessionStore<'conn> {
    pub fn new(conn: &'conn Connection) -> Self {
        Self { conn }
    }

    /// 写入一条新会话，状态为 `created`（**不是 running**：此时还没有进程）。
    ///
    /// 重复的 `(harness_id, source_session_id)` 会被数据库唯一索引拒绝。
    pub fn insert(&self, session: &NewSession) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sessions (
                hub_session_id, source_session_id, harness_id, installation_id, project_id,
                runtime_target_id, parent_session_id, status, launch_mode, cwd, worktree_path,
                started_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'created', ?8, ?9, ?10, ?11, ?11, ?11)",
            params![
                session.hub_session_id,
                session.source_session_id,
                session.harness_id,
                session.installation_id,
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

    /// `created` → `running`。**只有进程真的启动成功后才能调用。**
    ///
    /// 返回是否真的发生了状态迁移（非法迁移返回 `false`，不报错）。
    pub fn mark_running(&self, hub_session_id: &str, updated_at: &str) -> Result<bool> {
        let updated = self.conn.execute(
            "UPDATE sessions SET status = 'running', updated_at = ?2
              WHERE hub_session_id = ?1 AND status = 'created'",
            params![hub_session_id, updated_at],
        )?;
        Ok(updated > 0)
    }

    /// 启动失败：**仅 `created` → `failed`**，并记录 `launch_failed`。
    ///
    /// 已经 `running` 的会话必须走 [`Self::finish`]：否则并发下晚到的 `fail`
    /// 会覆盖 `running`（Task 4 的 spawn 线程与等待线程会真的并发）。
    pub fn fail(&self, hub_session_id: &str, ended_at: &str) -> Result<bool> {
        let updated = self.conn.execute(
            "UPDATE sessions
                SET status = 'failed', termination_reason = 'launch_failed',
                    ended_at = ?2, updated_at = ?2
              WHERE hub_session_id = ?1 AND status = 'created'",
            params![hub_session_id, ended_at],
        )?;
        Ok(updated > 0)
    }

    /// 进程结束：**只接受 `running`**。
    ///
    /// `reason` 决定「为什么结束」，`exit_code` 是进程自己的退出码 —— 两者正交。
    /// 终态由 [`TerminationReason::terminal_status`] 推导。
    /// 对 `created` 会话调用会返回 `false`：它从未启动过，结束它只会造出假历史。
    pub fn finish(
        &self,
        hub_session_id: &str,
        exit_code: Option<i32>,
        reason: TerminationReason,
        ended_at: &str,
    ) -> Result<bool> {
        let status = reason.terminal_status(exit_code);

        let updated = self.conn.execute(
            "UPDATE sessions
                SET status = ?2, termination_reason = ?3, ended_at = ?4,
                    exit_code = ?5, updated_at = ?4
              WHERE hub_session_id = ?1 AND status = 'running'",
            params![
                hub_session_id,
                status.as_str(),
                reason.as_str(),
                ended_at,
                exit_code
            ],
        )?;

        Ok(updated > 0)
    }
}

fn map_session_row(row: &Row<'_>) -> rusqlite::Result<SessionRecord> {
    let status: String = row.get(7)?;
    let launch_mode: String = row.get(8)?;
    let termination_reason: Option<String> = row.get(14)?;

    Ok(SessionRecord {
        hub_session_id: row.get(0)?,
        source_session_id: row.get(1)?,
        harness_id: row.get(2)?,
        installation_id: row.get(3)?,
        project_id: row.get(4)?,
        runtime_target_id: row.get(5)?,
        parent_session_id: row.get(6)?,
        status: SessionStatus::from_db(&status),
        launch_mode: LaunchMode::from_db(&launch_mode),
        cwd: row.get(9)?,
        worktree_path: row.get(10)?,
        started_at: row.get(11)?,
        ended_at: row.get(12)?,
        exit_code: row.get(13)?,
        termination_reason: termination_reason
            .as_deref()
            .and_then(TerminationReason::from_db),
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
            .with_installation_id(crate::harness::inventory::installation_id(
                HARNESS_ID,
                RUNTIME_TARGET_ID,
            ))
            .with_cwd("D:/work/project");
        store.insert(&new_session).expect("插入会话");

        let stored = store.get("hub-1").expect("查询").expect("应存在");

        assert_eq!(stored.source_session_id.as_deref(), Some("codex-abc"));
        assert_eq!(stored.installation_id.as_deref(), Some("codex@local"));
        assert_eq!(stored.launch_mode, LaunchMode::Terminal);
        assert_eq!(stored.cwd.as_deref(), Some("D:/work/project"));
        assert_eq!(stored.runtime_target_id, RUNTIME_TARGET_ID);
        assert!(stored.ended_at.is_none());
    }

    /// 新建的会话**绝不能**是 running：此时还没有任何进程。
    #[test]
    fn a_new_session_is_created_not_running() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");

        assert_eq!(
            store.get("hub-1").expect("查询").expect("应存在").status,
            SessionStatus::Created,
            "没有进程就不能标记为 running"
        );
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

    // ---- 状态机 ----

    #[test]
    fn mark_running_moves_created_to_running() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");

        assert!(store
            .mark_running("hub-1", "2026-01-01T10:00:01Z")
            .expect("迁移"));

        let stored = store.get("hub-1").expect("查询").expect("应存在");
        assert_eq!(stored.status, SessionStatus::Running);
        assert!(stored.ended_at.is_none(), "running 不应有结束时间");
    }

    #[test]
    fn mark_running_is_rejected_for_already_running_sessions() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");
        store
            .mark_running("hub-1", "2026-01-01T10:00:01Z")
            .expect("首次");

        assert!(
            !store
                .mark_running("hub-1", "2026-01-01T10:00:02Z")
                .expect("重复"),
            "running → running 不是合法迁移"
        );
    }

    #[test]
    fn fail_marks_a_never_started_session_as_failed() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");

        assert!(store
            .fail("hub-1", "2026-01-01T10:00:05Z")
            .expect("标记失败"));

        let stored = store.get("hub-1").expect("查询").expect("应存在");
        assert_eq!(stored.status, SessionStatus::Failed);
        assert_eq!(stored.ended_at.as_deref(), Some("2026-01-01T10:00:05Z"));
        assert!(stored.exit_code.is_none(), "启动失败没有退出码");
    }

    #[test]
    fn fail_is_rejected_for_finished_sessions() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");
        store
            .mark_running("hub-1", "2026-01-01T10:00:01Z")
            .expect("启动");
        store
            .finish(
                "hub-1",
                Some(0),
                TerminationReason::NaturalExit,
                "2026-01-01T10:00:02Z",
            )
            .expect("结束");

        assert!(!store
            .fail("hub-1", "2026-01-01T10:00:03Z")
            .expect("不应生效"));
        assert_eq!(
            store.get("hub-1").expect("查询").expect("应存在").status,
            SessionStatus::Exited
        );
    }

    /// `running` 的会话不得被 `fail` 改写 —— 进程已经跑起来了，终态必须由 finish 决定。
    #[test]
    fn fail_is_rejected_for_running_sessions() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");
        store
            .mark_running("hub-1", "2026-01-01T10:00:01Z")
            .expect("启动");

        assert!(!store
            .fail("hub-1", "2026-01-01T10:00:02Z")
            .expect("不应生效"));
        assert_eq!(
            store.get("hub-1").expect("查询").expect("应存在").status,
            SessionStatus::Running
        );
    }

    /// **Task 4 并发契约**：终态一旦写入，晚到的状态更新必须全部失败。
    ///
    /// 典型真实竞态：进程已经退出并 `finish`，另一个线程随后才 `mark_running`
    /// —— 若无条件 UPDATE，数据库就会显示 running 而进程早就死了（幽灵 session）。
    #[test]
    fn terminal_states_cannot_be_overwritten_by_late_updates() {
        for (label, terminal_update) in
            [("exited", Some(0)), ("failed", Some(1)), ("unknown", None)]
        {
            let db = seeded_db();
            let store = SessionStore::new(db.connection());
            store
                .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
                .expect("插入");
            store
                .mark_running("hub-1", "2026-01-01T10:00:01Z")
                .expect("启动");
            store
                .finish(
                    "hub-1",
                    terminal_update,
                    TerminationReason::NaturalExit,
                    "2026-01-01T10:00:02Z",
                )
                .expect("结束");
            let terminal = store.get("hub-1").expect("查询").expect("应存在").status;

            // 晚到的所有其他迁移都必须失败
            assert!(
                !store
                    .mark_running("hub-1", "2026-01-01T10:00:03Z")
                    .expect("晚到 mark_running"),
                "{label} 之后 mark_running 必须失败"
            );
            assert!(
                !store
                    .fail("hub-1", "2026-01-01T10:00:03Z")
                    .expect("晚到 fail"),
                "{label} 之后 fail 必须失败"
            );
            assert!(
                !store
                    .finish(
                        "hub-1",
                        Some(0),
                        TerminationReason::NaturalExit,
                        "2026-01-01T10:00:03Z"
                    )
                    .expect("晚到 finish"),
                "{label} 之后 finish 必须失败"
            );

            let after = store.get("hub-1").expect("查询").expect("应存在");
            assert_eq!(after.status, terminal, "{label} 状态被覆盖了");
            assert_eq!(after.ended_at.as_deref(), Some("2026-01-01T10:00:02Z"));
        }
    }

    #[test]
    fn finish_marks_exit_status_and_time() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");
        store
            .mark_running("hub-1", "2026-01-01T10:00:01Z")
            .expect("启动");

        assert!(store
            .finish(
                "hub-1",
                Some(0),
                TerminationReason::NaturalExit,
                "2026-01-01T10:05:00Z"
            )
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
            .mark_running("hub-1", "2026-01-01T10:00:01Z")
            .expect("启动");

        store
            .finish(
                "hub-1",
                Some(130),
                TerminationReason::NaturalExit,
                "2026-01-01T10:05:00Z",
            )
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
            .mark_running("hub-1", "2026-01-01T10:00:01Z")
            .expect("启动");

        store
            .finish(
                "hub-1",
                None,
                TerminationReason::NaturalExit,
                "2026-01-01T10:05:00Z",
            )
            .expect("结束");

        assert_eq!(
            store.get("hub-1").expect("查询").expect("应存在").status,
            SessionStatus::Unknown
        );
    }

    /// 从未启动的会话不能被「结束」——否则会造出 exited 但从未运行的假历史。
    #[test]
    fn finish_is_rejected_for_never_started_sessions() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");

        assert!(!store
            .finish(
                "hub-1",
                Some(0),
                TerminationReason::NaturalExit,
                "2026-01-01T10:05:00Z"
            )
            .expect("不应生效"));
        assert_eq!(
            store.get("hub-1").expect("查询").expect("应存在").status,
            SessionStatus::Created
        );
    }

    #[test]
    fn finish_is_idempotent() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");
        store
            .mark_running("hub-1", "2026-01-01T10:00:01Z")
            .expect("启动");

        assert!(store
            .finish(
                "hub-1",
                Some(0),
                TerminationReason::NaturalExit,
                "2026-01-01T10:05:00Z"
            )
            .expect("第一次"));
        assert!(!store
            .finish(
                "hub-1",
                Some(0),
                TerminationReason::NaturalExit,
                "2026-01-01T10:09:00Z"
            )
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

    // ---- termination_reason（与 exit_code 正交） ----

    #[test]
    fn termination_reason_maps_to_the_right_terminal_status() {
        use TerminationReason::*;

        assert_eq!(NaturalExit.terminal_status(Some(0)), SessionStatus::Exited);
        assert_eq!(NaturalExit.terminal_status(Some(2)), SessionStatus::Failed);
        assert_eq!(NaturalExit.terminal_status(None), SessionStatus::Unknown);

        assert_eq!(
            UserKilled.terminal_status(Some(137)),
            SessionStatus::Exited,
            "用户主动结束不是业务失败，即使进程以非零码退出"
        );

        assert_eq!(LaunchFailed.terminal_status(None), SessionStatus::Failed);
        assert_eq!(RuntimeError.terminal_status(Some(1)), SessionStatus::Failed);
        assert_eq!(HostShutdown.terminal_status(None), SessionStatus::Unknown);
        assert_eq!(Lost.terminal_status(None), SessionStatus::Unknown);
    }

    /// 用户强杀：状态是「已结束」，但退出码与原因都保留下来 —— 这就是拆分二者的意义。
    #[test]
    fn user_kill_keeps_both_the_reason_and_the_exit_code() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");
        store
            .mark_running("hub-1", "2026-01-01T10:00:01Z")
            .expect("启动");

        store
            .finish(
                "hub-1",
                Some(137),
                TerminationReason::UserKilled,
                "2026-01-01T10:05:00Z",
            )
            .expect("结束");

        let stored = store.get("hub-1").expect("查询").expect("应存在");
        assert_eq!(stored.status, SessionStatus::Exited);
        assert_eq!(stored.exit_code, Some(137), "真实退出码必须保留");
        assert_eq!(
            stored.termination_reason,
            Some(TerminationReason::UserKilled)
        );
    }

    #[test]
    fn launch_failure_is_recorded_with_its_own_reason() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");

        assert!(store
            .fail("hub-1", "2026-01-01T10:00:05Z")
            .expect("标记失败"));

        let stored = store.get("hub-1").expect("查询").expect("应存在");
        assert_eq!(stored.status, SessionStatus::Failed);
        assert_eq!(
            stored.termination_reason,
            Some(TerminationReason::LaunchFailed),
            "启动失败必须与运行期失败区分开"
        );
        assert!(stored.exit_code.is_none(), "启动失败没有退出码");
    }

    #[test]
    fn running_sessions_have_no_termination_reason() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");
        store
            .mark_running("hub-1", "2026-01-01T10:00:01Z")
            .expect("启动");

        let stored = store.get("hub-1").expect("查询").expect("应存在");
        assert_eq!(stored.termination_reason, None);
    }

    /// 数据库层兜底：非法原因写不进去。
    #[test]
    fn database_rejects_an_unknown_termination_reason() {
        let db = seeded_db();
        let store = SessionStore::new(db.connection());
        store
            .insert(&session("hub-1", "2026-01-01T10:00:00Z"))
            .expect("插入");

        let invalid = db.connection().execute(
            "UPDATE sessions SET status = 'failed', termination_reason = 'because-i-said-so'
              WHERE hub_session_id = 'hub-1'",
            [],
        );

        assert!(invalid.is_err(), "CHECK 约束必须拒绝未知的终止原因");
    }

    #[test]
    fn termination_reason_round_trips_through_the_database() {
        assert_eq!(
            TerminationReason::from_db("host_shutdown"),
            Some(TerminationReason::HostShutdown)
        );
        assert_eq!(TerminationReason::from_db("nonsense"), None);
        assert_eq!(TerminationReason::NaturalExit.as_str(), "natural_exit");
        assert_eq!(TerminationReason::UserKilled.as_str(), "user_killed");
    }
}
