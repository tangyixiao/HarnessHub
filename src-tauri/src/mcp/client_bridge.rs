//! F22 MCP Client bridge（spec 2026-11-09-f22-mcp-client-server-design.md §3.8）。
//!
//! 职责边界（§1）：
//! - **Python 拥有连接与传输**：本模块只把 registry 定义翻译成 `mcp.connect`
//!   参数、把 sidecar 应答**安全地**收敛成 DTO（fail-closed decoder，风格对齐
//!   `sidecar::gateway`）；
//! - **Rust 拥有配置与策略**：tool call 先过 [`super::policy`]，deny 不出 Rust、
//!   绝不转发 Python（§2.3 双保险的第一道）；
//! - 领域模块纪律：不依赖任何 Tauri GUI 类型，入参是
//!   [`SidecarManager`](crate::sidecar::manager::SidecarManager) + `&Connection`。
//!
//! 红线（§2.1）：**任何 env 值不过 IPC**——`envKeys` / `envMissingKeys` 只有变量名
//! （来自 registry 行），且 v1 两者相同（不解引用凭据）；env 值由 sidecar 子进程
//! 继承宿主环境获得，Rust 不经手。
//!
//! **契约纪律（AGENTS.md）**：跨 IPC DTO 带 `#[serde(rename_all = "camelCase")]`，
//! 实际键名与嵌套形状由 serialization contract test 逐键锁定；decoder 从 `&Value`
//! 手工取值，不依赖 `Deserialize`。

use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use super::policy;
use super::McpTransport;
use crate::sidecar::dto::SidecarState;
use crate::sidecar::manager::SidecarManager;

// ────────────────────────────────────────────────────────────────────────────
// 冻结上限（spec §3.2 / §3.5 / §3.6）
// ────────────────────────────────────────────────────────────────────────────

/// `mcp.status` servers 数组上限（decoder 侧防御；风格对齐 gateway 的 MAX_ITEMS）。
pub const MAX_SERVERS: usize = 64;
/// `lastError` 截断长度（spec §3.2）。
pub const MAX_LAST_ERROR_LEN: usize = 256;
/// `serverInfo.name/version` 截断长度（spec §3.2；name/protocolVersion 同宽防御）。
pub const MAX_SERVER_INFO_LEN: usize = 120;
/// tools/list 数组上限（spec §3.5：分页循尽但最多 512 个工具）。
pub const MAX_TOOLS: usize = 512;
/// 工具 `description` 截断长度（spec §3.5）。
pub const MAX_DESCRIPTION_LEN: usize = 200;
/// `inputSchemaKeys` 顶层键个数上限（spec §3.5）。
pub const MAX_SCHEMA_KEYS: usize = 32;
/// `textPreview` 截断长度（spec §3.6）。
pub const MAX_TEXT_PREVIEW_LEN: usize = 2048;
/// connect 协商的首选 MCP 规范版本（spec §3.3，v1 冻结）。
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

const SIDECAR_NOT_READY: &str = "Python sidecar 未就绪，MCP runtime 不可用";

// ────────────────────────────────────────────────────────────────────────────
// 跨 IPC DTO（Serialize only；形状由契约测试锁定）
// ────────────────────────────────────────────────────────────────────────────

/// 跨 IPC 的单 server 运行态（§3.8）：registry 行（env 名单）⊕ sidecar 运行态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerRuntimeState {
    pub name: String,
    pub transport: String,
    /// `disconnected | connecting | ready | degraded`（§3.2 状态机）。
    pub state: String,
    pub protocol_version: Option<String>,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub tool_count: u32,
    pub last_error: Option<String>,
    /// registry 行 `env_keys`：只有变量名，值绝不进 IPC（§2.1）。
    pub env_keys: Vec<String>,
    /// v1 与 `envKeys` 相同（§3.8：不解引用凭据；信息性提示宿主环境需备齐）。
    pub env_missing_keys: Vec<String>,
}

/// MCP runtime 总状态（`mcp_runtime_status` 返回）。失败收敛为
/// `available: false` + 原因 + 空列表，**永不返回 IPC Err**（§3.8）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRuntimeStatus {
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub servers: Vec<McpServerRuntimeState>,
}

impl McpRuntimeStatus {
    /// sidecar 未就绪 / RPC 失败时的完整降级对象（对齐 `GatewayStatus::unavailable`）。
    pub fn unavailable(reason: &str) -> Self {
        Self {
            available: false,
            unavailable_reason: Some(reason.to_string()),
            servers: Vec::new(),
        }
    }
}

/// 工具描述符（§3.5 归一形状；description ≤200、schema 键 ≤32 在 decoder 截断）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDescriptorDto {
    pub name: String,
    pub description: String,
    pub input_schema_keys: Vec<String>,
}

/// `mcp.tools.call` 的结构化结果（§3.6 + §3.8 deny 收敛）。语义冻结：
/// - `allowed` **只表达策略结论**；deny 时**只有** `allowed=false` + `reason`
///   （策略冻结串），其余字段全 `None`，且绝不转发 Python（§2.3）；
/// - `allowed=true` 且 `reason` 非空 = 策略放行但未能执行（sidecar 未就绪 /
///   RPC 失败 / 应答形状非法），结果字段保持 `None`；
/// - `isError` / `contentCount` / `contentTypes` / `textPreview` / `truncated`
///   只有真正执行了 `mcp.tools.call` 才 `Some`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallOutcome {
    pub allowed: bool,
    pub reason: Option<String>,
    pub is_error: Option<bool>,
    pub content_count: Option<u32>,
    pub content_types: Option<Vec<String>>,
    pub text_preview: Option<String>,
    pub truncated: Option<bool>,
}

impl McpToolCallOutcome {
    /// 策略 deny（reason = 冻结串）或前置不可执行时的未执行结果。
    pub fn not_executed(allowed: bool, reason: &str) -> Self {
        Self {
            allowed,
            reason: Some(reason.to_string()),
            is_error: None,
            content_count: None,
            content_types: None,
            text_preview: None,
            truncated: None,
        }
    }
}

/// `mcp.connect` / `mcp.disconnect` 的结构化结果（§3.3 / §3.4）。
///
/// disconnect 复用本类型：成功（含未知 server / sidecar 未就绪的幂等成功，
/// §3.4）= `ok: true` + `server: None`；失败 = `ok: false` + `error`。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConnectOutcome {
    pub ok: bool,
    pub error: Option<String>,
    pub server: Option<McpServerRuntimeState>,
}

impl McpConnectOutcome {
    /// 失败收敛（HTTP 200 语义，前端渲染 error 文案）。
    pub fn failure(error: &str) -> Self {
        Self {
            ok: false,
            error: Some(error.to_string()),
            server: None,
        }
    }

    /// 断开成功的幂等形状（无 server 快照可带）。
    pub fn disconnected() -> Self {
        Self {
            ok: true,
            error: None,
            server: None,
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// decoder 的内部中间结构（Python 应答视图；env 名单来自 registry，不在这里）
// ────────────────────────────────────────────────────────────────────────────

/// `mcp.status` / `mcp.connect` 应答里单 server 的解码视图（内部结构，无 serde）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerRuntime {
    pub name: String,
    pub transport: String,
    pub state: String,
    pub protocol_version: Option<String>,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub tool_count: u32,
    pub last_error: Option<String>,
}

// ────────────────────────────────────────────────────────────────────────────
// registry 自包含查询（不改 registry.rs / sync.rs：bridge 自己的 SELECT）
// ────────────────────────────────────────────────────────────────────────────

/// `mcp_servers` 行的自包含投影（bridge 编排与 RPC 参数构造的最小需要）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerRow {
    pub id: String,
    pub name: String,
    pub transport: McpTransport,
    /// stdio：可执行文件。
    pub command: Option<String>,
    /// stdio：参数（DB 侧 JSON 数组；坏 JSON 安全默认空数组）。
    pub args: Vec<String>,
    /// http：endpoint。
    pub url: Option<String>,
    /// 环境变量**名**（值绝不入内，§2.1）。
    pub env_keys: Vec<String>,
    pub enabled: bool,
}

fn transport_label(transport: McpTransport) -> &'static str {
    match transport {
        McpTransport::Stdio => "stdio",
        McpTransport::Http => "http",
    }
}

/// 解析一行查询结果；transport 落在 CHECK 约束之外（脏数据）时 fail-closed 返回 None。
fn map_server_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Option<McpServerRow>> {
    let transport: String = row.get(2)?;
    let transport = match transport.as_str() {
        "stdio" => McpTransport::Stdio,
        "http" => McpTransport::Http,
        _ => return Ok(None),
    };
    Ok(Some(McpServerRow {
        id: row.get(0)?,
        name: row.get(1)?,
        transport,
        command: row.get(3)?,
        args: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
        url: row.get(5)?,
        env_keys: serde_json::from_str(&row.get::<_, String>(6)?).unwrap_or_default(),
        enabled: row.get::<_, i64>(7)? != 0,
    }))
}

/// 按 id 读一行 server 定义（**纯读**）。未知 id / 脏行 → `None`。
pub fn find_server_row(conn: &Connection, server_id: &str) -> Option<McpServerRow> {
    conn.query_row(
        "SELECT id, name, transport, command, args_json, url, env_keys, enabled
         FROM mcp_servers WHERE id = ?1",
        rusqlite::params![server_id],
        map_server_row,
    )
    .ok()
    .flatten()
}

/// 全部 server 行（按 name 排序；**纯读**）。status 快照以 registry 为列表基准。
fn list_server_rows(conn: &Connection) -> Vec<McpServerRow> {
    let Ok(mut statement) = conn.prepare(
        "SELECT id, name, transport, command, args_json, url, env_keys, enabled
         FROM mcp_servers ORDER BY name ASC",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = statement.query_map([], map_server_row) else {
        return Vec::new();
    };
    rows.flatten().flatten().collect()
}

// ────────────────────────────────────────────────────────────────────────────
// RPC 参数构造（spec §3.3 / §3.6；形状由契约测试锁定）
// ────────────────────────────────────────────────────────────────────────────

/// `mcp.connect` 参数（§3.3）。**env 不传**（§2.1：sidecar 子进程继承宿主环境）；
/// command / url 按登记原样携带（另一侧为 null，由 Python 按传输类型校验）。
pub fn connect_params(row: &McpServerRow) -> Value {
    serde_json::json!({
        "name": row.name,
        "transport": transport_label(row.transport),
        "command": row.command,
        "args": row.args,
        "url": row.url,
        "protocolVersion": MCP_PROTOCOL_VERSION,
    })
}

/// `mcp.tools.call` 参数（§3.6）：`policyDecision` 只能是 `"readonly"`（Rust 策略
/// 放行的唯一值，Python 侧二次校验）；`arguments` 非 object 一律置 `{}`。
pub fn tool_call_params(server_name: &str, tool: &str, arguments: &Value) -> Value {
    let arguments = if arguments.is_object() {
        arguments.clone()
    } else {
        Value::Object(serde_json::Map::new())
    };
    serde_json::json!({
        "name": server_name,
        "tool": tool,
        "arguments": arguments,
        "policyDecision": "readonly",
    })
}

// ────────────────────────────────────────────────────────────────────────────
// fail-closed decoder（风格对齐 sidecar::gateway：坏形状跳过 / 安全默认 / 截断）
// ────────────────────────────────────────────────────────────────────────────

fn clamp(raw: &str, max: usize) -> String {
    raw.chars().take(max).collect()
}

fn opt_string(raw: Option<&Value>, max: usize) -> Option<String> {
    raw.and_then(Value::as_str).map(|value| clamp(value, max))
}

/// 字符串数组逐元素取 `as_str`；任一元素类型不对 → 整列 `None`（不猜）。
fn strict_string_list(raw: &Value) -> Option<Vec<String>> {
    let items = raw.as_array()?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(item.as_str()?.to_string());
    }
    Some(out)
}

/// 单条 server 运行态解码：必填 name / transport / state 任一缺失或取值非法 →
/// `None`（整条跳过，fail-closed）。
pub fn decode_server_entry(value: &Value) -> Option<McpServerRuntime> {
    let object = value.as_object()?;
    let name = clamp(
        object
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())?,
        MAX_SERVER_INFO_LEN,
    );
    let transport = match object.get("transport").and_then(Value::as_str) {
        Some("stdio") => "stdio",
        Some("http") => "http",
        _ => return None,
    };
    let state = match object.get("state").and_then(Value::as_str) {
        Some("disconnected") => "disconnected",
        Some("connecting") => "connecting",
        Some("ready") => "ready",
        Some("degraded") => "degraded",
        _ => return None,
    };
    let server_info = object.get("serverInfo").and_then(Value::as_object);
    Some(McpServerRuntime {
        name,
        transport: transport.to_string(),
        state: state.to_string(),
        protocol_version: opt_string(object.get("protocolVersion"), MAX_SERVER_INFO_LEN),
        server_name: server_info.and_then(|info| opt_string(info.get("name"), MAX_SERVER_INFO_LEN)),
        server_version: server_info
            .and_then(|info| opt_string(info.get("version"), MAX_SERVER_INFO_LEN)),
        tool_count: object
            .get("toolCount")
            .and_then(Value::as_u64)
            .map(|count| count.min(u32::MAX as u64) as u32)
            .unwrap_or(0),
        last_error: opt_string(object.get("lastError"), MAX_LAST_ERROR_LEN),
    })
}

/// `mcp.status` 应答解码（fail-closed）：payload 非对象 / `servers` 非数组 → 空；
/// 数组先按 [`MAX_SERVERS`] 截顶再逐条过滤，坏形状条目跳过。
pub fn decode_mcp_status(value: &Value) -> Vec<McpServerRuntime> {
    decode_mcp_status_counted(value).0
}

/// 同 [`decode_mcp_status`]，并返回被跳过的坏形状条目数（观测 / 测试用）。
pub fn decode_mcp_status_counted(value: &Value) -> (Vec<McpServerRuntime>, usize) {
    let Some(items) = value
        .as_object()
        .and_then(|object| object.get("servers"))
        .and_then(Value::as_array)
    else {
        return (Vec::new(), 0);
    };
    let mut decoded = Vec::new();
    let mut skipped = 0usize;
    for item in items.iter().take(MAX_SERVERS) {
        match decode_server_entry(item) {
            Some(entry) => decoded.push(entry),
            None => skipped += 1,
        }
    }
    (decoded, skipped)
}

/// `mcp.tools.list` 应答解码（§3.5 归一数组）：非数组 → 空；512 截顶；坏条目
/// 跳过；description ≤200；`inputSchemaKeys` 非数组 → 空列表、键数 ≤32。
pub fn decode_tools_list(value: &Value) -> Vec<McpToolDescriptorDto> {
    let Some(items) = value.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .take(MAX_TOOLS)
        .filter_map(|item| {
            let object = item.as_object()?;
            let name = object
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())?;
            let description = object
                .get("description")
                .and_then(Value::as_str)
                .map(|raw| clamp(raw, MAX_DESCRIPTION_LEN))
                .unwrap_or_default();
            let input_schema_keys = match object.get("inputSchemaKeys").and_then(Value::as_array) {
                Some(keys) => keys
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .take(MAX_SCHEMA_KEYS)
                    .collect(),
                None => Vec::new(),
            };
            Some(McpToolDescriptorDto {
                name: name.to_string(),
                description,
                input_schema_keys,
            })
        })
        .collect()
}

/// `mcp.tools.call` 应答解码（§3.6 归一形状）→「已执行」的结果。
/// 非 object 应答 → `None`（调用方收敛为未执行 + 原因）；坏字段类型 → `None`。
pub fn decode_tool_call_result(value: &Value) -> Option<McpToolCallOutcome> {
    let object = value.as_object()?;
    Some(McpToolCallOutcome {
        allowed: true,
        reason: None,
        is_error: object.get("isError").and_then(Value::as_bool),
        content_count: object
            .get("contentCount")
            .and_then(Value::as_u64)
            .map(|count| count.min(u32::MAX as u64) as u32),
        content_types: object.get("contentTypes").and_then(strict_string_list),
        text_preview: object
            .get("textPreview")
            .and_then(Value::as_str)
            .map(|raw| clamp(raw, MAX_TEXT_PREVIEW_LEN)),
        truncated: object.get("truncated").and_then(Value::as_bool),
    })
}

// ────────────────────────────────────────────────────────────────────────────
// registry ⊕ sidecar 合并（纯函数）与编排函数
// ────────────────────────────────────────────────────────────────────────────

/// registry 行 ⊕ sidecar 解码结果（纯函数，可测）。registry 是**列表与身份**的
/// source of truth：sidecar 里没有的 server 一律 `disconnected`（§3.2 状态机
/// 起点），sidecar 里多余的条目（registry 已删除）丢弃。transport 取 registry 值。
pub fn merge_server_states(
    rows: Vec<McpServerRow>,
    decoded: &[McpServerRuntime],
) -> Vec<McpServerRuntimeState> {
    rows.into_iter()
        .map(|row| {
            let entry = decoded.iter().find(|entry| entry.name == row.name);
            McpServerRuntimeState {
                name: row.name,
                transport: transport_label(row.transport).to_string(),
                state: entry.map_or("disconnected".to_string(), |entry| entry.state.clone()),
                protocol_version: entry.and_then(|entry| entry.protocol_version.clone()),
                server_name: entry.and_then(|entry| entry.server_name.clone()),
                server_version: entry.and_then(|entry| entry.server_version.clone()),
                tool_count: entry.map_or(0, |entry| entry.tool_count),
                last_error: entry.and_then(|entry| entry.last_error.clone()),
                env_keys: row.env_keys.clone(),
                env_missing_keys: row.env_keys,
            }
        })
        .collect()
}

/// `mcp_runtime_status` 编排（§3.8）：sidecar 未就绪 / RPC 失败 → `unavailable`；
/// 成功 → registry ⊕ sidecar 快照。
pub fn runtime_status(manager: &SidecarManager, conn: &Connection) -> McpRuntimeStatus {
    if manager.status().state != SidecarState::Ready {
        return McpRuntimeStatus::unavailable(SIDECAR_NOT_READY);
    }
    let payload = match manager.call("mcp.status", &serde_json::json!({})) {
        Ok(payload) => payload,
        Err(message) => return McpRuntimeStatus::unavailable(&message),
    };
    let decoded = decode_mcp_status(&payload);
    McpRuntimeStatus {
        available: true,
        unavailable_reason: None,
        servers: merge_server_states(list_server_rows(conn), &decoded),
    }
}

/// `mcp_runtime_connect` 编排（§3.3）：读 registry 定义 → 转发 sidecar → 收敛 DTO。
/// 未知 server / sidecar 未就绪 / RPC 失败 / 应答形状非法全部收敛为 `ok: false`。
pub fn connect(manager: &SidecarManager, conn: &Connection, server_id: &str) -> McpConnectOutcome {
    let Some(row) = find_server_row(conn, server_id) else {
        return McpConnectOutcome::failure(&format!("MCP server 不存在：{server_id}"));
    };
    if manager.status().state != SidecarState::Ready {
        return McpConnectOutcome::failure(SIDECAR_NOT_READY);
    }
    match manager.call("mcp.connect", &connect_params(&row)) {
        Ok(payload) => match decode_server_entry(&payload) {
            Some(entry) => {
                // 应答即该 server 的当前运行态（§3.3：失败已由 Python 收敛为
                // degraded + lastError）；身份与 env 名单仍以 registry 为准。
                let server = merge_server_states(vec![row], std::slice::from_ref(&entry))
                    .into_iter()
                    .next()
                    .expect("单行合并必产出一个状态");
                McpConnectOutcome {
                    ok: true,
                    error: None,
                    server: Some(server),
                }
            }
            None => McpConnectOutcome::failure("mcp.connect 应答形状非法"),
        },
        Err(message) => McpConnectOutcome::failure(&message),
    }
}

/// `mcp_runtime_disconnect` 编排（§3.4，幂等）：未知 server / sidecar 未就绪都算
/// 成功——目标状态「无连接」已经成立；仅 RPC 真实失败才 `ok: false`。
pub fn disconnect(
    manager: &SidecarManager,
    conn: &Connection,
    server_id: &str,
) -> McpConnectOutcome {
    let Some(row) = find_server_row(conn, server_id) else {
        return McpConnectOutcome::disconnected();
    };
    if manager.status().state != SidecarState::Ready {
        return McpConnectOutcome::disconnected();
    }
    match manager.call("mcp.disconnect", &serde_json::json!({ "name": row.name })) {
        Ok(_) => McpConnectOutcome::disconnected(),
        Err(message) => McpConnectOutcome::failure(&message),
    }
}

/// `mcp_runtime_tools_list` 编排（§3.5）：未知 server / sidecar 未就绪 / RPC 失败 /
/// 应答形状非法一律收敛为**空列表**（前端 fail-closed 显示无工具；服务器态在
/// status 快照里可见）。
pub fn tools_list(
    manager: &SidecarManager,
    conn: &Connection,
    server_id: &str,
) -> Vec<McpToolDescriptorDto> {
    let Some(row) = find_server_row(conn, server_id) else {
        return Vec::new();
    };
    if manager.status().state != SidecarState::Ready {
        return Vec::new();
    }
    match manager.call("mcp.tools.list", &serde_json::json!({ "name": row.name })) {
        Ok(payload) => decode_tools_list(&payload),
        Err(_) => Vec::new(),
    }
}

/// `mcp_runtime_tool_call` 编排（§3.6 / §3.8）：**策略唯一判定点在 Rust**（§2.3）。
/// 顺序：registry 行 → 策略 → sidecar 就绪检查 → 转发。
/// deny 时返回 `allowed=false` + 冻结 reason，**不转发 Python**。
pub fn tool_call(
    manager: &SidecarManager,
    conn: &Connection,
    server_id: &str,
    tool: &str,
    arguments: &Value,
) -> McpToolCallOutcome {
    let Some(row) = find_server_row(conn, server_id) else {
        return McpToolCallOutcome::not_executed(false, &format!("MCP server 不存在：{server_id}"));
    };
    let decision = policy::evaluate_tool_call(&row.name, tool);
    if !decision.allowed {
        return McpToolCallOutcome::not_executed(false, decision.reason);
    }
    if manager.status().state != SidecarState::Ready {
        return McpToolCallOutcome::not_executed(true, SIDECAR_NOT_READY);
    }
    match manager.call(
        "mcp.tools.call",
        &tool_call_params(&row.name, tool, arguments),
    ) {
        Ok(payload) => decode_tool_call_result(&payload).unwrap_or_else(|| {
            McpToolCallOutcome::not_executed(true, "mcp.tools.call 应答形状非法")
        }),
        Err(message) => McpToolCallOutcome::not_executed(true, &message),
    }
}
