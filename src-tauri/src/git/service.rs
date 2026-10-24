//! 把 `git::capture` 接进会话生命周期：短锁读 cwd → 无锁跑 git → 短锁落行。
//!
//! 快照是 **best-effort 观测**：cwd 缺失 / 会话不存在 / 非 Git 仓库 / git 命令失败
//! （含超时）都返回「没有快照」，绝不上升为会话状态机的错误 ——
//! 观测失败把启动或退出搞挂，比少一行快照严重得多。
//!
//! `snapshot_record` 会执行 git 子进程（每条命令 3 秒上限，见 `git::runner`），
//! 因此**调用方不得在数据库锁内调用**；后台线程入口见 [`spawn_capture`]。

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::clock;
use crate::db::Database;
use crate::error::Result;
use crate::git::store::{GitSnapshotStore, SessionGitSnapshot, SnapshotPhase};
use crate::git::{capture, GitObservation};
use crate::session::store::SessionStore;

/// 跑一次有界 capture 并组装快照记录。不碰数据库。
///
/// 返回 `None` 的合法情形：cwd 不是 Git 仓库、git 命令失败（含超时）。
pub fn snapshot_record(
    hub_session_id: &str,
    cwd: &str,
    phase: SnapshotPhase,
) -> Option<SessionGitSnapshot> {
    let snapshot = match capture(Path::new(cwd)) {
        Ok(GitObservation::Repository(snapshot)) => snapshot,
        // 非 Git 仓库 / git 失败：没有快照，不是错误。
        _ => return None,
    };

    Some(SessionGitSnapshot {
        hub_session_id: hub_session_id.to_string(),
        phase,
        captured_at: clock::now_rfc3339(),
        // SQLite TEXT 存路径；非 UTF-8 路径做损失转换（快照用于展示，不用于回放定位）。
        worktree_root: snapshot.worktree_root.to_string_lossy().into_owned(),
        head: snapshot.head,
        staged_files: bounded(snapshot.staged_files),
        unstaged_files: bounded(snapshot.unstaged_files),
        untracked_files: bounded(snapshot.untracked_files),
        conflicted_files: bounded(snapshot.conflicted_files),
    })
}

/// u64 文件计数 → SQLite INTEGER。计数受 1 MiB 输出上限约束，
/// 理论上不可能溢出 i64，饱和转换只是防御。
fn bounded(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

/// 同步补拍一条快照：读 cwd → capture → 落行。
///
/// **会执行 git 子进程**：调用方要么不持有外层数据库锁，要么接受最长
/// 每条命令 3 秒的阻塞。GUI 路径请用 [`spawn_capture`]。
pub fn capture_for_session(
    conn: &Connection,
    hub_session_id: &str,
    phase: SnapshotPhase,
) -> Result<bool> {
    let Some(session) = SessionStore::new(conn).get(hub_session_id)? else {
        return Ok(false);
    };
    let Some(cwd) = session.cwd else {
        return Ok(false);
    };
    let Some(record) = snapshot_record(hub_session_id, &cwd, phase) else {
        return Ok(false);
    };
    GitSnapshotStore::new(conn).insert(&record)
}

/// 在后台线程补拍一条快照（terminal / IPC 终态路径用）。
///
/// 三段式锁纪律：短锁读 cwd → **丢锁**跑 git 子进程 → 短锁落行。
/// 任何一步失败都静默放弃（不落行）；返回不通知调用方 —— 观测是 best-effort。
pub fn spawn_capture(
    db: Arc<Mutex<Database>>,
    hub_session_id: &str,
    phase: SnapshotPhase,
) {
    let hub_session_id = hub_session_id.to_string();
    std::thread::spawn(move || {
        let cwd = db
            .lock()
            .ok()
            .and_then(|database| {
                SessionStore::new(database.connection())
                    .get(&hub_session_id)
                    .ok()
                    .flatten()
            })
            .and_then(|session| session.cwd);
        let Some(cwd) = cwd else { return };

        let Some(record) = snapshot_record(&hub_session_id, &cwd, phase) else {
            return;
        };

        if let Ok(database) = db.lock() {
            let _ = GitSnapshotStore::new(database.connection()).insert(&record);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::store::GitSnapshotStore;
    use crate::git::test_support::Repo;
    use crate::session::service::SessionService;
    use crate::session::SessionStatus;
    use crate::test_support::{cold_start_db, seeded_db, HARNESS_ID, RUNTIME_TARGET_ID};

    fn installation() -> String {
        crate::harness::inventory::installation_id(HARNESS_ID, RUNTIME_TARGET_ID)
    }

    /// 真仓库上的完整链路：start 快照 → 仓库变脏 → end 快照。
    #[test]
    fn captures_start_and_end_for_a_real_repository() {
        let repo = Repo::new();
        std::fs::write(repo.0.join("file"), b"data").unwrap();
        repo.git(&["add", "file"]);
        repo.git(&["commit", "-qm", "initial"]);

        let db = seeded_db();
        let service = SessionService::new(db.connection());
        let session = service
            .start_from_installation(&installation(), None, Some(repo.0.to_string_lossy().as_ref()))
            .expect("登记会话");
        service
            .mark_running(&session.hub_session_id, Some(4242))
            .expect("启动");

        assert!(
            capture_for_session(db.connection(), &session.hub_session_id, SnapshotPhase::Start)
                .expect("start 快照"),
            "真仓库的 start 快照必须落行"
        );

        std::fs::write(repo.0.join("untracked"), b"untracked").unwrap();
        std::fs::write(repo.0.join("file"), b"changed").unwrap();
        repo.git(&["add", "file"]);

        assert!(
            capture_for_session(db.connection(), &session.hub_session_id, SnapshotPhase::End)
                .expect("end 快照")
        );

        let snapshots = GitSnapshotStore::new(db.connection())
            .list_by_session(&session.hub_session_id)
            .expect("列出");
        assert_eq!(snapshots.len(), 2);

        let start = &snapshots[0];
        let end = &snapshots[1];
        assert_eq!(start.phase, SnapshotPhase::Start);
        assert_eq!(end.phase, SnapshotPhase::End);
        assert_eq!(start.head, end.head, "没有新 commit，HEAD 不应变化");
        assert!(start.head.as_deref().is_some_and(|head| head.len() == 40));
        assert_eq!(start.untracked_files, 0);
        assert_eq!(end.untracked_files, 1, "新增的未跟踪文件必须被计数");
        assert_eq!(end.staged_files, 1, "git add 后的暂存修改必须被计数");
        assert_eq!(start.worktree_root, end.worktree_root);
    }

    /// 空（unborn）仓库的 HEAD 是 NULL，快照仍必须落行。
    #[test]
    fn unborn_repository_snapshots_with_null_head() {
        let repo = Repo::new();
        let db = seeded_db();
        let session = SessionService::new(db.connection())
            .start_from_installation(&installation(), None, Some(repo.0.to_string_lossy().as_ref()))
            .expect("登记会话");

        assert!(capture_for_session(
            db.connection(),
            &session.hub_session_id,
            SnapshotPhase::Start
        )
        .expect("start 快照"));

        let snapshots = GitSnapshotStore::new(db.connection())
            .list_by_session(&session.hub_session_id)
            .expect("列出");
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].head, None, "unborn HEAD 必须是 NULL 而不是空串");
    }

    /// 非 Git 仓库：不落行、不报错。
    #[test]
    fn plain_directory_produces_no_snapshot_and_no_error() {
        let plain = std::env::temp_dir().join(format!("hh-nonrepo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&plain).unwrap();

        let db = seeded_db();
        let session = SessionService::new(db.connection())
            .start_from_installation(&installation(), None, Some(plain.to_string_lossy().as_ref()))
            .expect("登记会话");

        assert!(!capture_for_session(
            db.connection(),
            &session.hub_session_id,
            SnapshotPhase::Start
        )
        .expect("非仓库目录不得报错"));

        assert!(GitSnapshotStore::new(db.connection())
            .list_by_session(&session.hub_session_id)
            .expect("列出")
            .is_empty());
        let _ = std::fs::remove_dir_all(plain);
    }

    /// 没有 cwd 的会话与不存在的会话：安全返回 false。
    #[test]
    fn missing_cwd_or_unknown_session_produces_no_snapshot() {
        let db = seeded_db();
        let service = SessionService::new(db.connection());
        let no_cwd = service
            .start_from_installation(&installation(), None, None)
            .expect("登记");

        assert!(!capture_for_session(db.connection(), &no_cwd.hub_session_id, SnapshotPhase::Start)
            .expect("无 cwd 不得报错"));
        assert!(!capture_for_session(db.connection(), "does-not-exist", SnapshotPhase::Start)
            .expect("未知会话不得报错"));
    }

    /// **冷启动纪律**（AGENTS.md）：空库起步，前置状态全部由生产代码填充
    /// （ensure_local_target + reconcile 都在 `cold_start_db` 内走生产入口），
    /// 快照行必须经得起完整 FK 链。
    #[test]
    fn cold_start_session_can_capture_a_snapshot() {
        let repo = Repo::new();
        std::fs::write(repo.0.join("file"), b"data").unwrap();
        repo.git(&["add", "file"]);
        repo.git(&["commit", "-qm", "initial"]);

        let db = cold_start_db();
        let service = SessionService::new(db.connection());
        let session = service
            .start_from_installation(&installation(), None, Some(repo.0.to_string_lossy().as_ref()))
            .expect("冷启动会话");
        assert!(service
            .mark_running(&session.hub_session_id, Some(4242))
            .expect("启动"));

        assert!(capture_for_session(
            db.connection(),
            &session.hub_session_id,
            SnapshotPhase::Start
        )
        .expect("start 快照"));

        assert_eq!(
            GitSnapshotStore::new(db.connection())
                .list_by_session(&session.hub_session_id)
                .expect("列出")
                .len(),
            1
        );
        assert_eq!(session.status, SessionStatus::Created);
    }
}
