//! Session 编排：把「哪个安装、在哪个项目」组装成一条库记录。
//!
//! 这一层负责 Store 不该管的事：
//!   1. 生成 `hub_session_id`（UUID v4）—— 与外部 Harness 的 `source_session_id` 彻底分离；
//!   2. 取当前时间；
//!   3. 从 installation **推导** harness_id 与 runtime_target_id，让矛盾组合无从构造。
//!
//! 状态迁移由 Store 保证合法性（见 `session::store` 的状态机）：
//! `start_from_installation` 只创建 `created`，**不**标记 running —— 启动进程是 Task 4 的事。

use rusqlite::Connection;
use uuid::Uuid;

use crate::clock;
use crate::error::{Error, Result};
use crate::harness::store::find_installation;
use crate::session::store::{NewSession, SessionRecord, SessionStore};

/// Session 表的编排入口。
pub struct SessionService<'conn> {
    conn: &'conn Connection,
    store: SessionStore<'conn>,
}

impl<'conn> SessionService<'conn> {
    pub fn new(conn: &'conn Connection) -> Self {
        Self {
            conn,
            store: SessionStore::new(conn),
        }
    }

    /// 根据**安装**登记一条会话，状态为 `created`，返回落库后的完整记录。
    ///
    /// `harness_id` 与 `runtime_target_id` 都从 installation 推导，调用方**无法**
    /// 构造出「installation 在 local，却把 session 挂到 wsl」这种矛盾组合
    /// （数据库层另有复合外键兜底，见 migration 0003）。
    ///
    /// `hub_session_id` 由 Harness Hub 生成（UUID v4），与外部 Harness 的
    /// `source_session_id` 完全分离：PTY 会话没有外部 id，导入的历史会话才有。
    pub fn start_from_installation(
        &self,
        installation_id: &str,
        project_id: Option<&str>,
        cwd: Option<&str>,
    ) -> Result<SessionRecord> {
        let installation = find_installation(self.conn, installation_id)?.ok_or_else(|| {
            Error::InvalidInput(format!(
                "未知的安装：{installation_id}（先同步 Harness 清单）"
            ))
        })?;

        let hub_session_id = Uuid::new_v4().to_string();
        let started_at = clock::now_rfc3339();

        let mut session = NewSession::new(
            hub_session_id.as_str(),
            &installation.harness_id,
            &installation.runtime_target_id,
            started_at.as_str(),
        );
        session.installation_id = Some(installation.id);
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

    /// `created` → `running`。只有进程真的启动成功后才能调用（Task 4）。
    pub fn mark_running(&self, hub_session_id: &str) -> Result<bool> {
        self.store
            .mark_running(hub_session_id, &clock::now_rfc3339())
    }

    /// 启动失败：`created` / `running` → `failed`。
    pub fn fail(&self, hub_session_id: &str) -> Result<bool> {
        self.store.fail(hub_session_id, &clock::now_rfc3339())
    }

    /// 进程结束（只接受 `running`）；重复调用返回 `false`，不报错。
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

    fn installation() -> String {
        crate::harness::inventory::installation_id(HARNESS_ID, "local")
    }

    #[test]
    fn start_only_creates_the_session_it_does_not_claim_to_be_running() {
        let db = seeded_db();
        let service = service_from(&db);

        let record = service
            .start_from_installation(&installation(), None, Some("D:/work"))
            .expect("登记会话");

        assert_eq!(
            record.status,
            SessionStatus::Created,
            "还没有进程，就不能是 running"
        );
        assert_eq!(record.harness_id, HARNESS_ID);
        assert_eq!(
            record.installation_id.as_deref(),
            Some(installation().as_str())
        );
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

    /// 未知安装必须给明确错误，而不是让外键报错。
    #[test]
    fn starting_from_an_unknown_installation_is_rejected() {
        let db = seeded_db();
        let service = service_from(&db);

        let error = service
            .start_from_installation("codex@nowhere", None, None)
            .expect_err("未知安装必须失败");

        assert!(
            error.to_string().contains("codex@nowhere"),
            "错误信息要指出是哪个安装：{error}"
        );
        assert!(
            service.list_recent(10).expect("列出").is_empty(),
            "失败时不得留下会话"
        );
    }

    /// **组合一致性**：session 的 runtime_target_id 必须由 installation 推导，
    /// 而不是由调用方另行提供（否则可能出现 codex@local 配 wsl 的矛盾组合）。
    #[test]
    fn session_runtime_target_is_derived_from_the_installation() {
        let db = seeded_db();
        let conn = db.connection();

        // 在另一个 runtime target 上造一个安装：同一个 Codex，不同机器
        conn.execute(
            "INSERT INTO runtime_targets (id, kind, display_name, created_at)
             VALUES ('wsl-ubuntu', 'local', 'WSL Ubuntu', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("第二个 runtime target");
        conn.execute(
            "INSERT INTO harness_installations
                (id, harness_id, runtime_target_id, availability, first_detected_at, last_seen_at)
             VALUES ('codex@wsl-ubuntu', 'codex', 'wsl-ubuntu', 'available',
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("wsl 上的安装");

        let service = SessionService::new(conn);
        let local = service
            .start_from_installation(&installation(), None, None)
            .expect("local 会话");
        let wsl = service
            .start_from_installation("codex@wsl-ubuntu", None, None)
            .expect("wsl 会话");

        assert_eq!(local.runtime_target_id, "local");
        assert_eq!(wsl.runtime_target_id, "wsl-ubuntu");
        assert_eq!(
            wsl.installation_id.as_deref(),
            Some("codex@wsl-ubuntu"),
            "session 必须绑定到它自己的安装"
        );
        assert_ne!(
            local.runtime_target_id, wsl.runtime_target_id,
            "同一 Harness 的不同安装必须落在各自的 runtime target 上"
        );
    }

    /// 完整生命周期：created → running → exited。
    #[test]
    fn full_lifecycle_created_running_exited() {
        let db = seeded_db();
        let service = service_from(&db);
        let started = service
            .start_from_installation(&installation(), None, None)
            .expect("登记");

        assert!(service.mark_running(&started.hub_session_id).expect("启动"));
        assert_eq!(
            service
                .get(&started.hub_session_id)
                .expect("查询")
                .expect("存在")
                .status,
            SessionStatus::Running
        );

        assert!(service
            .finish(&started.hub_session_id, Some(0))
            .expect("结束"));
        let stored = service
            .get(&started.hub_session_id)
            .expect("查询")
            .expect("存在");
        assert_eq!(stored.status, SessionStatus::Exited);
        assert!(stored.ended_at.is_some());
    }

    /// 未启动的会话不能被结束 —— 否则会造出「exited 但从未运行」的假历史。
    #[test]
    fn a_created_session_cannot_be_finished() {
        let db = seeded_db();
        let service = service_from(&db);
        let started = service
            .start_from_installation(&installation(), None, None)
            .expect("登记");

        assert!(!service
            .finish(&started.hub_session_id, Some(0))
            .expect("不应生效"));
        assert_eq!(
            service
                .get(&started.hub_session_id)
                .expect("查询")
                .expect("存在")
                .status,
            SessionStatus::Created
        );
    }

    #[test]
    fn fail_marks_a_session_whose_process_never_started() {
        let db = seeded_db();
        let service = service_from(&db);
        let started = service
            .start_from_installation(&installation(), None, None)
            .expect("登记");

        assert!(service.fail(&started.hub_session_id).expect("标记失败"));

        let stored = service
            .get(&started.hub_session_id)
            .expect("查询")
            .expect("存在");
        assert_eq!(stored.status, SessionStatus::Failed);
        assert!(stored.ended_at.is_some());
    }

    /// **冷启动纪律**：从真正空库开始，只走生产路径（ensure_local_target + reconcile + start）。
    ///
    /// 这条测试的存在理由：真实宿主上曾经因为夹具替生产代码插好了 harness 行，
    /// 导致单元测试全绿而冷启动 `create_session` 直接 FK 失败。
    #[test]
    fn cold_start_from_an_empty_database_can_create_a_session() {
        // 空库：没有任何 harness / installation / runtime target
        let db = crate::test_support::cold_start_db();
        let service = SessionService::new(db.connection());

        let record = service
            .start_from_installation(&installation(), None, None)
            .expect("冷启动后必须能建会话");

        assert_eq!(record.status, SessionStatus::Created);
        assert_eq!(
            record.installation_id.as_deref(),
            Some(installation().as_str())
        );
    }

    #[test]
    fn each_start_gets_a_distinct_hub_session_id() {
        let db = seeded_db();
        let service = service_from(&db);

        let first = service
            .start_from_installation(&installation(), None, None)
            .expect("第一次");
        let second = service
            .start_from_installation(&installation(), None, None)
            .expect("第二次");

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
            .start_from_installation(&installation(), Some("p1"), Some("D:/work/demo"))
            .expect("登记");

        assert_eq!(record.project_id.as_deref(), Some("p1"));
    }

    #[test]
    fn start_without_cwd_records_null_instead_of_empty_string() {
        let db = seeded_db();
        let service = service_from(&db);

        let record = service
            .start_from_installation(&installation(), None, None)
            .expect("登记");

        assert!(record.cwd.is_none(), "缺省 cwd 应为 NULL，而不是空串");
    }

    #[test]
    fn finish_is_idempotent() {
        let db = seeded_db();
        let service = service_from(&db);
        let started = service
            .start_from_installation(&installation(), None, None)
            .expect("登记");
        service.mark_running(&started.hub_session_id).expect("启动");

        assert!(service
            .finish(&started.hub_session_id, Some(0))
            .expect("结束"));
        assert!(!service
            .finish(&started.hub_session_id, Some(0))
            .expect("重复结束"));
    }

    #[test]
    fn finish_with_nonzero_exit_code_marks_failed() {
        let db = seeded_db();
        let service = service_from(&db);
        let started = service
            .start_from_installation(&installation(), None, None)
            .expect("登记");
        service.mark_running(&started.hub_session_id).expect("启动");

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
        service
            .start_from_installation(&installation(), None, None)
            .expect("第一次");
        service
            .start_from_installation(&installation(), None, None)
            .expect("第二次");
        service
            .start_from_installation(&installation(), None, None)
            .expect("第三次");

        assert_eq!(service.list_recent(2).expect("列出").len(), 2);
    }

    #[test]
    fn list_recent_on_empty_database_returns_empty() {
        let db = seeded_db();
        let service = service_from(&db);

        assert!(service.list_recent(50).expect("列出").is_empty());
    }
}
