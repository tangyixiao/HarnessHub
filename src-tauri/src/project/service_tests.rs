//! service 契约（Task 1 RED）：repo root 检测注入 + 幂等登记 + 冷启动。

use crate::db::Database;
use crate::error::{Error, Result};
use crate::project::git_root::GitRootProbe;
use crate::project::{service::ProjectService, RegisterRequest};
use std::path::Path;
use std::sync::Arc;

/// Fake：把目录映射到 repo root；未映射的目录一律「不是仓库」。
struct FakeGitRoot {
    roots: Vec<(String, String)>,
}

impl GitRootProbe for FakeGitRoot {
    fn repo_root(&self, path: &Path) -> Result<Option<String>> {
        let key = path.to_string_lossy().replace('\\', "/");
        Ok(self
            .roots
            .iter()
            .find(|(candidate, _)| key.starts_with(candidate.as_str()))
            .map(|(_, root)| root.clone()))
    }
}

fn probe_of(roots: Vec<(String, String)>) -> Arc<dyn GitRootProbe> {
    Arc::new(FakeGitRoot { roots })
}

fn request(path: &str) -> RegisterRequest {
    RegisterRequest {
        path: path.to_string(),
        name: None,
    }
}

#[test]
fn register_resolves_the_real_repo_root() {
    let db = Database::open_in_memory().expect("内存库");
    let service = ProjectService::new(
        db.connection(),
        probe_of(vec![(
            "D:/work/demo".to_string(),
            "D:/work/demo".to_string(),
        )]),
    );

    let record = service.register(&request("D:/work/demo")).expect("登记");

    assert_eq!(record.repo_root, "D:/work/demo");
    assert_eq!(record.name, "demo", "未指定名字时取 root 最后一段");
}

#[test]
fn register_resolves_a_subdirectory_to_its_repository_root() {
    let db = Database::open_in_memory().expect("内存库");
    let service = ProjectService::new(
        db.connection(),
        probe_of(vec![(
            "D:/work/demo".to_string(),
            "D:/work/demo".to_string(),
        )]),
    );

    let record = service
        .register(&request("D:/work/demo/src-tauri/src"))
        .expect("登记子目录");

    assert_eq!(
        record.repo_root, "D:/work/demo",
        "以 repo root 为准，不是输入目录"
    );
}

#[test]
fn register_rejects_a_path_outside_any_repository() {
    let db = Database::open_in_memory().expect("内存库");
    let service = ProjectService::new(db.connection(), probe_of(vec![]));

    let error = service
        .register(&request("D:/not-a-repo"))
        .expect_err("非 repo 必须失败");

    assert!(matches!(error, Error::InvalidInput(ref message)
        if message.contains("不是有效的本地 Git 仓库")));
    assert_eq!(service.list_recent(10).expect("列表").len(), 0);
}

#[test]
fn register_keeps_the_caller_supplied_name() {
    let db = Database::open_in_memory().expect("内存库");
    let service = ProjectService::new(
        db.connection(),
        probe_of(vec![(
            "D:/work/demo".to_string(),
            "D:/work/demo".to_string(),
        )]),
    );

    let record = service
        .register(&RegisterRequest {
            path: "D:/work/demo".to_string(),
            name: Some("工作仓库".to_string()),
        })
        .expect("登记");

    assert_eq!(record.name, "工作仓库");
}

#[test]
fn re_registering_the_same_repo_is_idempotent_and_touches_last_opened_at() {
    let db = Database::open_in_memory().expect("内存库");
    let service = ProjectService::new(
        db.connection(),
        probe_of(vec![(
            "D:/work/demo".to_string(),
            "D:/work/demo".to_string(),
        )]),
    );

    let first = service.register(&request("D:/work/demo")).expect("首次");
    let second = service.register(&request("D:/work/demo")).expect("重复");

    assert_eq!(second.id, first.id, "同一 repo_root 不产生第二行");
    assert_eq!(service.list_recent(10).expect("列表").len(), 1);
    assert_eq!(
        second.last_opened_at.as_deref(),
        Some(second.updated_at.as_str()),
        "重复登记刷新 last_opened_at"
    );
    assert!(
        second.last_opened_at.as_deref() >= first.last_opened_at.as_deref(),
        "last_opened_at 单调不减"
    );
}

#[test]
fn remove_is_idempotent_and_reports_whether_anything_was_removed() {
    let db = Database::open_in_memory().expect("内存库");
    let service = ProjectService::new(
        db.connection(),
        probe_of(vec![(
            "D:/work/demo".to_string(),
            "D:/work/demo".to_string(),
        )]),
    );

    let record = service.register(&request("D:/work/demo")).expect("登记");

    assert!(service.remove(&record.id).expect("删除"));
    assert!(!service.remove(&record.id).expect("重复删除"));
}

#[test]
fn registers_from_a_truly_cold_database_with_only_production_code() {
    // 冷启动纪律：空库 → 只调生产代码（ensure_local_target 与注册表无关，
    // projects 不依赖任何 harness 行），禁止夹具补前置状态。
    let db = Database::open_in_memory().expect("内存库");
    let service = ProjectService::new(
        db.connection(),
        probe_of(vec![(
            "D:/work/demo".to_string(),
            "D:/work/demo".to_string(),
        )]),
    );

    let record = service.register(&request("D:/work/demo")).expect("登记");
    let listed = service.list_recent(10).expect("列表");

    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0], record);
}

#[test]
fn sessions_can_be_filtered_by_project_once_registered() {
    let db = Database::open_in_memory().expect("内存库");
    let conn = db.connection();

    crate::runtime::local::ensure_local_target(conn).expect("runtime target");
    conn.execute(
        "INSERT INTO harnesses (id, display_name, created_at, updated_at)
         VALUES ('codex', 'Codex', ?1, ?1)",
        ["2026-10-12T00:00:00Z"],
    )
    .expect("harness 行");

    let service = ProjectService::new(
        conn,
        probe_of(vec![(
            "D:/work/demo".to_string(),
            "D:/work/demo".to_string(),
        )]),
    );
    let project = service.register(&request("D:/work/demo")).expect("登记");

    for (id, project_id, started_at) in [
        ("s1", Some(project.id.as_str()), "2026-10-12T00:00:01Z"),
        ("s2", Some(project.id.as_str()), "2026-10-12T00:00:02Z"),
        ("s3", None, "2026-10-12T00:00:03Z"),
    ] {
        conn.execute(
            "INSERT INTO sessions (
                hub_session_id, harness_id, project_id, runtime_target_id, status, launch_mode,
                started_at, created_at, updated_at
             ) VALUES (?1, 'codex', ?2, 'local', 'created', 'terminal',
                       ?3, ?3, ?3)",
            rusqlite::params![id, project_id, started_at],
        )
        .expect("session 行");
    }

    let sessions = service.sessions(&project.id, 10).expect("项目 session");
    let ids: Vec<&str> = sessions.iter().map(|s| s.hub_session_id.as_str()).collect();
    assert_eq!(ids, vec!["s2", "s1"], "只返回该项目的，按时间倒序");
}
