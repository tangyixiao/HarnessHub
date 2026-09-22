//! Session 编排：把「哪个 Harness、在哪个项目、哪台运行目标」组装成一条库记录。
//!
//! 这一层负责三件 Store 不该管的事：
//!   1. 生成 `hub_session_id`（UUID v4）—— 与外部 Harness 的 `source_session_id` 彻底分离；
//!   2. 取当前时间；
//!   3. 绑定运行目标（v0.1 恒为 `local`）。

use rusqlite::Connection;
use uuid::Uuid;

use crate::clock;
use crate::error::{Error, Result};
use crate::runtime::local::LOCAL_TARGET_ID;
use crate::session::store::{NewSession, SessionRecord, SessionStore};

/// Session 表的编排入口。
pub struct SessionService<'conn> {
    store: SessionStore<'conn>,
}

impl<'conn> SessionService<'conn> {
    pub fn new(conn: &'conn Connection) -> Self {
        Self {
            store: SessionStore::new(conn),
        }
    }

    /// 新建一条 `running` 会话记录，返回**落库之后**的完整记录。
    ///
    /// `hub_session_id` 由 Harness Hub 生成（UUID v4），与外部 Harness 的
    /// `source_session_id` 完全分离：PTY 会话没有外部 id，导入的历史会话才有。
    pub fn start(
        &self,
        harness_id: &str,
        project_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<SessionRecord> {
        let hub_session_id = Uuid::new_v4().to_string();
        let started_at = clock::now_rfc3339();

        let mut session = NewSession::new(
            hub_session_id.as_str(),
            harness_id,
            LOCAL_TARGET_ID,
            started_at.as_str(),
        );
        session.project_id = project_id.map(str::to_string);
        // 缺省 cwd 写 NULL，而不是空串：空串会让「未记录 cwd」和「cwd 是空目录」混淆。
        session.cwd = cwd.map(str::to_string);

        self.store.insert(&session)?;

        self.store
            .get(&hub_session_id)?
            .ok_or_else(|| Error::InvalidInput("刚写入的会话读不回来".to_string()))
    }

    pub fn get(&self, hub_session_id: &str) -> Result<Option<SessionRecord>> {
        self.store.get(hub_session_id)
    }

    /// 结束会话；返回是否真的更新了行（重复调用返回 `false`，不报错）。
    pub fn finish(&self, hub_session_id: &str, exit_code: Option<i32>) -> Result<bool> {
        self.store
            .finish(hub_session_id, exit_code, &clock::now_rfc3339())
    }

    pub fn list_recent(&self, limit: u32) -> Result<Vec<SessionRecord>> {
        self.store.list_recent(limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::local::ensure_local_target;
    use crate::session::store::SessionStatus;
    use crate::test_support::{seeded_db, HARNESS_ID};

    fn service_from(db: &crate::db::Database) -> SessionService<'_> {
        ensure_local_target(db.connection()).expect("引导 runtime target");
        SessionService::new(db.connection())
    }

    #[test]
    fn start_creates_a_running_session_bound_to_the_local_target() {
        let db = seeded_db();
        let service = service_from(&db);

        let record = service
            .start(HARNESS_ID, None, Some("D:/work"))
            .expect("启动会话记录");

        assert_eq!(record.status, SessionStatus::Running);
        assert_eq!(record.harness_id, HARNESS_ID);
        assert_eq!(record.runtime_target_id, "local");
        assert_eq!(record.cwd.as_deref(), Some("D:/work"));
        assert!(!record.hub_session_id.is_empty(), "必须生成 hub_session_id");
        assert!(
            record.started_at.ends_with('Z'),
            "时间必须是 RFC3339：{}",
            record.started_at
        );
        assert!(record.ended_at.is_none());
        assert!(
            record.source_session_id.is_none(),
            "PTY 会话没有外部 session id"
        );
    }

    #[test]
    fn each_start_gets_a_distinct_hub_session_id() {
        let db = seeded_db();
        let service = service_from(&db);

        let first = service.start(HARNESS_ID, None, None).expect("第一次");
        let second = service.start(HARNESS_ID, None, None).expect("第二次");

        assert_ne!(first.hub_session_id, second.hub_session_id);
        assert_eq!(service.list_recent(10).expect("列出").len(), 2);
        // 没有 source_session_id 时不会被唯一索引去重。
        assert!(first.source_session_id.is_none() && second.source_session_id.is_none());
    }

    #[test]
    fn start_records_the_optional_project() {
        let db = seeded_db();
        db.connection()
            .execute(
                "INSERT INTO projects (id, name, repo_root, created_at, updated_at)
                 VALUES ('p1', 'demo', 'D:/work/demo', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .expect("插入项目");
        let service = service_from(&db);

        let record = service
            .start(HARNESS_ID, Some("p1"), Some("D:/work/demo"))
            .expect("启动");

        assert_eq!(record.project_id.as_deref(), Some("p1"));
    }

    #[test]
    fn start_without_cwd_records_null_instead_of_empty_string() {
        let db = seeded_db();
        let service = service_from(&db);

        let record = service.start(HARNESS_ID, None, None).expect("启动");

        assert!(record.cwd.is_none(), "缺省 cwd 应为 NULL，而不是空串");
    }

    #[test]
    fn finish_marks_the_session_ended_and_is_idempotent() {
        let db = seeded_db();
        let service = service_from(&db);
        let started = service.start(HARNESS_ID, None, None).expect("启动");

        assert!(service
            .finish(&started.hub_session_id, Some(0))
            .expect("结束"));
        assert!(!service
            .finish(&started.hub_session_id, Some(0))
            .expect("重复结束"));

        let stored = service
            .get(&started.hub_session_id)
            .expect("查询")
            .expect("应存在");
        assert_eq!(stored.status, SessionStatus::Exited);
        assert!(stored.ended_at.is_some());
    }

    #[test]
    fn finish_with_nonzero_exit_code_marks_failed() {
        let db = seeded_db();
        let service = service_from(&db);
        let started = service.start(HARNESS_ID, None, None).expect("启动");

        service
            .finish(&started.hub_session_id, Some(130))
            .expect("结束");

        assert_eq!(
            service
                .get(&started.hub_session_id)
                .expect("查询")
                .expect("应存在")
                .status,
            SessionStatus::Failed
        );
    }

    #[test]
    fn finish_on_unknown_session_reports_false_without_creating_anything() {
        let db = seeded_db();
        let service = service_from(&db);

        assert!(!service.finish("does-not-exist", Some(0)).expect("不应报错"));
        assert!(service.list_recent(10).expect("列出").is_empty());
    }

    #[test]
    fn list_recent_respects_the_limit() {
        let db = seeded_db();
        let service = service_from(&db);
        service.start(HARNESS_ID, None, None).expect("第一次");
        service.start(HARNESS_ID, None, None).expect("第二次");
        service.start(HARNESS_ID, None, None).expect("第三次");

        assert_eq!(service.list_recent(2).expect("列出").len(), 2);
    }

    #[test]
    fn list_recent_on_empty_database_returns_empty() {
        let db = seeded_db();
        let service = service_from(&db);

        assert!(service.list_recent(50).expect("列出").is_empty());
    }
}
