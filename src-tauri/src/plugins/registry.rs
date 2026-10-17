//! 插件注册表的 **store 层**（migration 0009 的 `plugins` 表）。
//!
//! 这里只做「行 ↔ 结构体」，不做扫描、不做 spawn、不做校验：
//! 发现目录是 [`super::discovery`] 的事，manifest 合法性是 [`super::manifest`] 的事，
//! 进程状态是 [`super::supervisor`] 的事。把这三件事混进来，store 就没法在
//! 没有文件系统、没有子进程的单测里被验证。
//!
//! 三条与表结构绑定的约定：
//!
//! 1. **`registered_at` 只写一次**。首次登记时间才是审计关心的；后续 refresh 覆盖
//!    name/version/api_version/path/capabilities/status，但必须保留原 `registered_at`
//!    （见 [`upsert_plugin`] 的 `ON CONFLICT` 分支——它**刻意不出现** registered_at）。
//! 2. **`status` 由数据库 CHECK 兜底**（`ready` / `degraded`）。[`set_status`] 不预先验证
//!    字面量：非法值要让 SQLite 报错，而不是被 Rust 层悄悄吞掉——这样「新增状态忘了改
//!    迁移」一定会在测试里炸，而不是变成一行永远不会被读到的数据。
//! 3. **`list_plugins` 按 id 升序**。插件顺序必须稳定：UI 列表抖动会把「插件消失/新增」
//!    演成视觉噪音，也给端到端测试引入 flaky 断言。

use rusqlite::{params, Connection};

use crate::error::Result;

/// 插件状态：可以正常调用。
pub const STATUS_READY: &str = "ready";
/// 插件状态：进程崩过 / 握手失败，宿主仍可尝试 lazy respawn。
pub const STATUS_DEGRADED: &str = "degraded";

/// `plugins` 表的一行。
///
/// 字段顺序与 [`SELECT_COLUMNS`] 一一对应；`capabilities_json` / `skills_cache_json`
/// 是**原文透传**的 JSON 文本，store 不解析（解析在 manifest 层，且失败应在那里报错）。
#[derive(Debug, Clone, PartialEq)]
pub struct PluginRecord {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub path: String,
    pub capabilities_json: String,
    pub skills_cache_json: Option<String>,
    pub status: String,
    pub registered_at: String,
}

const SELECT_COLUMNS: &str = "id, name, version, api_version, path, capabilities_json, \
     skills_cache_json, status, registered_at";

fn map_plugin_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PluginRecord> {
    Ok(PluginRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        version: row.get(2)?,
        api_version: row.get(3)?,
        path: row.get(4)?,
        capabilities_json: row.get(5)?,
        skills_cache_json: row.get(6)?,
        status: row.get(7)?,
        registered_at: row.get(8)?,
    })
}

/// 幂等 upsert：同 id 覆盖可变字段，但**保留原有 `registered_at`**。
///
/// 为什么用 `ON CONFLICT (id) DO UPDATE` 而不是「先查后写」：
/// 探询再插入在两次 refresh 并发时会互相覆盖，而且需要两倍往返。
/// 单条 upsert 由 SQLite 保证原子性——这也是全仓 usage 导入用的同一手法。
///
/// 注意 `registered_at` **只出现在 VALUES 里**，不在 `DO UPDATE SET` 里：
/// 这正是「首次登记时间不变」的实现方式，不是靠调用方传对一个旧时间戳。
/// `skills_cache_json` 同理不在覆盖列表：它由 [`set_skills_cache`] 单独维护，
/// 一次刷新登记不该把已拉到的缓存清成 NULL。
pub fn upsert_plugin(conn: &Connection, record: &PluginRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO plugins (
            id, name, version, api_version, path, capabilities_json,
            skills_cache_json, status, registered_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        ON CONFLICT (id) DO UPDATE SET
            name = excluded.name,
            version = excluded.version,
            api_version = excluded.api_version,
            path = excluded.path,
            capabilities_json = excluded.capabilities_json,
            status = excluded.status",
        params![
            record.id,
            record.name,
            record.version,
            record.api_version,
            record.path,
            record.capabilities_json,
            record.skills_cache_json,
            record.status,
            record.registered_at,
        ],
    )?;
    Ok(())
}

/// 全部已登记插件，按 id 升序（顺序必须稳定，见模块头）。
pub fn list_plugins(conn: &Connection) -> Result<Vec<PluginRecord>> {
    let sql = format!("SELECT {SELECT_COLUMNS} FROM plugins ORDER BY id ASC");
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map([], map_plugin_row)?;

    let mut plugins = Vec::new();
    for row in rows {
        plugins.push(row?);
    }
    Ok(plugins)
}

/// 按 id 取一行；不存在返回 `Ok(None)`（「没装」是正常状态，不是错误）。
pub fn get_plugin(conn: &Connection, id: &str) -> Result<Option<PluginRecord>> {
    let sql = format!("SELECT {SELECT_COLUMNS} FROM plugins WHERE id = ?1");
    let mut statement = conn.prepare(&sql)?;
    let mut rows = statement.query_map(params![id], map_plugin_row)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// 覆盖 status。
///
/// 不做前置校验：非法取值必须由 SQLite 的 CHECK 拒绝（见模块头第 2 条）。
/// 影响 0 行**不**报错——插件可能已被卸载，调用方（supervisor 的降级路径）
/// 不该因为「插件刚好不在了」而在错误处理里再炸一次。
pub fn set_status(conn: &Connection, id: &str, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE plugins SET status = ?2 WHERE id = ?1",
        params![id, status],
    )?;
    Ok(())
}

/// 覆盖 `skills_cache_json`（注册时拉取到的 skill 列表快照）。
///
/// 传 `"[]"` 表示「拉过，零个 skill」，与 `NULL`（还没拉过）语义不同——调用方负责区分。
pub fn set_skills_cache(conn: &Connection, id: &str, cache: &str) -> Result<()> {
    conn.execute(
        "UPDATE plugins SET skills_cache_json = ?2 WHERE id = ?1",
        params![id, cache],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::error::Error;

    /// 完整的一行；测试各自只改关心的字段。
    fn record(id: &str) -> PluginRecord {
        PluginRecord {
            id: id.to_string(),
            name: format!("Plugin {id}"),
            version: "0.1.0".to_string(),
            api_version: "1".to_string(),
            path: format!("D:/plugins/{id}"),
            capabilities_json: r#"{"usage":true,"skills":false}"#.to_string(),
            skills_cache_json: None,
            status: STATUS_READY.to_string(),
            registered_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    fn db() -> Database {
        // 走生产迁移路径（不是手写 schema）：store 与 SQL 的契约必须一起被验证。
        Database::open_in_memory().expect("打开内存库")
    }

    #[test]
    fn upsert_then_list_round_trips_every_field() {
        let db = db();
        let conn = db.connection();
        let mut expected = record("usage-connect");
        expected.skills_cache_json = Some(r#"[{"skill_id":"demo"}]"#.to_string());

        upsert_plugin(conn, &expected).expect("写入");
        let listed = list_plugins(conn).expect("列插件");

        assert_eq!(listed, vec![expected]);
    }

    /// 二次登记只更新可变字段：`registered_at` 必须还是第一次的值。
    #[test]
    fn second_upsert_keeps_the_first_registered_at() {
        let db = db();
        let conn = db.connection();
        let first = record("usage-connect");
        upsert_plugin(conn, &first).expect("首次登记");

        let mut second = record("usage-connect");
        second.name = "Usage Connect (renamed)".to_string();
        second.version = "0.2.0".to_string();
        second.path = "E:/elsewhere/usage-connect".to_string();
        second.capabilities_json = r#"{"usage":false,"skills":true}"#.to_string();
        second.status = STATUS_DEGRADED.to_string();
        second.registered_at = "2026-06-06T12:00:00Z".to_string();
        upsert_plugin(conn, &second).expect("二次登记");

        let stored = get_plugin(conn, "usage-connect")
            .expect("读取")
            .expect("插件必须还在");

        assert_eq!(stored.name, "Usage Connect (renamed)");
        assert_eq!(stored.version, "0.2.0");
        assert_eq!(stored.path, "E:/elsewhere/usage-connect");
        assert_eq!(stored.capabilities_json, r#"{"usage":false,"skills":true}"#);
        assert_eq!(stored.status, STATUS_DEGRADED);
        assert_eq!(
            stored.registered_at, "2026-01-01T00:00:00Z",
            "registered_at 必须是首次登记时间，不能被 refresh 覆盖"
        );
        assert_eq!(
            list_plugins(conn).expect("列插件").len(),
            1,
            "不得插出第二行"
        );
    }

    /// 非法 status 必须由数据库 CHECK 拒绝（不是被 Rust 层吞掉）。
    #[test]
    fn set_status_rejects_a_value_outside_the_check_constraint() {
        let db = db();
        let conn = db.connection();
        upsert_plugin(conn, &record("hello-skill")).expect("登记");

        // 正向对照：合法值必须写得进去，否则「拒绝」可能只是因为 SQL 写错了。
        set_status(conn, "hello-skill", STATUS_DEGRADED).expect("degraded 合法");
        assert_eq!(
            get_plugin(conn, "hello-skill")
                .expect("读取")
                .expect("存在")
                .status,
            STATUS_DEGRADED
        );

        let error = set_status(conn, "hello-skill", "enabled").expect_err("非法 status 必须报错");

        assert!(
            matches!(error, Error::Database(_)),
            "必须是 SQLite 的 CHECK 失败（Error::Database），实际：{error:?}"
        );
        assert!(
            error.to_string().contains("CHECK"),
            "错误信息应来自 CHECK 约束，实际：{error}"
        );
        assert_eq!(
            get_plugin(conn, "hello-skill")
                .expect("读取")
                .expect("存在")
                .status,
            STATUS_DEGRADED,
            "被拒绝的写入不得改动原值"
        );
    }

    #[test]
    fn skills_cache_round_trips_and_null_means_never_fetched() {
        let db = db();
        let conn = db.connection();
        upsert_plugin(conn, &record("hello-skill")).expect("登记");
        assert_eq!(
            get_plugin(conn, "hello-skill")
                .expect("读取")
                .expect("存在")
                .skills_cache_json,
            None,
            "新登记是 NULL（还没拉过），不是空数组"
        );

        let cache = r#"[{"skill_id":"demo","name":"Demo"}]"#;
        set_skills_cache(conn, "hello-skill", cache).expect("写缓存");

        assert_eq!(
            get_plugin(conn, "hello-skill")
                .expect("读取")
                .expect("存在")
                .skills_cache_json
                .as_deref(),
            Some(cache)
        );

        // 再次 refresh 登记不得清掉已拉到的缓存。
        upsert_plugin(conn, &record("hello-skill")).expect("二次登记");
        assert_eq!(
            get_plugin(conn, "hello-skill")
                .expect("读取")
                .expect("存在")
                .skills_cache_json
                .as_deref(),
            Some(cache),
            "upsert 不该把 skills_cache_json 抹回 NULL"
        );
    }

    #[test]
    fn list_order_is_stable_and_ascending_by_id() {
        let db = db();
        let conn = db.connection();
        // 刻意乱序写入，证明顺序来自 SQL 而不是插入顺序。
        for id in ["zulu-plugin", "alpha-plugin", "mike-plugin"] {
            upsert_plugin(conn, &record(id)).expect("登记");
        }

        let ids: Vec<String> = list_plugins(conn)
            .expect("列插件")
            .into_iter()
            .map(|plugin| plugin.id)
            .collect();

        assert_eq!(ids, vec!["alpha-plugin", "mike-plugin", "zulu-plugin"]);
    }

    #[test]
    fn get_plugin_returns_none_for_an_unknown_id() {
        let db = db();

        assert_eq!(
            get_plugin(db.connection(), "never-installed").expect("查询"),
            None,
            "「没装」是正常状态，不是错误"
        );
    }

    /// 影响 0 行的更新不算错误：插件可能刚被卸载，降级路径不该再炸一次。
    #[test]
    fn updating_a_missing_plugin_is_a_no_op() {
        let db = db();
        let conn = db.connection();

        set_status(conn, "gone", STATUS_DEGRADED).expect("插件不存在也不报错");
        set_skills_cache(conn, "gone", "[]").expect("插件不存在也不报错");

        assert!(list_plugins(conn).expect("列插件").is_empty());
    }
}
