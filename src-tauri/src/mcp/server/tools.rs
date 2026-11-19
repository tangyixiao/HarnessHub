//! F22 S3 只读工具目录（spec §4.2 白名单五工具）。
//!
//! **此进程模式不提供任何写路径**（spec §2.4 红线）：本模块所有 handler
//! 只拿 `&Connection`，全部复用现有领域模块的只读投影
//! （`session::store` / `project::store` / `usage::summary` / `activity::query`），
//! 绝不写库、绝不起进程、绝不开端口（ADR-0012 同款纪律）。
//!
//! 参数纪律（spec §4.2）：
//! - 多余键一律忽略（每个 handler 只读自己声明的键）；
//! - `limit` 类参数：数字一律 clamp 进白名单范围，非法类型拒绝（isError）；
//! - 未知名工具：`isError: true` + 文本 `unknown tool: <name>`，不崩进程。
//!
//! 集中注册表：`TOOL_DEFS` 是唯一的工具目录事实来源，
//! `tools/list` 与 `tools/call` 都从它生成，两者不可能漂移。

use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Value};

/// spec §4.2：session_list 的 limit 范围（1..=100，缺省 50）。
/// 缺省与 IPC 层 `commands::DEFAULT_SESSION_LIMIT` 同值，但上限比 IPC 的 500 收得更紧。
const SESSION_LIST_DEFAULT: u32 = 50;
const SESSION_LIST_MAX: u32 = 100;
/// spec §4.2：activity_timeline 的 limit 范围（1..=200，缺省 100）。
const TIMELINE_DEFAULT: u32 = 100;
const TIMELINE_MAX: u32 = 200;
/// project_list 无参：与 IPC `list_projects` 的缺省条数同口径。
const PROJECT_LIST_LIMIT: u32 = crate::commands::DEFAULT_SESSION_LIMIT;

/// 工具 handler 的统一形状：只读连接 + 已验证的 arguments → 结果值或错误文本。
/// `Err(message)` 会被包成 MCP 的 `isError: true` 结果，绝不让进程崩掉。
type ToolHandler = fn(&Connection, &Value) -> Result<Value, String>;

/// 一个工具的 MCP `tools/list` 描述。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

/// 注册表条目：描述 + handler 成对出现，`tools/list` 与 `tools/call` 共用。
struct ToolDef {
    name: &'static str,
    description: &'static str,
    input_schema: fn() -> Value,
    handler: ToolHandler,
}

fn object_schema(properties: Value) -> Value {
    json!({ "type": "object", "properties": properties })
}

fn no_fields_schema() -> Value {
    object_schema(json!({}))
}

fn limit_schema(max: u32) -> Value {
    object_schema(json!({
        "limit": {
            "type": "integer",
            "minimum": 1,
            "maximum": max,
            "description": "返回条数上限；超出范围的值会被 clamp 进这个范围",
        }
    }))
}

// 具名 schema 函数：const 注册表里只放 fn 指针，不放闭包。
fn session_list_schema() -> Value {
    limit_schema(SESSION_LIST_MAX)
}

fn activity_timeline_schema() -> Value {
    limit_schema(TIMELINE_MAX)
}

/// spec §4.2 白名单五工具。顺序即 `tools/list` 的输出顺序。
const TOOL_DEFS: [ToolDef; 5] = [
    ToolDef {
        name: "harness_list",
        description: "列出 Harness Hub 已登记的 Harness 清单（检测结果的持久化投影：定义 + 安装）",
        input_schema: no_fields_schema,
        handler: harness_list,
    },
    ToolDef {
        name: "project_list",
        description: "列出最近登记的本地 Git 项目（按 last_opened_at / created_at 倒序）",
        input_schema: no_fields_schema,
        handler: project_list,
    },
    ToolDef {
        name: "session_list",
        description: "列出最近的会话（按开始时间倒序，limit 1..=100，缺省 50）",
        input_schema: session_list_schema,
        handler: session_list,
    },
    ToolDef {
        name: "usage_summary",
        description: "Usage Dashboard 的只读聚合投影（全时间窗口，UTC 日聚合）",
        input_schema: no_fields_schema,
        handler: usage_summary,
    },
    ToolDef {
        name: "activity_timeline",
        description: "Activity 活动流（usage + session 统一事件流，limit 1..=200，缺省 100）",
        input_schema: activity_timeline_schema,
        handler: activity_timeline,
    },
];

/// `tools/list` 的 MCP 结果（spec §4.2：从注册表自动生成）。
pub fn tools_list_result() -> Value {
    let tools: Vec<ToolSpec> = TOOL_DEFS
        .iter()
        .map(|def| ToolSpec {
            name: def.name,
            description: def.description,
            input_schema: (def.input_schema)(),
        })
        .collect();
    json!({ "tools": tools })
}

/// `tools/call` 的 MCP 结果。
///
/// 结构性参数问题（缺 name / arguments 不是对象）返回 `Err` → JSON-RPC `-32602`；
/// 工具层面的失败（未知工具、非法参数值、查询失败）一律收敛成
/// `isError: true` 的结果对象 —— 进程绝不因此退出。
pub fn tools_call_result(db: &Connection, params: &Value) -> Result<Value, String> {
    let object = params
        .as_object()
        .ok_or_else(|| "tools/call 的 params 必须是对象".to_string())?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "tools/call 需要 name 参数（字符串）".to_string())?;

    let arguments = match object.get("arguments") {
        None | Some(Value::Null) => Value::Object(serde_json::Map::new()),
        Some(value) if value.is_object() => value.clone(),
        Some(_) => return Err("tools/call 的 arguments 必须是对象".to_string()),
    };

    let Some(def) = TOOL_DEFS.iter().find(|def| def.name == name) else {
        // spec §4.2：未知名工具 → isError:true，不崩进程。
        return Ok(error_envelope(format!("unknown tool: {name}")));
    };

    match (def.handler)(db, &arguments) {
        Ok(value) => {
            // spec §4.2：text block 内是工具结果的 JSON.stringify。
            let text = serde_json::to_string(&value)
                .map_err(|error| format!("工具结果序列化失败：{error}"))?;
            Ok(ok_envelope(text))
        }
        Err(message) => Ok(error_envelope(message)),
    }
}

fn ok_envelope(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": false })
}

fn error_envelope(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

/// `limit` 参数：缺省（缺失）用 default；数字一律 clamp 到 1..=max；非数字拒绝。
/// 这是 spec §4.2「非法值 clamp/拒绝」的统一实现。
fn clamp_limit(arguments: &Value, default: u32, max: u32) -> Result<u32, String> {
    let Some(value) = arguments.get("limit") else {
        return Ok(default);
    };
    let number = value
        .as_f64()
        .ok_or_else(|| "limit 必须是数字".to_string())?;
    if !number.is_finite() {
        return Err("limit 必须是有限数字".to_string());
    }
    Ok(number.clamp(f64::from(1), f64::from(max)).round() as u32)
}

fn query_failed(tool: &str, error: impl std::fmt::Display) -> String {
    format!("{tool} 查询失败：{error}")
}

/* ------------------------------------------------------------------ *
 * 五个只读 handler（spec §4.2：每个 = 现有领域查询的 &Connection 直调）
 * ------------------------------------------------------------------ */

/// `harness_list`：SQLite 里持久化的 Harness 清单。
///
/// 为什么不是 `HarnessRegistry::summaries()`：那会逐个适配器跑真实检测
/// （探测二进制 / 版本），违反 spec §2.4「此进程模式只拿 `&Connection`、
/// 不起进程」。检测结果由 `harness::inventory::reconcile_harnesses`
/// 在应用启动 / 显式 Refresh 时落库，这里读的正是那份持久投影。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessDefinitionProjection {
    id: String,
    display_name: String,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HarnessInstallationProjection {
    id: String,
    harness_id: String,
    runtime_target_id: String,
    binary_path: Option<String>,
    version: Option<String>,
    availability: String,
}

fn harness_list(db: &Connection, _arguments: &Value) -> Result<Value, String> {
    let mut definitions = Vec::new();
    {
        let mut statement = db
            .prepare(
                "SELECT id, display_name, created_at, updated_at
                   FROM harnesses ORDER BY id ASC",
            )
            .map_err(|error| query_failed("harness_list", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok(HarnessDefinitionProjection {
                    id: row.get(0)?,
                    display_name: row.get(1)?,
                    created_at: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            })
            .map_err(|error| query_failed("harness_list", error))?;
        for row in rows {
            definitions.push(row.map_err(|error| query_failed("harness_list", error))?);
        }
    }

    let mut installations = Vec::new();
    {
        let mut statement = db
            .prepare(
                "SELECT id, harness_id, runtime_target_id, binary_path, version, availability
                   FROM harness_installations ORDER BY id ASC",
            )
            .map_err(|error| query_failed("harness_list", error))?;
        let rows = statement
            .query_map([], |row| {
                Ok(HarnessInstallationProjection {
                    id: row.get(0)?,
                    harness_id: row.get(1)?,
                    runtime_target_id: row.get(2)?,
                    binary_path: row.get(3)?,
                    version: row.get(4)?,
                    availability: row.get(5)?,
                })
            })
            .map_err(|error| query_failed("harness_list", error))?;
        for row in rows {
            installations.push(row.map_err(|error| query_failed("harness_list", error))?);
        }
    }

    serde_json::to_value(HarnessListProjection {
        definitions,
        installations,
    })
    .map_err(|error| query_failed("harness_list", error))
}

#[derive(Serialize)]
struct HarnessListProjection {
    definitions: Vec<HarnessDefinitionProjection>,
    installations: Vec<HarnessInstallationProjection>,
}

/// `project_list`：与 IPC `list_projects` 同款只读投影（`ProjectStore` 只接
/// `&Connection`），无参固定取缺省条数。
fn project_list(db: &Connection, _arguments: &Value) -> Result<Value, String> {
    let projects = crate::project::store::ProjectStore::new(db)
        .list_recent(PROJECT_LIST_LIMIT)
        .map_err(|error| query_failed("project_list", error))?;
    serde_json::to_value(projects).map_err(|error| query_failed("project_list", error))
}

/// `session_list`：与 IPC `list_sessions` 同款（`SessionService::list_recent`
/// 只接 `&Connection`），limit clamp 到 1..=100。
fn session_list(db: &Connection, arguments: &Value) -> Result<Value, String> {
    let limit = clamp_limit(arguments, SESSION_LIST_DEFAULT, SESSION_LIST_MAX)?;
    let sessions = crate::session::SessionService::new(db)
        .list_recent(limit)
        .map_err(|error| query_failed("session_list", error))?;
    serde_json::to_value(sessions).map_err(|error| query_failed("session_list", error))
}

/// `usage_summary`：Dashboard 只读聚合投影（`usage::summary` 只接 `&Connection`）。
///
/// 工具无参 → 固定 `all` 窗口（无边界；timezone offset 0 = UTC）。
/// 本地日聚合是桌面端 Dashboard 的职责；MCP 调用方拿 UTC 日口径。
fn usage_summary(db: &Connection, _arguments: &Value) -> Result<Value, String> {
    let window = crate::usage::summary::range_bounds(
        db,
        crate::usage::summary::UsageRange::All,
        &crate::clock::now_rfc3339(),
        0,
    )
    .map_err(|error| query_failed("usage_summary", error))?;
    let summary = crate::usage::summary::summary(db, &window)
        .map_err(|error| query_failed("usage_summary", error))?;
    serde_json::to_value(summary).map_err(|error| query_failed("usage_summary", error))
}

/// `activity_timeline`：Activity 只读投影（`activity::query::timeline` 只接
/// `&Connection`，ADR-0012），limit clamp 到 1..=200。
fn activity_timeline(db: &Connection, arguments: &Value) -> Result<Value, String> {
    let limit = clamp_limit(arguments, TIMELINE_DEFAULT, TIMELINE_MAX)?;
    let window = crate::usage::summary::range_bounds(
        db,
        crate::usage::summary::UsageRange::All,
        &crate::clock::now_rfc3339(),
        0,
    )
    .map_err(|error| query_failed("activity_timeline", error))?;
    let timeline = crate::activity::query::timeline(db, &window, limit)
        .map_err(|error| query_failed("activity_timeline", error))?;
    serde_json::to_value(timeline).map_err(|error| query_failed("activity_timeline", error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    use crate::test_support::{detected_codex_summary, empty_db, RUNTIME_TARGET_ID};

    /// 生产路径的种子：空库 → 引导 runtime target → reconcile（与冷启动一致）。
    fn db_with_seeded_harness() -> crate::db::Database {
        let db = empty_db();
        crate::runtime::local::ensure_local_target(db.connection()).expect("引导 runtime target");
        crate::harness::inventory::reconcile_harnesses(
            db.connection(),
            &[detected_codex_summary()],
            RUNTIME_TARGET_ID,
            crate::test_support::NOW,
        )
        .expect("同步 harness 清单");
        db
    }

    fn call(db: &Connection, tool: &str, arguments: Value) -> Value {
        tools_call_result(db, &json!({"name": tool, "arguments": arguments})).expect("参数结构合法")
    }

    // ---------- 注册表 ----------

    /// spec §4.2：白名单恰好五个工具，顺序固定（tools/list 自动生成）。
    #[test]
    fn the_registry_holds_exactly_the_five_readonly_tools() {
        let listing = tools_list_result();
        let names: Vec<&str> = listing["tools"]
            .as_array()
            .expect("tools 数组")
            .iter()
            .map(|tool| tool["name"].as_str().expect("name"))
            .collect();
        assert_eq!(
            names,
            vec![
                "harness_list",
                "project_list",
                "session_list",
                "usage_summary",
                "activity_timeline",
            ]
        );
    }

    /// 每个工具的描述与 schema 都非空（tools/list 的可用性底线）。
    #[test]
    fn every_tool_lists_a_description_and_an_object_schema() {
        for tool in tools_list_result()["tools"].as_array().expect("数组") {
            assert!(!tool["description"].as_str().expect("描述").is_empty());
            assert_eq!(tool["inputSchema"]["type"], "object");
        }
    }

    // ---------- 未知工具 ----------

    /// spec §4.2：未知名工具 → isError:true + 文本，不崩进程、也不是 JSON-RPC 错误。
    #[test]
    fn an_unknown_tool_is_an_error_envelope_not_a_crash() {
        let db = empty_db();
        let outcome = call(db.connection(), "not_a_tool", json!({}));
        assert_eq!(outcome["isError"], true);
        assert_eq!(outcome["content"][0]["type"], "text");
        assert_eq!(outcome["content"][0]["text"], "unknown tool: not_a_tool");
    }

    // ---------- 参数纪律 ----------

    /// 非数字 limit 拒绝（isError），数字一律 clamp（spec §4.2）。
    #[test]
    fn session_list_limit_is_clamped_and_bad_values_are_rejected() {
        let db = empty_db();

        // 缺省 50 / 显式 null 视为缺省形状只对「缺失」成立；这里断言 clamp：
        for (arguments, expected_len_desc) in [
            (json!({}), "缺省 50"),
            (json!({"limit": 1}), "下界"),
            (json!({"limit": 100}), "上界"),
            (json!({"limit": 0}), "0 clamp 到 1"),
            (json!({"limit": 999}), "999 clamp 到 100"),
            (json!({"limit": -50}), "负数 clamp 到 1"),
            (json!({"limit": 3, "extra": "多余键忽略"}), "多余键"),
        ] {
            let outcome = call(db.connection(), "session_list", arguments.clone());
            assert_eq!(
                outcome["isError"], false,
                "{expected_len_desc} 必须成功：{arguments} → {outcome}"
            );
            let rows: Vec<Value> =
                serde_json::from_str(outcome["content"][0]["text"].as_str().expect("text block"))
                    .expect("工具结果是 JSON 串");
            assert!(rows.len() <= 100, "{expected_len_desc}：{arguments}");
        }

        // 非数字 → isError 拒绝。
        for bad in [json!({"limit": "10"}), json!({"limit": true})] {
            let outcome = call(db.connection(), "session_list", bad.clone());
            assert_eq!(
                outcome["isError"], true,
                "非数字 limit 必须拒绝：{bad} → {outcome}"
            );
        }
    }

    #[test]
    fn activity_timeline_limit_is_clamped_and_bad_values_are_rejected() {
        let db = empty_db();

        for arguments in [
            json!({}),
            json!({"limit": 1}),
            json!({"limit": 200}),
            json!({"limit": 0}),
            json!({"limit": 99999}),
        ] {
            let outcome = call(db.connection(), "activity_timeline", arguments.clone());
            assert_eq!(
                outcome["isError"], false,
                "clamp 后必须成功：{arguments} → {outcome}"
            );
        }

        let outcome = call(
            db.connection(),
            "activity_timeline",
            json!({"limit": "big"}),
        );
        assert_eq!(outcome["isError"], true);
    }

    /// 结构性参数问题走 JSON-RPC 层（Err），不进 isError 信封。
    #[test]
    fn structural_param_violations_are_json_rpc_errors() {
        let db = empty_db();
        assert!(tools_call_result(db.connection(), &json!("not an object")).is_err());
        assert!(tools_call_result(db.connection(), &json!({"arguments": {}})).is_err());
        assert!(tools_call_result(
            db.connection(),
            &json!({"name": "harness_list", "arguments": [1]})
        )
        .is_err());
    }

    // ---------- 五工具的空库 / 种子库形状 ----------

    #[test]
    fn harness_list_returns_the_persisted_inventory_projection() {
        let db = db_with_seeded_harness();
        let outcome = call(db.connection(), "harness_list", json!({}));
        assert_eq!(outcome["isError"], false);
        let payload: Value =
            serde_json::from_str(outcome["content"][0]["text"].as_str().expect("text"))
                .expect("JSON 串");

        let definitions = payload["definitions"].as_array().expect("definitions");
        assert_eq!(definitions.len(), 1, "reconcile 种了 codex 定义");
        // 新序列化形状的 camelCase 契约（AGENTS.md 纪律）。
        assert_eq!(definitions[0]["id"], "codex");
        assert_eq!(definitions[0]["displayName"], "Codex");
        assert!(
            definitions[0].get("createdAt").is_some() && definitions[0].get("updatedAt").is_some()
        );

        let installations = payload["installations"].as_array().expect("installations");
        assert_eq!(installations.len(), 1);
        assert_eq!(installations[0]["harnessId"], "codex");
        assert_eq!(installations[0]["runtimeTargetId"], "local");
        assert_eq!(installations[0]["availability"], "available");
    }

    #[test]
    fn harness_list_on_an_empty_database_returns_empty_lists() {
        let db = empty_db();
        let outcome = call(db.connection(), "harness_list", json!({}));
        assert_eq!(outcome["isError"], false);
        let payload: Value =
            serde_json::from_str(outcome["content"][0]["text"].as_str().expect("text"))
                .expect("JSON 串");
        assert_eq!(payload["definitions"].as_array().unwrap().len(), 0);
        assert_eq!(payload["installations"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn project_list_returns_project_store_records() {
        let db = empty_db();
        crate::project::store::ProjectStore::new(db.connection())
            .upsert("proj-1", "Demo", "D:/tmp/demo", crate::test_support::NOW)
            .expect("登记项目");

        let outcome = call(db.connection(), "project_list", json!({}));
        assert_eq!(outcome["isError"], false);
        let rows: Vec<Value> =
            serde_json::from_str(outcome["content"][0]["text"].as_str().expect("text"))
                .expect("JSON 串");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "proj-1");
        assert_eq!(rows[0]["repoRoot"], "D:/tmp/demo");
    }

    #[test]
    fn session_list_returns_session_records_from_the_production_store() {
        let db = db_with_seeded_harness();
        crate::session::SessionService::new(db.connection())
            .start_from_installation("codex@local", None, None)
            .expect("登记会话");

        let outcome = call(db.connection(), "session_list", json!({"limit": 1}));
        assert_eq!(outcome["isError"], false);
        let rows: Vec<Value> =
            serde_json::from_str(outcome["content"][0]["text"].as_str().expect("text"))
                .expect("JSON 串");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["harnessId"], "codex");
        assert_eq!(rows[0]["status"], "created");
    }

    #[test]
    fn usage_summary_and_activity_timeline_are_safe_on_an_empty_database() {
        let db = empty_db();

        let outcome = call(db.connection(), "usage_summary", json!({}));
        assert_eq!(outcome["isError"], false, "空库不是错误：{outcome}");
        let payload: Value =
            serde_json::from_str(outcome["content"][0]["text"].as_str().expect("text"))
                .expect("JSON 串");
        assert_eq!(payload["eventCount"], 0);
        assert_eq!(payload["totals"]["totalTokens"], 0);

        let outcome = call(db.connection(), "activity_timeline", json!({}));
        assert_eq!(outcome["isError"], false);
        let payload: Value =
            serde_json::from_str(outcome["content"][0]["text"].as_str().expect("text"))
                .expect("JSON 串");
        assert_eq!(payload["events"].as_array().unwrap().len(), 0);
        assert_eq!(payload["hours"].as_array().unwrap().len(), 24, "恒 24 格");
        assert_eq!(payload["hasMore"], false);
    }
}
