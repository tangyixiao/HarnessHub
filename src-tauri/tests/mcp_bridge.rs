//! F22 S2 集成测试：MCP client bridge + 只读策略
//! （spec 2026-11-09-f22-mcp-client-server-design.md §3.8 / §3.9）。
//!
//! **不需要真 Python**：`SidecarManager` 用「解释器不存在」的配置构造（懒启动、
//! 不 spawn），专门验证 fail-closed 收敛路径；decoder 与 RPC 参数契约用合成
//! payload 逐键锁定。真机两侧对上的回归在 S1 合入后跑 `sidecar_lifecycle`。
//!
//! 冷启动纪律（AGENTS.md）：DB 前置状态只走生产路径
//! （`Database::open_in_memory` + `registry::upsert_server`），不手写 INSERT。

use harness_hub_lib::db::Database;
use harness_hub_lib::mcp::client_bridge::{
    connect_params, decode_mcp_status, decode_mcp_status_counted, decode_server_entry,
    decode_tool_call_result, decode_tools_list, find_server_row, merge_server_states,
    runtime_status, tool_call, tool_call_params, tools_list, McpConnectOutcome, McpRuntimeStatus,
    McpServerRow, McpToolCallOutcome, McpToolDescriptorDto, MAX_DESCRIPTION_LEN,
    MAX_LAST_ERROR_LEN, MAX_SCHEMA_KEYS, MAX_SERVERS, MAX_SERVER_INFO_LEN, MAX_TEXT_PREVIEW_LEN,
    MAX_TOOLS, MCP_PROTOCOL_VERSION,
};
use harness_hub_lib::mcp::policy::evaluate_tool_call;
use harness_hub_lib::mcp::registry::{upsert_server, McpServerInput};
use harness_hub_lib::mcp::McpTransport;
use harness_hub_lib::sidecar::config::SidecarConfig;
use harness_hub_lib::sidecar::manager::SidecarManager;
use serde_json::{json, Value};

// ────────────────────────────────────────────────────────────────────────────
// §3.8 策略 v1 冻结规则：表驱动正反例
// ────────────────────────────────────────────────────────────────────────────

/// 12 个只读动词的正例（大小写 / 两种分隔符 / 首尾空白都走同一管线）。
const READONLY_TOOLS: &[&str] = &[
    "get_forecast",
    "GET",
    "List-Files",
    "search_web",
    "read_config",
    "FETCH_URL",
    "query_db",
    "find-user",
    "describe_table",
    "info",
    "status_probe",
    "health",
    "stats_daily",
    "  get_forecast  ",
];

#[test]
fn policy_allows_the_frozen_readonly_verbs_case_insensitively() {
    for tool in READONLY_TOOLS {
        let decision = evaluate_tool_call("weather", tool);
        assert!(decision.allowed, "{tool:?} 应按只读启发式放行");
        assert_eq!(
            decision.reason, "readonly_by_default",
            "{tool:?} 的 reason 必须是冻结字符串"
        );
    }
}

/// 反例：第一段不在动词集 → 默认拒绝；`_x` 首段为空同样不在动词集。
const NON_READONLY_TOOLS: &[&str] = &[
    "write_file",
    "delete_user",
    "create_issue",
    "update_config",
    "send_message",
    "run_command",
    "exec",
    "set_theme",
    "patch_notes",
    "_hidden",
];

#[test]
fn policy_denies_non_readonly_tools_with_frozen_reason() {
    for tool in NON_READONLY_TOOLS {
        let decision = evaluate_tool_call("weather", tool);
        assert!(!decision.allowed, "{tool:?} 必须默认拒绝");
        assert_eq!(
            decision.reason, "not_readonly_by_default",
            "{tool:?} 的 reason 必须是冻结字符串"
        );
    }
}

/// 非法名 fail-closed：空名 / 纯空白 / 内部空白 / 非法字符（含非 ASCII）。
const INVALID_TOOLS: &[&str] = &[
    "",
    "   ",
    "\t",
    "get forecast",
    "list\tfiles",
    "read\nme",
    "get.forecast",
    "fetch:url",
    "query-db!",
    "获取数据",
    "read/me",
];

#[test]
fn policy_denies_invalid_tool_names_fail_closed() {
    for tool in INVALID_TOOLS {
        let decision = evaluate_tool_call("weather", tool);
        assert!(!decision.allowed, "{tool:?} 必须 fail-closed");
        assert_eq!(decision.reason, "invalid_tool_name", "{tool:?}");
    }
}

/// v1 规则只看 tool 名：换 server 名不得改变判定（冻结行为的防漂移锁）。
#[test]
fn policy_decision_does_not_depend_on_server_name() {
    for tool in READONLY_TOOLS {
        assert_eq!(
            evaluate_tool_call("weather", tool),
            evaluate_tool_call("other-server", tool),
            "{tool:?} 的判定不得依赖 server 名"
        );
    }
}

// ────────────────────────────────────────────────────────────────────────────
// §3.2 mcp.status fail-closed decoder
// ────────────────────────────────────────────────────────────────────────────

/// 合法的单条 status 条目（spec §3.2 形状）。
fn valid_entry() -> Value {
    json!({
        "name": "weather",
        "transport": "stdio",
        "state": "ready",
        "protocolVersion": "2025-06-18",
        "serverInfo": { "name": "weather-server", "version": "1.2.0" },
        "toolCount": 3,
        "lastError": null,
        "envKeys": ["WEATHER_API_KEY"]
    })
}

#[test]
fn decode_mcp_status_is_empty_on_non_object_or_bad_shape_payloads() {
    let bad_payloads = [
        json!(null),
        json!([]),
        json!("not-an-object"),
        json!(42),
        json!({}),
        json!({"servers": 42}),
        json!({"servers": "not-an-array"}),
        json!({"servers": null}),
    ];
    for (index, payload) in bad_payloads.iter().enumerate() {
        let decoded = decode_mcp_status(payload);
        assert!(
            decoded.is_empty(),
            "坏 payload #{index} 必须收敛为空列表，实际 {decoded:?}"
        );
        let (decoded, skipped) = decode_mcp_status_counted(payload);
        assert!(decoded.is_empty(), "坏 payload #{index}");
        assert_eq!(skipped, 0, "坏 payload #{index} 没有条目可跳过");
    }
}

#[test]
fn decode_mcp_status_skips_bad_entries_and_counts_them() {
    let payload = json!({
        "servers": [
            valid_entry(),
            null,
            "not-an-object",
            42,
            {"name": "", "transport": "stdio", "state": "ready"},
            {"name": 42, "transport": "stdio", "state": "ready"},
            {"name": "no-transport", "state": "ready"},
            {"name": "bad-transport", "transport": "carrier-pigeon", "state": "ready"},
            {"name": "bad-state", "transport": "stdio", "state": "exploded"},
            json!({"name": "missing-state", "transport": "http"})
        ]
    });
    let (decoded, skipped) = decode_mcp_status_counted(&payload);
    assert_eq!(decoded.len(), 1, "只有第一条合法条目被保留");
    assert_eq!(decoded[0].name, "weather");
    assert_eq!(decoded[0].state, "ready");
    assert_eq!(skipped, 9, "坏形状条目必须被跳过并计数");
    // 无计数版本返回同样的列表。
    assert_eq!(decode_mcp_status(&payload), decoded);
}

#[test]
fn decode_mcp_status_truncates_strings_and_caps_array() {
    let long = "x".repeat(1000);
    let many: Vec<Value> = (0..100)
        .map(|i| {
            let mut entry = valid_entry();
            entry["name"] = json!(format!("server-{i}"));
            entry
        })
        .collect();
    let payload = json!({
        "servers": many,
        "protocolVersions": ["2025-06-18"]
    });
    let decoded = decode_mcp_status(&payload);
    assert_eq!(decoded.len(), MAX_SERVERS, "数组必须按 64 截顶");
    assert_eq!(MAX_SERVERS, 64, "spec §3.2 冻结上限");

    let entry = json!({
        "name": "weather",
        "transport": "stdio",
        "state": "degraded",
        "protocolVersion": long,
        "serverInfo": { "name": long, "version": long },
        "toolCount": 7,
        "lastError": long
    });
    let decoded = decode_server_entry(&entry).expect("合法条目");
    let last_error = decoded.last_error.as_deref().expect("lastError 应保留");
    assert_eq!(last_error.chars().count(), MAX_LAST_ERROR_LEN);
    assert_eq!(MAX_LAST_ERROR_LEN, 256, "spec §3.2 冻结 lastError 上限");
    let server_name = decoded.server_name.as_deref().expect("serverInfo.name");
    assert_eq!(server_name.chars().count(), MAX_SERVER_INFO_LEN);
    let server_version = decoded
        .server_version
        .as_deref()
        .expect("serverInfo.version");
    assert_eq!(server_version.chars().count(), MAX_SERVER_INFO_LEN);
    assert_eq!(MAX_SERVER_INFO_LEN, 120, "spec §3.2 冻结 serverInfo 上限");
    let protocol_version = decoded
        .protocol_version
        .as_deref()
        .expect("protocolVersion 应保留");
    assert_eq!(
        protocol_version.chars().count(),
        MAX_SERVER_INFO_LEN,
        "协议版本同样按防御性 120 截断"
    );
}

#[test]
fn decode_mcp_status_maps_fields_and_uses_safe_defaults() {
    let entry = json!({
        "name": "remote",
        "transport": "http",
        "state": "ready",
        "serverInfo": "not-an-object",
        "toolCount": -3,
        "lastError": 42
    });
    let decoded = decode_server_entry(&entry).expect("必填字段齐全即保留");
    assert_eq!(decoded.name, "remote");
    assert_eq!(decoded.transport, "http");
    assert_eq!(decoded.state, "ready");
    assert!(decoded.protocol_version.is_none(), "缺失 → None");
    assert!(decoded.server_name.is_none(), "serverInfo 非对象 → None");
    assert!(decoded.server_version.is_none());
    assert_eq!(decoded.tool_count, 0, "toolCount 非法 → 安全默认 0");
    assert!(decoded.last_error.is_none(), "lastError 非字符串 → None");

    let missing_tool_count = json!({
        "name": "weather",
        "transport": "stdio",
        "state": "connecting"
    });
    let decoded = decode_server_entry(&missing_tool_count).expect("合法");
    assert_eq!(decoded.tool_count, 0, "toolCount 缺失 → 0");
}

// ────────────────────────────────────────────────────────────────────────────
// §3.5 tools.list / §3.6 tools.call 应答 decoder
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn decode_tools_list_is_fail_closed_with_frozen_caps() {
    assert!(decode_tools_list(&json!(null)).is_empty(), "非数组 → 空");
    assert!(
        decode_tools_list(&json!({"tools": []})).is_empty(),
        "对象 → 空"
    );
    assert_eq!(MAX_TOOLS, 512, "spec §3.5 冻结工具数上限");
    assert_eq!(MAX_DESCRIPTION_LEN, 200, "spec §3.5 冻结 description 上限");
    assert_eq!(MAX_SCHEMA_KEYS, 32, "spec §3.5 冻结 schema 键上限");

    let long = "y".repeat(1000);
    let payload = json!([
        {"name": "get_forecast", "description": long, "inputSchemaKeys": ["location"]},
        {"name": "", "description": "空名条目必须跳过"},
        {"name": 42},
        null,
        "junk",
        {"name": "no_description", "inputSchemaKeys": "not-an-array"}
    ]);
    let decoded = decode_tools_list(&payload);
    assert_eq!(decoded.len(), 2, "只有 name 非空字符串的条目保留");
    assert_eq!(decoded[0].name, "get_forecast");
    assert_eq!(decoded[0].description.chars().count(), MAX_DESCRIPTION_LEN);
    assert_eq!(decoded[0].input_schema_keys, vec!["location"]);
    assert_eq!(decoded[1].name, "no_description");
    assert_eq!(decoded[1].description, "", "description 缺失 → 空串");
    assert!(decoded[1].input_schema_keys.is_empty());

    let too_many_keys = json!([{
        "name": "get_x",
        "inputSchemaKeys": (0..100).map(|i| format!("key{i}")).collect::<Vec<_>>()
    }]);
    let decoded = decode_tools_list(&too_many_keys);
    assert_eq!(decoded[0].input_schema_keys.len(), MAX_SCHEMA_KEYS);
}

#[test]
fn decode_tool_call_result_maps_fields_fail_closed() {
    assert!(decode_tool_call_result(&json!(null)).is_none());
    assert!(decode_tool_call_result(&json!("x")).is_none());
    assert!(decode_tool_call_result(&json!([])).is_none());

    let full = decode_tool_call_result(&json!({
        "isError": false,
        "contentCount": 1,
        "contentTypes": ["text"],
        "textPreview": "上海晴，26°C",
        "truncated": true
    }))
    .expect("完整对象应解码");
    assert!(
        full.allowed,
        "decoder 只产出「已执行」的结果，allowed 恒 true"
    );
    assert_eq!(full.reason, None);
    assert_eq!(full.is_error, Some(false));
    assert_eq!(full.content_count, Some(1));
    assert_eq!(full.content_types, Some(vec!["text".to_string()]));
    assert_eq!(full.text_preview.as_deref(), Some("上海晴，26°C"));
    assert_eq!(full.truncated, Some(true));

    let garbage = decode_tool_call_result(&json!({
        "isError": 42,
        "contentCount": "many",
        "contentTypes": [1, 2],
        "textPreview": 7,
        "truncated": "yes"
    }))
    .expect("对象形状仍解码");
    assert!(garbage.is_error.is_none(), "坏类型 → None（不猜）");
    assert!(garbage.content_count.is_none());
    assert!(garbage.content_types.is_none(), "坏元素类型 → 整列 None");
    assert!(garbage.text_preview.is_none());
    assert!(garbage.truncated.is_none());

    let preview = "z".repeat(MAX_TEXT_PREVIEW_LEN + 500);
    let clamped = decode_tool_call_result(&json!({
        "isError": true,
        "contentCount": 2,
        "contentTypes": ["text", "image"],
        "textPreview": preview,
        "truncated": false
    }))
    .expect("解码");
    assert_eq!(MAX_TEXT_PREVIEW_LEN, 2048, "spec §3.6 冻结 preview 上限");
    assert_eq!(
        clamped
            .text_preview
            .as_deref()
            .expect("preview 应保留")
            .chars()
            .count(),
        MAX_TEXT_PREVIEW_LEN
    );
}

// ────────────────────────────────────────────────────────────────────────────
// §3.3 / §3.6 RPC 参数构造（env 值红线）
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn connect_params_freeze_the_shape_and_never_carry_env() {
    let row = McpServerRow {
        id: "id-1".to_string(),
        name: "weather".to_string(),
        transport: McpTransport::Stdio,
        command: Some("uvx".to_string()),
        args: vec!["mcp-server-weather".to_string()],
        url: None,
        env_keys: vec!["WEATHER_API_KEY".to_string()],
        enabled: true,
    };
    let params = connect_params(&row);
    assert_eq!(
        params,
        json!({
            "name": "weather",
            "transport": "stdio",
            "command": "uvx",
            "args": ["mcp-server-weather"],
            "url": null,
            "protocolVersion": "2025-06-18"
        })
    );
    let object = params.as_object().expect("params 必须是对象");
    assert_eq!(object.len(), 6, "connect 参数恰好 6 键");
    assert!(!object.contains_key("env"), "env 值绝不过 IPC（spec §2.1）");
    assert_eq!(
        object.get("protocolVersion").and_then(Value::as_str),
        Some(MCP_PROTOCOL_VERSION),
        "协商版本冻结为首选版本"
    );

    let http_row = McpServerRow {
        transport: McpTransport::Http,
        command: None,
        url: Some("https://mcp.example.com".to_string()),
        args: vec![],
        ..row
    };
    let params = connect_params(&http_row);
    assert_eq!(params["transport"], "http");
    assert_eq!(params["url"], "https://mcp.example.com");
    assert_eq!(params["command"], Value::Null);
}

#[test]
fn tool_call_params_freeze_readonly_policy_and_safe_arguments() {
    let params = tool_call_params("weather", "get_forecast", &json!({"location": "Shanghai"}));
    assert_eq!(
        params,
        json!({
            "name": "weather",
            "tool": "get_forecast",
            "arguments": {"location": "Shanghai"},
            "policyDecision": "readonly"
        })
    );
    let object = params.as_object().expect("params 必须是对象");
    assert_eq!(object.len(), 4, "tools.call 参数恰好 4 键");
    assert_eq!(
        object.get("policyDecision").and_then(Value::as_str),
        Some("readonly"),
        "Rust 放行的唯一 policyDecision 值（spec §2.3 双保险）"
    );

    // 非 object 的 arguments（数组 / 标量 / 缺失）一律置 {}。
    for bad in [json!([1, 2]), json!("x"), json!(42), json!(null)] {
        let params = tool_call_params("weather", "get_forecast", &bad);
        assert_eq!(
            params["arguments"],
            json!({}),
            "非 object 必须置空（{:?}）",
            bad
        );
    }
}

// ────────────────────────────────────────────────────────────────────────────
// registry 自包含查询 + registry ⊕ sidecar 合并（纯函数）
// ────────────────────────────────────────────────────────────────────────────

fn stdio_input(name: &str) -> McpServerInput {
    McpServerInput {
        id: None,
        name: name.to_string(),
        transport: McpTransport::Stdio,
        command: Some("uvx".to_string()),
        args: vec!["mcp-server-weather".to_string()],
        url: None,
        env_keys: vec!["WEATHER_API_KEY".to_string()],
        enabled: true,
    }
}

/// 内存库 + 生产路径登记（不手写 INSERT——冷启动纪律）。
fn seeded_db() -> Database {
    let db = Database::open_in_memory().expect("打开内存库");
    upsert_server(db.connection(), &stdio_input("weather")).expect("登记 weather");
    upsert_server(
        db.connection(),
        &McpServerInput {
            name: "remote".to_string(),
            transport: McpTransport::Http,
            command: None,
            args: vec![],
            url: Some("https://mcp.example.com".to_string()),
            env_keys: vec![],
            ..stdio_input("remote")
        },
    )
    .expect("登记 remote");
    db
}

fn manager_without_python() -> std::sync::Arc<SidecarManager> {
    SidecarManager::new(SidecarConfig {
        python: std::path::PathBuf::from("no-such-python-interpreter-9f3b.exe"),
        runtime_root: std::path::PathBuf::from("python"),
        host_version: "0.5.0".to_string(),
    })
}

#[test]
fn find_server_row_reads_the_registry_row_by_id() {
    let db = seeded_db();
    let conn = db.connection();
    let listed = harness_hub_lib::mcp::registry::list_servers(conn).expect("列出");
    let weather = listed
        .iter()
        .find(|s| s.name == "weather")
        .expect("weather 行");

    let row = find_server_row(conn, &weather.id).expect("按 id 读行");
    assert_eq!(row.id, weather.id);
    assert_eq!(row.name, "weather");
    assert_eq!(row.transport, McpTransport::Stdio);
    assert_eq!(row.command.as_deref(), Some("uvx"));
    assert_eq!(row.args, vec!["mcp-server-weather"]);
    assert_eq!(row.url, None);
    assert_eq!(row.env_keys, vec!["WEATHER_API_KEY"]);
    assert!(row.enabled);

    assert!(
        find_server_row(conn, "no-such-id").is_none(),
        "未知 id → None"
    );
}

#[test]
fn merge_server_states_keeps_registry_as_source_of_truth() {
    let rows = vec![
        McpServerRow {
            id: "id-a".to_string(),
            name: "weather".to_string(),
            transport: McpTransport::Stdio,
            command: Some("uvx".to_string()),
            args: vec![],
            url: None,
            env_keys: vec!["WEATHER_API_KEY".to_string()],
            enabled: true,
        },
        McpServerRow {
            id: "id-b".to_string(),
            name: "remote".to_string(),
            transport: McpTransport::Http,
            command: None,
            args: vec![],
            url: Some("https://mcp.example.com".to_string()),
            env_keys: vec![],
            enabled: false,
        },
    ];
    let decoded = vec![
        harness_hub_lib::mcp::client_bridge::McpServerRuntime {
            name: "weather".to_string(),
            transport: "stdio".to_string(),
            state: "ready".to_string(),
            protocol_version: Some("2025-06-18".to_string()),
            server_name: Some("weather-server".to_string()),
            server_version: Some("1.2.0".to_string()),
            tool_count: 3,
            last_error: None,
        },
        // sidecar 里有、registry 里没有的条目：registry 是 source of truth，丢弃。
        harness_hub_lib::mcp::client_bridge::McpServerRuntime {
            name: "ghost".to_string(),
            transport: "stdio".to_string(),
            state: "ready".to_string(),
            protocol_version: None,
            server_name: None,
            server_version: None,
            tool_count: 9,
            last_error: None,
        },
    ];

    let merged = merge_server_states(rows, &decoded);
    assert_eq!(merged.len(), 2, "ghost 不在 registry，不进快照");
    assert_eq!(merged[0].name, "weather");
    assert_eq!(merged[0].state, "ready");
    assert_eq!(merged[0].protocol_version.as_deref(), Some("2025-06-18"));
    assert_eq!(merged[0].server_name.as_deref(), Some("weather-server"));
    assert_eq!(merged[0].server_version.as_deref(), Some("1.2.0"));
    assert_eq!(merged[0].tool_count, 3);
    assert_eq!(merged[0].env_keys, vec!["WEATHER_API_KEY"]);
    assert_eq!(
        merged[0].env_missing_keys,
        vec!["WEATHER_API_KEY"],
        "v1 envMissingKeys 与 envKeys 相同（spec §2.1：不解引用凭据）"
    );

    // sidecar 里没有的 server：disconnected 起点 + registry 的 env 名单。
    assert_eq!(merged[1].name, "remote");
    assert_eq!(merged[1].state, "disconnected");
    assert_eq!(merged[1].transport, "http");
    assert!(merged[1].protocol_version.is_none());
    assert_eq!(merged[1].tool_count, 0);
    assert!(merged[1].env_keys.is_empty());
    assert!(merged[1].env_missing_keys.is_empty());

    // sidecar 一片空白：全部 disconnected。
    let rows = vec![McpServerRow {
        id: "id-a".to_string(),
        name: "weather".to_string(),
        transport: McpTransport::Stdio,
        command: Some("uvx".to_string()),
        args: vec![],
        url: None,
        env_keys: vec![],
        enabled: true,
    }];
    let merged = merge_server_states(rows, &[]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].state, "disconnected");
}

// ────────────────────────────────────────────────────────────────────────────
// 编排函数：sidecar 不可用 / deny 的 fail-closed 收敛（无真 Python）
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn runtime_status_converges_to_unavailable_without_sidecar() {
    let db = seeded_db();
    let manager = manager_without_python();
    let status = runtime_status(&manager, db.connection());
    assert!(!status.available, "sidecar 未就绪 → available:false");
    let reason = status
        .unavailable_reason
        .as_deref()
        .expect("必须带原因（spec §3.8：HTTP 200 语义）");
    assert!(reason.contains("未就绪"), "{reason}");
    assert!(status.servers.is_empty(), "不可用时不得编造 server 状态");
}

#[test]
fn connect_converges_failures_without_spawning_anything() {
    let db = seeded_db();
    let conn = db.connection();
    let listed = harness_hub_lib::mcp::registry::list_servers(conn).expect("列出");
    let weather_id = listed
        .iter()
        .find(|s| s.name == "weather")
        .expect("行")
        .clone();
    let manager = manager_without_python();

    let unknown = harness_hub_lib::mcp::client_bridge::connect(&manager, conn, "no-such-id");
    assert!(!unknown.ok, "未知 server 必须失败");
    assert!(
        unknown
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("不存在"),
        "{unknown:?}"
    );
    assert!(unknown.server.is_none());

    let not_ready = harness_hub_lib::mcp::client_bridge::connect(&manager, conn, &weather_id.id);
    assert!(!not_ready.ok, "sidecar 未就绪必须收敛为 ok:false");
    assert!(
        not_ready
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("未就绪"),
        "{not_ready:?}"
    );
    assert!(not_ready.server.is_none());
}

#[test]
fn disconnect_is_idempotent_even_without_sidecar() {
    let db = seeded_db();
    let conn = db.connection();
    let listed = harness_hub_lib::mcp::registry::list_servers(conn).expect("列出");
    let weather_id = listed
        .iter()
        .find(|s| s.name == "weather")
        .expect("行")
        .id
        .clone();
    let manager = manager_without_python();

    // §3.4：未知名幂等成功（目标状态「已断开」已成立）。
    let unknown = harness_hub_lib::mcp::client_bridge::disconnect(&manager, conn, "no-such-id");
    assert!(unknown.ok, "{unknown:?}");
    assert!(unknown.error.is_none());
    assert!(unknown.server.is_none());

    let known = harness_hub_lib::mcp::client_bridge::disconnect(&manager, conn, &weather_id);
    assert!(known.ok, "sidecar 未就绪 = 无连接可断，幂等成功：{known:?}");
    assert!(known.error.is_none());
    assert!(known.server.is_none());
}

#[test]
fn tools_list_converges_to_empty_without_sidecar() {
    let db = seeded_db();
    let conn = db.connection();
    let manager = manager_without_python();
    assert!(
        tools_list(&manager, conn, "no-such-id").is_empty(),
        "未知 server → 空列表"
    );
    let listed = harness_hub_lib::mcp::registry::list_servers(conn).expect("列出");
    let weather_id = listed
        .iter()
        .find(|s| s.name == "weather")
        .expect("行")
        .id
        .clone();
    assert!(
        tools_list(&manager, conn, &weather_id).is_empty(),
        "sidecar 未就绪 → 空列表（fail-closed，服务器态在 status 里可见）"
    );
}

#[test]
fn tool_call_deny_never_reaches_the_sidecar() {
    let db = seeded_db();
    let conn = db.connection();
    let listed = harness_hub_lib::mcp::registry::list_servers(conn).expect("列出");
    let weather_id = listed
        .iter()
        .find(|s| s.name == "weather")
        .expect("行")
        .id
        .clone();
    let manager = manager_without_python();

    // deny：策略拦截发生在 sidecar 检查之前——未就绪的 sidecar 也证明没被碰。
    let denied = tool_call(&manager, conn, &weather_id, "write_file", &json!({}));
    assert!(!denied.allowed);
    assert_eq!(
        denied.reason.as_deref(),
        Some("not_readonly_by_default"),
        "deny 时 reason 是策略冻结串"
    );
    assert!(denied.is_error.is_none(), "deny 时只有 allowed+reason");
    assert!(denied.content_count.is_none());
    assert!(denied.content_types.is_none());
    assert!(denied.text_preview.is_none());
    assert!(denied.truncated.is_none());

    let invalid = tool_call(&manager, conn, &weather_id, "get forecast", &json!({}));
    assert!(!invalid.allowed);
    assert_eq!(invalid.reason.as_deref(), Some("invalid_tool_name"));
    assert!(invalid.is_error.is_none());
}

#[test]
fn tool_call_allowed_but_unexecuted_carries_reason_only() {
    let db = seeded_db();
    let conn = db.connection();
    let listed = harness_hub_lib::mcp::registry::list_servers(conn).expect("列出");
    let weather_id = listed
        .iter()
        .find(|s| s.name == "weather")
        .expect("行")
        .id
        .clone();
    let manager = manager_without_python();

    let outcome = tool_call(&manager, conn, &weather_id, "get_forecast", &json!({}));
    assert!(outcome.allowed, "策略放行（allowed 只表达策略结论）");
    let reason = outcome.reason.as_deref().expect("执行失败必须带原因");
    assert!(reason.contains("未就绪"), "{reason}");
    assert!(outcome.is_error.is_none(), "未执行 → 结果字段全 None");
    assert!(outcome.content_count.is_none());
    assert!(outcome.content_types.is_none());
    assert!(outcome.text_preview.is_none());
    assert!(outcome.truncated.is_none());

    let unknown = tool_call(&manager, conn, "no-such-id", "get_forecast", &json!({}));
    assert!(!unknown.allowed, "server 不存在 → 不可执行");
    assert!(
        unknown
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("不存在"),
        "{unknown:?}"
    );
}

// ────────────────────────────────────────────────────────────────────────────
// §3.9 DTO serde 往返：Rust → JSON 文本 → JSON 值逐键锁定
// ────────────────────────────────────────────────────────────────────────────

fn round_trip<T: serde::Serialize>(value: &T) -> Value {
    let text = serde_json::to_string(value).expect("序列化");
    serde_json::from_str(&text).expect("JSON 文本必须能解析回来")
}

#[test]
fn runtime_status_dto_round_trips_with_locked_camel_case_keys() {
    let status = McpRuntimeStatus {
        available: true,
        unavailable_reason: None,
        servers: vec![harness_hub_lib::mcp::client_bridge::McpServerRuntimeState {
            name: "weather".to_string(),
            transport: "stdio".to_string(),
            state: "ready".to_string(),
            protocol_version: Some("2025-06-18".to_string()),
            server_name: Some("weather-server".to_string()),
            server_version: Some("1.2.0".to_string()),
            tool_count: 3,
            last_error: None,
            env_keys: vec!["WEATHER_API_KEY".to_string()],
            env_missing_keys: vec!["WEATHER_API_KEY".to_string()],
        }],
    };
    let value = round_trip(&status);
    assert_eq!(
        value,
        json!({
            "available": true,
            "unavailableReason": null,
            "servers": [{
                "name": "weather",
                "transport": "stdio",
                "state": "ready",
                "protocolVersion": "2025-06-18",
                "serverName": "weather-server",
                "serverVersion": "1.2.0",
                "toolCount": 3,
                "lastError": null,
                "envKeys": ["WEATHER_API_KEY"],
                "envMissingKeys": ["WEATHER_API_KEY"]
            }]
        })
    );
    let object = value.as_object().expect("McpRuntimeStatus 必须是对象");
    assert_eq!(object.len(), 3, "顶层恰好 3 键");
    for leaked in ["unavailable_reason", "env_keys", "env_missing_keys"] {
        assert!(
            !object.contains_key(leaked),
            "顶层泄漏 snake_case：{leaked}"
        );
    }
    let server = value["servers"][0].as_object().expect("servers[0] 对象");
    assert_eq!(server.len(), 10, "server 运行态恰好 10 键");

    let unavailable = McpRuntimeStatus::unavailable("Python sidecar 未就绪，MCP runtime 不可用");
    let value = round_trip(&unavailable);
    assert_eq!(
        value,
        json!({
            "available": false,
            "unavailableReason": "Python sidecar 未就绪，MCP runtime 不可用",
            "servers": []
        }),
        "unavailable 形状必须完整可解析（前端有东西可渲染）"
    );
}

#[test]
fn tool_call_outcome_dto_round_trips_with_locked_shapes() {
    // deny 形状：恰好 7 键，除 allowed/reason 外全 null。
    let denied = McpToolCallOutcome {
        allowed: false,
        reason: Some("not_readonly_by_default".to_string()),
        is_error: None,
        content_count: None,
        content_types: None,
        text_preview: None,
        truncated: None,
    };
    let value = round_trip(&denied);
    assert_eq!(
        value,
        json!({
            "allowed": false,
            "reason": "not_readonly_by_default",
            "isError": null,
            "contentCount": null,
            "contentTypes": null,
            "textPreview": null,
            "truncated": null
        })
    );
    let object = value.as_object().expect("McpToolCallOutcome 必须是对象");
    assert_eq!(object.len(), 7, "恰好 7 键");
    for leaked in ["is_error", "content_count", "content_types", "text_preview"] {
        assert!(!object.contains_key(leaked), "泄漏 snake_case：{leaked}");
    }

    // 已执行形状：全部 Some。
    let executed = McpToolCallOutcome {
        allowed: true,
        reason: None,
        is_error: Some(false),
        content_count: Some(1),
        content_types: Some(vec!["text".to_string()]),
        text_preview: Some("上海晴".to_string()),
        truncated: Some(true),
    };
    assert_eq!(
        round_trip(&executed),
        json!({
            "allowed": true,
            "reason": null,
            "isError": false,
            "contentCount": 1,
            "contentTypes": ["text"],
            "textPreview": "上海晴",
            "truncated": true
        })
    );
}

#[test]
fn connect_outcome_and_tool_descriptor_dtos_round_trip() {
    let failure = McpConnectOutcome {
        ok: false,
        error: Some("Python sidecar 未就绪，MCP runtime 不可用".to_string()),
        server: None,
    };
    let value = round_trip(&failure);
    assert_eq!(
        value,
        json!({
            "ok": false,
            "error": "Python sidecar 未就绪，MCP runtime 不可用",
            "server": null
        })
    );
    let object = value.as_object().expect("McpConnectOutcome 必须是对象");
    assert_eq!(object.len(), 3, "恰好 3 键");

    let descriptor = McpToolDescriptorDto {
        name: "get_forecast".to_string(),
        description: "查询天气预报".to_string(),
        input_schema_keys: vec!["location".to_string()],
    };
    assert_eq!(
        round_trip(&descriptor),
        json!({
            "name": "get_forecast",
            "description": "查询天气预报",
            "inputSchemaKeys": ["location"]
        }),
        "工具描述符恰好 3 键（inputSchemaKeys 是 camelCase 键名本身）"
    );
}
