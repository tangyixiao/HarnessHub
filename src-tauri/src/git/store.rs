//! Git Session Snapshot 的持久化（migration 0010）。
//!
//! 与 `git_events`（Canonical Event 平面的容器，见 ROADMAP 阶段 G24）刻意分开：
//! 这里存的是「每个会话 start / end 至多一行」的有界快照投影，供 Sessions 页直读。
//!
//! 快照只表达「会话时间窗口内仓库的状态」，不能证明代码由 AI 编写
//! （docs/CONTEXT.md 核心约束 7 / 规划 R5）。

use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// 快照在会话生命周期中的时点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotPhase {
    /// 进程启动成功（`mark_running`）之后。
    Start,
    /// 终态写入成功之后。
    End,
}

impl SnapshotPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::End => "end",
        }
    }

    fn from_db(value: &str) -> Option<Self> {
        match value {
            "start" => Some(Self::Start),
            "end" => Some(Self::End),
            _ => None,
        }
    }
}

/// 一条已落库的快照。跨 IPC，键名形状由 commands.rs 的契约测试锁定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionGitSnapshot {
    pub hub_session_id: String,
    pub phase: SnapshotPhase,
    pub captured_at: String,
    pub worktree_root: String,
    /// unborn HEAD（空仓库）为 `None`。
    pub head: Option<String>,
    pub staged_files: i64,
    pub unstaged_files: i64,
    pub untracked_files: i64,
    pub conflicted_files: i64,
}

/// `git_snapshots` 表的读写入口。
pub struct GitSnapshotStore<'conn> {
    conn: &'conn Connection,
}

impl<'conn> GitSnapshotStore<'conn> {
    pub fn new(conn: &'conn Connection) -> Self {
        Self { conn }
    }

    /// 写一条快照；同一 (session, phase) 已存在时返回 `false`（幂等，不报错）。
    ///
    /// 终态路径可能重复触发补拍，靠主键去重而不是靠调用方记忆状态。
    pub fn insert(&self, snapshot: &SessionGitSnapshot) -> Result<bool> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO git_snapshots
                (hub_session_id, phase, captured_at, worktree_root, head,
                 staged_files, unstaged_files, untracked_files, conflicted_files)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                snapshot.hub_session_id,
                snapshot.phase.as_str(),
                snapshot.captured_at,
                snapshot.worktree_root,
                snapshot.head,
                snapshot.staged_files,
                snapshot.unstaged_files,
                snapshot.untracked_files,
                snapshot.conflicted_files,
            ],
        )?;
        Ok(inserted > 0)
    }

    /// 某个会话的快照，start 永远在前、end 在后。
    ///
    /// **phase 优先于时间**：秒退的会话里 start / end 两个后台线程可能乱序落行
    /// （谁先完成 git 子进程谁先插），展示顺序必须由 phase 决定，不能交给线程调度。
    pub fn list_by_session(&self, hub_session_id: &str) -> Result<Vec<SessionGitSnapshot>> {
        let mut statement = self.conn.prepare(
            "SELECT hub_session_id, phase, captured_at, worktree_root, head,
                    staged_files, unstaged_files, untracked_files, conflicted_files
             FROM git_snapshots WHERE hub_session_id = ?1
             ORDER BY CASE phase WHEN 'start' THEN 0 ELSE 1 END, captured_at ASC",
        )?;
        let rows = statement.query_map(params![hub_session_id], map_snapshot_row)?;

        let mut snapshots = Vec::new();
        for row in rows {
            snapshots.push(row?);
        }
        Ok(snapshots)
    }
}

fn map_snapshot_row(row: &Row<'_>) -> rusqlite::Result<SessionGitSnapshot> {
    let phase: String = row.get(1)?;
    // phase 由 CHECK 约束兜底，解析失败说明库被外部改坏。
    let phase = SnapshotPhase::from_db(&phase).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            format!("未知快照 phase：{phase}").into(),
        )
    })?;

    Ok(SessionGitSnapshot {
        hub_session_id: row.get(0)?,
        phase,
        captured_at: row.get(2)?,
        worktree_root: row.get(3)?,
        head: row.get(4)?,
        staged_files: row.get(5)?,
        unstaged_files: row.get(6)?,
        untracked_files: row.get(7)?,
        conflicted_files: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::store::{NewSession, SessionStore};
    use crate::test_support::{seeded_db, HARNESS_ID, RUNTIME_TARGET_ID};

    fn snapshot(session_id: &str, phase: SnapshotPhase, captured_at: &str) -> SessionGitSnapshot {
        SessionGitSnapshot {
            hub_session_id: session_id.to_string(),
            phase,
            captured_at: captured_at.to_string(),
            worktree_root: "D:/work/demo".to_string(),
            head: Some("a".repeat(40)),
            staged_files: 1,
            unstaged_files: 2,
            untracked_files: 3,
            conflicted_files: 0,
        }
    }

    fn session(db: &crate::db::Database, id: &str) {
        SessionStore::new(db.connection())
            .insert(&NewSession::new(
                id,
                HARNESS_ID,
                RUNTIME_TARGET_ID,
                "2026-01-01T10:00:00Z",
            ))
            .expect("插入会话");
    }

    #[test]
    fn insert_then_list_roundtrip_orders_by_captured_at() {
        let db = seeded_db();
        session(&db, "hub-1");
        let store = GitSnapshotStore::new(db.connection());

        assert!(store
            .insert(&snapshot(
                "hub-1",
                SnapshotPhase::End,
                "2026-01-01T10:05:00Z"
            ))
            .expect("end"));
        assert!(store
            .insert(&snapshot(
                "hub-1",
                SnapshotPhase::Start,
                "2026-01-01T10:00:01Z"
            ))
            .expect("start"));

        let listed = store.list_by_session("hub-1").expect("列出");
        assert_eq!(listed.len(), 2);
        // 排序按拍摄时间：start 在前，即使后插入。
        assert_eq!(listed[0].phase, SnapshotPhase::Start);
        assert_eq!(listed[1].phase, SnapshotPhase::End);
        assert_eq!(listed[0].captured_at, "2026-01-01T10:00:01Z");
    }

    /// 同一 (session, phase) 重复补拍必须幂等：返回 false 且**不改写**首次的行。
    #[test]
    fn duplicate_phase_is_ignored_and_keeps_the_first_capture() {
        let db = seeded_db();
        session(&db, "hub-1");
        let store = GitSnapshotStore::new(db.connection());

        assert!(store
            .insert(&snapshot(
                "hub-1",
                SnapshotPhase::Start,
                "2026-01-01T10:00:01Z"
            ))
            .expect("首次"));
        let mut again = snapshot("hub-1", SnapshotPhase::Start, "2026-01-01T10:09:00Z");
        again.head = Some("f".repeat(40));
        assert!(
            !store.insert(&again).expect("重复"),
            "重复 (session, phase) 不得再插入"
        );

        let stored = &store.list_by_session("hub-1").expect("列出")[0];
        assert_eq!(
            stored.captured_at, "2026-01-01T10:00:01Z",
            "首次快照不得被改写"
        );
        let expected_head = "a".repeat(40);
        assert_eq!(stored.head.as_deref(), Some(expected_head.as_str()));
    }

    #[test]
    fn list_for_unknown_session_is_empty() {
        let db = seeded_db();
        let store = GitSnapshotStore::new(db.connection());
        assert!(store
            .list_by_session("does-not-exist")
            .expect("列出")
            .is_empty());
    }

    #[test]
    fn phase_round_trips_through_the_database() {
        assert_eq!(SnapshotPhase::Start.as_str(), "start");
        assert_eq!(SnapshotPhase::End.as_str(), "end");
        assert_eq!(SnapshotPhase::from_db("start"), Some(SnapshotPhase::Start));
        assert_eq!(SnapshotPhase::from_db("end"), Some(SnapshotPhase::End));
        assert_eq!(SnapshotPhase::from_db("nonsense"), None);
    }
}
