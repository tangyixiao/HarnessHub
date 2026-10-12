//! Session 挂接/级联行为之外的 store 契约（Task 1 RED）。

use crate::db::Database;
use crate::project::store::ProjectStore;

fn empty_db() -> Database {
    Database::open_in_memory().expect("内存库")
}

#[test]
fn register_is_idempotent_per_repo_root() {
    let db = empty_db();
    let conn = db.connection();
    let store = ProjectStore::new(conn);

    let first = store.insert("proj-1", "demo", "D:/work/demo", "2026-10-12T00:00:00Z");
    let second = store.insert("proj-2", "other", "D:/work/demo", "2026-10-12T00:00:01Z");

    assert!(first.is_ok(), "首次插入成功");
    assert!(
        second.is_err(),
        "同一 repo_root 第二次插入必须被 UNIQUE 拒绝"
    );
    assert_eq!(store.count().expect("count"), 1);
}

#[test]
fn upsert_returns_the_existing_row_and_touches_last_opened_at() {
    let db = empty_db();
    let conn = db.connection();
    let store = ProjectStore::new(conn);

    store
        .insert("proj-1", "demo", "D:/work/demo", "2026-10-12T00:00:00Z")
        .expect("首次插入");

    let upserted = store
        .upsert(
            "proj-1",
            "demo (renamed)",
            "D:/work/demo",
            "2026-10-12T08:00:00Z",
        )
        .expect("幂等 upsert");

    assert_eq!(upserted.id, "proj-1");
    assert_eq!(upserted.name, "demo (renamed)");
    assert_eq!(
        upserted.last_opened_at.as_deref(),
        Some("2026-10-12T08:00:00Z"),
        "重复登记视为一次「打开」"
    );
    assert_eq!(upserted.created_at, "2026-10-12T00:00:00Z");
    assert_eq!(store.count().expect("count"), 1);
}

#[test]
fn list_recent_orders_by_last_opened_then_created() {
    let db = empty_db();
    let conn = db.connection();
    let store = ProjectStore::new(conn);

    store
        .insert("a", "alpha", "D:/a", "2026-10-01T00:00:00Z")
        .expect("a");
    store
        .insert("b", "bravo", "D:/b", "2026-10-02T00:00:00Z")
        .expect("b");
    store
        .insert("c", "charlie", "D:/c", "2026-09-01T00:00:00Z")
        .expect("c");
    store.touch("c", "2026-10-12T00:00:00Z").expect("touch c");

    let ids: Vec<String> = store
        .list_recent(10)
        .expect("列表")
        .into_iter()
        .map(|p| p.id)
        .collect();

    // c 有 last_opened_at（最新）→ b（created 较新）→ a
    assert_eq!(ids, vec!["c", "b", "a"]);
}

#[test]
fn list_recent_honours_limit() {
    let db = empty_db();
    let conn = db.connection();
    let store = ProjectStore::new(conn);

    for id in ["a", "b", "c"] {
        store
            .insert(id, id, &format!("D:/{id}"), "2026-10-01T00:00:00Z")
            .expect("插入");
    }

    let recent = store.list_recent(2).expect("列表");
    assert_eq!(recent.len(), 2);
}

#[test]
fn delete_removes_the_row_and_returns_false_when_missing() {
    let db = empty_db();
    let conn = db.connection();
    let store = ProjectStore::new(conn);

    store
        .insert("proj-1", "demo", "D:/work/demo", "2026-10-12T00:00:00Z")
        .expect("插入");

    assert!(store.delete("proj-1").expect("删除"));
    assert!(!store.delete("proj-1").expect("重复删除必须幂等返回 false"));
    assert_eq!(store.count().expect("count"), 0);
}

#[test]
fn deleting_a_project_detaches_sessions_and_cascades_settings() {
    // FK 行为（ON DELETE SET NULL / CASCADE）是注册表删除语义的一半，
    // 这里用真实外键验证，不 mock。
    let db = empty_db();
    let conn = db.connection();

    crate::runtime::local::ensure_local_target(conn).expect("runtime target");
    conn.execute(
        "INSERT INTO harnesses (id, display_name, created_at, updated_at)
         VALUES ('codex', 'Codex', ?1, ?1)",
        ["2026-10-12T00:00:00Z"],
    )
    .expect("harness 行");

    let store = ProjectStore::new(conn);
    store
        .insert("proj-1", "demo", "D:/work/demo", "2026-10-12T00:00:00Z")
        .expect("插入");

    conn.execute(
        "INSERT INTO sessions (
            hub_session_id, harness_id, project_id, runtime_target_id, status, launch_mode,
            started_at, created_at, updated_at
         ) VALUES ('s1', 'codex', 'proj-1', 'local', 'created', 'terminal',
                   '2026-10-12T00:00:00Z', '2026-10-12T00:00:00Z', '2026-10-12T00:00:00Z')",
        [],
    )
    .expect("session 行");
    conn.execute(
        "INSERT INTO project_harness_settings (project_id, harness_id, settings_json, updated_at)
         VALUES ('proj-1', 'codex', '{}', '2026-10-12T00:00:00Z')",
        [],
    )
    .expect("settings 行");

    assert!(store.delete("proj-1").expect("删除"));

    let detached: Option<String> = conn
        .query_row(
            "SELECT project_id FROM sessions WHERE hub_session_id = 's1'",
            [],
            |row| row.get(0),
        )
        .expect("session 还在");
    assert_eq!(detached, None, "session 保留，只是失去归属");

    let settings: i64 = conn
        .query_row(
            "SELECT count(*) FROM project_harness_settings WHERE project_id = 'proj-1'",
            [],
            |row| row.get(0),
        )
        .expect("settings 计数");
    assert_eq!(settings, 0, "项目级设置随项目级联删除");
}
