//! Registry CRUD：server 登记与绑定（SQLite 侧，值红线见 mod.rs）。

use std::collections::BTreeMap;

use rusqlite::{params, Connection, Row};
use serde::Deserialize;

use super::{McpBinding, McpServer, McpTransport};
use crate::error::{Error, Result};

/// 新增 / 更新 server 的输入。`id` 为 None = 新建。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInput {
    pub id: Option<String>,
    pub name: String,
    pub transport: McpTransport,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub env_keys: Vec<String>,
    pub enabled: bool,
}

const SERVER_COLUMNS: &str =
    "id, name, transport, command, args_json, url, env_keys, enabled, source, created_at, updated_at";

fn map_server(row: &Row<'_>) -> rusqlite::Result<McpServer> {
    let transport: String = row.get(2)?;
    let args_json: String = row.get(4)?;
    let env_keys_json: String = row.get(6)?;
    Ok(McpServer {
        id: row.get(0)?,
        name: row.get(1)?,
        transport: match transport.as_str() {
            "stdio" => McpTransport::Stdio,
            "http" => McpTransport::Http,
            other => unreachable!("CHECK 约束保证 transport 合法：{other}"),
        },
        command: row.get(3)?,
        args: serde_json::from_str(&args_json).unwrap_or_default(),
        url: row.get(5)?,
        env_keys: serde_json::from_str(&env_keys_json).unwrap_or_default(),
        enabled: row.get::<_, i64>(7)? != 0,
        source: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn parse_args_json(args: &[String]) -> String {
    serde_json::to_string(args).unwrap_or_else(|_| "[]".to_string())
}

/// server 名 = 配置文件里的键：限安全字符（防 TOML/JSON 键注入与展示歧义）。
fn validate_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidInput("MCP server 名不能为空".to_string()));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(Error::InvalidInput(format!(
            "MCP server 名只允许字母 / 数字 / . _ -：{trimmed}"
        )));
    }
    Ok(trimmed.to_string())
}

/// 新增或更新 server。校验：stdio 必须有 command、http 必须有 url。
///
/// 更新保持 `source` 与 `created_at` 不变（导入来源是历史事实，不由编辑改写）。
pub fn upsert_server(conn: &Connection, input: &McpServerInput) -> Result<McpServer> {
    let name = validate_name(&input.name)?;
    match input.transport {
        McpTransport::Stdio if input.command.as_deref().unwrap_or("").trim().is_empty() => {
            return Err(Error::InvalidInput(
                "stdio 传输必须提供 command".to_string(),
            ));
        }
        McpTransport::Http if input.url.as_deref().unwrap_or("").trim().is_empty() => {
            return Err(Error::InvalidInput("http 传输必须提供 url".to_string()));
        }
        _ => {}
    }

    let now = crate::clock::now_rfc3339();
    let args_json = parse_args_json(&input.args);
    let env_keys_json = parse_args_json(&input.env_keys);

    let inserted = match &input.id {
        Some(id) => conn.execute(
            "UPDATE mcp_servers SET name = ?2, transport = ?3, command = ?4, args_json = ?5,
             url = ?6, env_keys = ?7, enabled = ?8, updated_at = ?9 WHERE id = ?1",
            params![
                id,
                name,
                match input.transport {
                    McpTransport::Stdio => "stdio",
                    McpTransport::Http => "http",
                },
                input.command,
                args_json,
                input.url,
                env_keys_json,
                i64::from(input.enabled),
                now,
            ],
        ),
        None => conn.execute(
            "INSERT INTO mcp_servers (id, name, transport, command, args_json, url, env_keys,
             enabled, source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'hub', ?9, ?9)",
            params![
                uuid::Uuid::new_v4().to_string(),
                name,
                match input.transport {
                    McpTransport::Stdio => "stdio",
                    McpTransport::Http => "http",
                },
                input.command,
                args_json,
                input.url,
                env_keys_json,
                i64::from(input.enabled),
                now,
            ],
        ),
    };

    match inserted {
        Ok(updated) if updated > 0 => {
            find_server(conn, input.id.as_deref().unwrap_or(&name), &name)
        }
        Ok(_) => Err(Error::InvalidInput("MCP server 不存在".to_string())),
        Err(error) => Err(error.into()),
    }
}

/// 按 id（或新建后按 name）取回 server 行。
fn find_server(conn: &Connection, id: &str, name: &str) -> Result<McpServer> {
    let mut statement = conn.prepare(&format!(
        "SELECT {SERVER_COLUMNS} FROM mcp_servers WHERE id = ?1 OR name = ?1"
    ))?;
    // id 优先：新建时 id 与 name 都可能命中；按主键语义先试 id。
    match statement.query_row(params![id], map_server) {
        Ok(server) => Ok(server),
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            let mut statement = conn.prepare(&format!(
                "SELECT {SERVER_COLUMNS} FROM mcp_servers WHERE name = ?1"
            ))?;
            Ok(statement.query_row(params![name], map_server)?)
        }
        Err(error) => Err(error.into()),
    }
}

/// 全部 server（按 name 排序）。**纯读**。
pub fn list_servers(conn: &Connection) -> Result<Vec<McpServer>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {SERVER_COLUMNS} FROM mcp_servers ORDER BY name ASC"
    ))?;
    let rows = statement.query_map([], map_server)?;
    let mut servers = Vec::new();
    for row in rows {
        servers.push(row?);
    }
    Ok(servers)
}

/// 导入登记：把 harness 配置里读到的一条 MCP 条目注册进 Registry。
///
/// 返回 `Ok(false)` = 跳过（同名已存在——以 Hub 为准；或形态不可识别）。
/// env **只取变量名**（条目里 env 对象的键），值绝不入内。
pub fn register_imported(
    conn: &Connection,
    name: &str,
    source: &str,
    entry: &serde_json::Value,
) -> Result<bool> {
    let name = validate_name(name)?;
    let exists: i64 = conn.query_row(
        "SELECT count(*) FROM mcp_servers WHERE name = ?1",
        params![name],
        |row| row.get(0),
    )?;
    if exists > 0 {
        return Ok(false);
    }

    let Some(object) = entry.as_object() else {
        return Ok(false);
    };
    let url = object
        .get("url")
        .or_else(|| object.get("httpUrl"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let is_http = url.is_some()
        || object
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value == "http");
    let command = object
        .get("command")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);

    let (transport, command, url) = if is_http {
        let Some(url) = url else {
            return Ok(false);
        };
        (McpTransport::Http, None, Some(url))
    } else if let Some(command) = command {
        (McpTransport::Stdio, Some(command), None)
    } else {
        return Ok(false);
    };

    let args: Vec<String> = object
        .get("args")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let env_keys: Vec<String> = object
        .get("env")
        .and_then(serde_json::Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();

    let now = crate::clock::now_rfc3339();
    conn.execute(
        "INSERT INTO mcp_servers (id, name, transport, command, args_json, url, env_keys,
         enabled, source, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?9)",
        params![
            uuid::Uuid::new_v4().to_string(),
            name,
            match transport {
                McpTransport::Stdio => "stdio",
                McpTransport::Http => "http",
            },
            command,
            parse_args_json(&args),
            url,
            parse_args_json(&env_keys),
            source,
            now,
        ],
    )?;
    Ok(true)
}

/// 删除 server（绑定级联）。返回是否真的删了。
pub fn remove_server(conn: &Connection, id: &str) -> Result<bool> {
    let deleted = conn.execute("DELETE FROM mcp_servers WHERE id = ?1", params![id])?;
    Ok(deleted > 0)
}

const BINDING_COLUMNS: &str = "b.id, b.server_id, s.name, b.scope, b.harness_id, b.created_at";

fn map_binding(row: &Row<'_>) -> rusqlite::Result<McpBinding> {
    Ok(McpBinding {
        id: row.get(0)?,
        server_id: row.get(1)?,
        server_name: row.get(2)?,
        scope: row.get(3)?,
        harness_id: row.get(4)?,
        created_at: row.get(5)?,
    })
}

/// 添加绑定。scope：`global` 或 `harness`（harness_id 必填且必须是已注册 harness）。
///
/// 同 (server, scope) 重复返回已有绑定（幂等，UI 勾选语义）。
pub fn add_binding(
    conn: &Connection,
    server_id: &str,
    scope: &str,
    harness_id: Option<&str>,
) -> Result<McpBinding> {
    let server_exists: i64 = conn.query_row(
        "SELECT count(*) FROM mcp_servers WHERE id = ?1",
        params![server_id],
        |row| row.get(0),
    )?;
    if server_exists == 0 {
        return Err(Error::InvalidInput(format!(
            "MCP server 不存在：{server_id}"
        )));
    }

    let scope_key = match (scope, harness_id) {
        ("global", None) => "global".to_string(),
        ("harness", Some(harness)) => {
            let known: i64 = conn.query_row(
                "SELECT count(*) FROM harnesses WHERE id = ?1",
                params![harness],
                |row| row.get(0),
            )?;
            if known == 0 {
                return Err(Error::InvalidInput(format!("未注册的 Harness：{harness}")));
            }
            format!("harness:{harness}")
        }
        _ => {
            return Err(Error::InvalidInput(
                "scope 必须是 global（无 harness）或 harness（带 harness_id）".to_string(),
            ))
        }
    };

    let existing = conn
        .query_row(
            &format!(
                "SELECT {BINDING_COLUMNS} FROM mcp_bindings b
                 JOIN mcp_servers s ON s.id = b.server_id
                 WHERE b.server_id = ?1 AND b.scope_key = ?2"
            ),
            params![server_id, scope_key],
            map_binding,
        )
        .ok();
    if let Some(binding) = existing {
        return Ok(binding);
    }

    conn.execute(
        "INSERT INTO mcp_bindings (id, server_id, scope, harness_id, scope_key, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            uuid::Uuid::new_v4().to_string(),
            server_id,
            scope,
            harness_id,
            scope_key,
            crate::clock::now_rfc3339()
        ],
    )?;

    let mut statement = conn.prepare(&format!(
        "SELECT {BINDING_COLUMNS} FROM mcp_bindings b
         JOIN mcp_servers s ON s.id = b.server_id
         WHERE b.server_id = ?1 AND b.scope_key = ?2"
    ))?;
    Ok(statement.query_row(params![server_id, scope_key], map_binding)?)
}

/// 全部绑定（server name 排序）。**纯读**。
pub fn list_bindings(conn: &Connection) -> Result<Vec<McpBinding>> {
    let mut statement = conn.prepare(&format!(
        "SELECT {BINDING_COLUMNS} FROM mcp_bindings b
         JOIN mcp_servers s ON s.id = b.server_id
         ORDER BY s.name ASC, b.scope ASC"
    ))?;
    let rows = statement.query_map([], map_binding)?;
    let mut bindings = Vec::new();
    for row in rows {
        bindings.push(row?);
    }
    Ok(bindings)
}

/// 删除绑定。返回是否真的删了。
pub fn remove_binding(conn: &Connection, id: &str) -> Result<bool> {
    let deleted = conn.execute("DELETE FROM mcp_bindings WHERE id = ?1", params![id])?;
    Ok(deleted > 0)
}

/// 某 harness 的 desired 集：global ∪ harness 绑定中 enabled 的 server。
///
/// name → server；同名冲突不可能（UNIQUE name）。
pub fn desired_servers(conn: &Connection, harness_id: &str) -> Result<BTreeMap<String, McpServer>> {
    let mut statement = conn.prepare(&format!(
        "SELECT DISTINCT {} FROM mcp_bindings b
         JOIN mcp_servers s ON s.id = b.server_id
         WHERE b.scope_key IN ('global', ?1) AND s.enabled = 1
         ORDER BY s.name ASC",
        SERVER_COLUMNS
            .split(", ")
            .map(|column| format!("s.{column}"))
            .collect::<Vec<_>>()
            .join(", ")
    ))?;
    let rows = statement.query_map(params![format!("harness:{harness_id}")], map_server)?;
    let mut desired = BTreeMap::new();
    for row in rows {
        let server = row?;
        desired.insert(server.name.clone(), server);
    }
    Ok(desired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{empty_db, HARNESS_ID};

    fn db() -> crate::db::Database {
        empty_db()
    }

    fn seed_harness(db: &crate::db::Database) {
        db.connection()
            .execute(
                "INSERT INTO harnesses (id, display_name, created_at, updated_at)
                 VALUES (?1, 'Codex', 't', 't')",
                params![HARNESS_ID],
            )
            .unwrap();
    }

    fn stdio_input(name: &str) -> McpServerInput {
        McpServerInput {
            id: None,
            name: name.to_string(),
            transport: McpTransport::Stdio,
            command: Some("npx".to_string()),
            args: vec!["-y".to_string(), "server".to_string()],
            url: None,
            env_keys: vec!["API_KEY".to_string()],
            enabled: true,
        }
    }

    #[test]
    fn upsert_creates_and_updates_preserving_source() {
        let db = db();
        let conn = db.connection();

        let created = upsert_server(conn, &stdio_input("fetch")).unwrap();
        assert_eq!(created.name, "fetch");
        assert_eq!(created.transport, McpTransport::Stdio);
        assert_eq!(created.args, vec!["-y", "server"]);
        assert_eq!(created.env_keys, vec!["API_KEY"]);
        assert_eq!(created.source, "hub");

        // 更新：改 command；source / created_at 保持。
        let update = McpServerInput {
            id: Some(created.id.clone()),
            name: "fetch".to_string(),
            transport: McpTransport::Stdio,
            command: Some("node".to_string()),
            args: vec![],
            url: None,
            env_keys: vec![],
            enabled: false,
        };
        let updated = upsert_server(conn, &update).unwrap();
        assert_eq!(updated.id, created.id);
        assert_eq!(updated.command.as_deref(), Some("node"));
        assert!(!updated.enabled);
        assert_eq!(updated.source, "hub");
        assert_eq!(updated.created_at, created.created_at);

        let listed = list_servers(conn).unwrap();
        assert_eq!(listed.len(), 1);
    }

    #[test]
    fn stdio_requires_command_and_http_requires_url() {
        let db = db();
        let conn = db.connection();

        let bad_stdio = McpServerInput {
            transport: McpTransport::Stdio,
            command: None,
            url: Some("https://x".to_string()),
            ..stdio_input("bad")
        };
        assert!(upsert_server(conn, &bad_stdio).is_err());

        let bad_http = McpServerInput {
            transport: McpTransport::Http,
            command: Some("npx".to_string()),
            url: None,
            ..stdio_input("bad2")
        };
        assert!(upsert_server(conn, &bad_http).is_err());
    }

    #[test]
    fn server_name_is_restricted() {
        let db = db();
        let conn = db.connection();

        let evil = McpServerInput {
            name: "bad name;drop".to_string(),
            ..stdio_input("x")
        };
        let error = upsert_server(conn, &evil).unwrap_err();
        assert!(error.to_string().contains("只允许"), "{error}");
    }

    #[test]
    fn bindings_are_idempotent_and_cascade() {
        let db = db();
        seed_harness(&db);
        let conn = db.connection();
        let server = upsert_server(conn, &stdio_input("fetch")).unwrap();

        // global + harness 绑定。
        let global = add_binding(conn, &server.id, "global", None).unwrap();
        assert_eq!(global.scope, "global");
        assert!(global.harness_id.is_none());

        let harness = add_binding(conn, &server.id, "harness", Some(HARNESS_ID)).unwrap();
        assert_eq!(harness.harness_id.as_deref(), Some(HARNESS_ID));

        // 重复添加幂等：同一行。
        let again = add_binding(conn, &server.id, "global", None).unwrap();
        assert_eq!(again.id, global.id);
        assert_eq!(list_bindings(conn).unwrap().len(), 2);

        // 未注册 harness 拒绝。
        assert!(add_binding(conn, &server.id, "harness", Some("nope")).is_err());

        // server 删除 → 绑定级联。
        remove_server(conn, &server.id).unwrap();
        assert!(list_bindings(conn).unwrap().is_empty());
    }

    #[test]
    fn desired_merges_global_and_harness_bindings() {
        let db = db();
        seed_harness(&db);
        let conn = db.connection();

        let a = upsert_server(conn, &stdio_input("fetch")).unwrap();
        let b = upsert_server(conn, &stdio_input("search")).unwrap();
        let c = upsert_server(
            conn,
            &McpServerInput {
                enabled: false,
                ..stdio_input("disabled")
            },
        )
        .unwrap();

        add_binding(conn, &a.id, "global", None).unwrap();
        add_binding(conn, &b.id, "harness", Some(HARNESS_ID)).unwrap();
        add_binding(conn, &c.id, "global", None).unwrap();

        let desired = desired_servers(conn, HARNESS_ID).unwrap();
        assert_eq!(desired.len(), 2, "disabled 的不进 desired");
        assert!(desired.contains_key("fetch"));
        assert!(desired.contains_key("search"));

        // 其他 harness 只有 global。
        let other = desired_servers(conn, "gemini").unwrap();
        assert_eq!(other.len(), 1, "harness 绑定不跨 harness");
        assert!(other.contains_key("fetch"));
    }

    /// 跨 IPC 契约测试（AGENTS.md）：前端 normalize 依赖这些键。
    #[test]
    fn mcp_dtos_serialize_with_camel_case_keys() {
        let server = McpServer {
            id: "s1".to_string(),
            name: "fetch".to_string(),
            transport: McpTransport::Stdio,
            command: Some("npx".to_string()),
            args: vec!["-y".to_string()],
            url: None,
            env_keys: vec!["API_KEY".to_string()],
            enabled: true,
            source: "hub".to_string(),
            created_at: "t".to_string(),
            updated_at: "t".to_string(),
        };
        let json = serde_json::to_value(&server).unwrap();
        assert_eq!(json["transport"], "stdio");
        assert_eq!(json["envKeys"], serde_json::json!(["API_KEY"]));
        assert_eq!(json["createdAt"], "t");
        assert!(json.get("env_keys").is_none(), "不得两套契约并存");

        let input: McpServerInput = serde_json::from_value(serde_json::json!({
            "id": null,
            "name": "fetch",
            "transport": "http",
            "command": null,
            "args": [],
            "url": "https://mcp.example.com",
            "envKeys": [],
            "enabled": true
        }))
        .unwrap();
        assert_eq!(
            input.transport,
            McpTransport::Http,
            "输入侧也是 snake_case 传输名"
        );

        let binding = McpBinding {
            id: "b1".to_string(),
            server_id: "s1".to_string(),
            server_name: "fetch".to_string(),
            scope: "harness".to_string(),
            harness_id: Some("claude".to_string()),
            created_at: "t".to_string(),
        };
        let json = serde_json::to_value(&binding).unwrap();
        assert_eq!(json["serverId"], "s1");
        assert_eq!(json["serverName"], "fetch");
        assert_eq!(json["harnessId"], "claude");
    }
}
