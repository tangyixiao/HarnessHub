//! F21 Provider Gateway 的跨 IPC DTO 与安全 decoder（spec §6）。
//!
//! **契约纪律（AGENTS.md）**：`GatewayStatus` 的序列化形状由本文件的
//! serialization contract test 逐键锁定（顶层 8 个 camelCase 键、嵌套形状、
//! 无 snake_case 泄漏），不靠「看起来应该是 camelCase」猜。
//!
//! 所有 DTO 只派生 `Serialize`（与 F20 `dto.rs` 一致）：decoder 从 `&Value`
//! **手工**按字段取值，不依赖 `Deserialize`，也不需要反序列化 `SidecarState`。

use serde::Serialize;
use serde_json::Value;

/// §3.2 声明的 10 个 capability（顺序有意义），供降级构造器与契约测试共用。
pub const F21_CAPABILITIES: &[&str] = &[
    "ping",
    "runtime.info",
    "shutdown",
    "gateway.status",
    "gateway.routing",
    "gateway.streaming",
    "gateway.errors",
    "tokenization.precedence",
    "tokenization.estimate",
    "tokenization.revision",
];

/// §3.2 声明的 12 个 JSON-RPC method（顺序有意义）。
pub const F21_METHODS: &[&str] = &[
    "ping",
    "runtime.info",
    "initialize",
    "shutdown",
    "gateway.status",
    "gateway.providers",
    "gateway.route",
    "gateway.normalize.stream",
    "gateway.normalize.error",
    "tokenization.estimate",
    "tokenization.precedence",
    "tokenization.revision",
];

/// 字符串字段统一截断长度（spec §6.2）。
pub const MAX_FIELD_LEN: usize = 256;
/// 数组字段最多保留的项数（spec §6.2）。
pub const MAX_ITEMS: usize = 64;

const SHAPE_ERROR: &str = "gateway.status 应答形状非法";

/// 静态 provider 描述符（§3.3，冻结顺序与字段）。
const PROVIDER_DESCRIPTORS: &[(&str, &str)] = &[
    ("litellm", "unified"),
    ("openai", "native"),
    ("anthropic", "native"),
    ("google", "native"),
];
const PROVIDER_DETAIL: &str = "optional SDK not installed";

/// Tokenization 静态默认值（§3.3 / §6.1）。
fn static_tokenization() -> GatewayTokenization {
    GatewayTokenization {
        precedence: vec![
            "provider_reported".to_string(),
            "provider_count_api".to_string(),
            "local_exact".to_string(),
            "estimated".to_string(),
        ],
        levels: vec!["local_exact".to_string(), "estimated".to_string()],
        revision_binding: true,
    }
}

fn static_providers() -> Vec<GatewayProvider> {
    PROVIDER_DESCRIPTORS
        .iter()
        .map(|&(id, mode)| GatewayProvider {
            id: id.to_string(),
            mode: mode.to_string(),
            importable: false,
            detail: PROVIDER_DETAIL.to_string(),
        })
        .collect()
}

fn static_capabilities() -> Vec<String> {
    F21_CAPABILITIES.iter().map(|s| (*s).to_string()).collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GatewayState {
    Ready,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayProvider {
    pub id: String,
    pub mode: String,
    pub importable: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayTokenization {
    pub precedence: Vec<String>,
    pub levels: Vec<String>,
    pub revision_binding: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayStatus {
    pub state: GatewayState,
    /// 复用 F20 枚举（Serialize only / Copy），序列化为 "stopped"|"ready"|"degraded"。
    pub sidecar_state: crate::sidecar::dto::SidecarState,
    pub mode: String,
    pub providers: Vec<GatewayProvider>,
    pub tokenization: GatewayTokenization,
    pub capabilities: Vec<String>,
    pub protocol_version: Option<String>,
    pub last_error: Option<String>,
}

impl GatewayStatus {
    /// sidecar 未就绪时的完整降级对象（验收 #6：UI 必须有东西可显示）。
    pub fn unavailable(sidecar_state: crate::sidecar::dto::SidecarState, reason: &str) -> Self {
        Self {
            state: GatewayState::Unavailable,
            sidecar_state,
            mode: "unified".to_string(),
            providers: static_providers(),
            tokenization: static_tokenization(),
            capabilities: static_capabilities(),
            protocol_version: None,
            last_error: Some(reason.to_string()),
        }
    }

    /// RPC 失败 / 应答形状非法时的完整降级对象。
    pub fn degraded(sidecar_state: crate::sidecar::dto::SidecarState, reason: &str) -> Self {
        Self {
            state: GatewayState::Degraded,
            sidecar_state,
            mode: "unified".to_string(),
            providers: static_providers(),
            tokenization: static_tokenization(),
            capabilities: static_capabilities(),
            protocol_version: None,
            last_error: Some(reason.to_string()),
        }
    }
}

fn clamp_str(raw: &str) -> String {
    raw.chars().take(MAX_FIELD_LEN).collect()
}

fn opt_string(raw: Option<&Value>) -> Option<String> {
    raw.and_then(Value::as_str).map(clamp_str)
}

fn string_list(raw: Option<&Value>) -> Vec<String> {
    match raw.and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .filter_map(|item| item.as_str())
            .take(MAX_ITEMS)
            .map(clamp_str)
            .collect(),
        None => Vec::new(),
    }
}

fn provider_mode(raw: Option<&str>) -> String {
    match raw {
        Some("unified") => "unified".to_string(),
        Some("native") => "native".to_string(),
        _ => "unknown".to_string(),
    }
}

fn providers_list(raw: Option<&Value>) -> Vec<GatewayProvider> {
    match raw.and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .take(MAX_ITEMS)
            .filter_map(|item| {
                let object = item.as_object()?;
                Some(GatewayProvider {
                    id: opt_string(object.get("id")).unwrap_or_default(),
                    mode: provider_mode(object.get("mode").and_then(Value::as_str)),
                    importable: object
                        .get("importable")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    detail: opt_string(object.get("detail")).unwrap_or_default(),
                })
            })
            .collect(),
        None => Vec::new(),
    }
}

fn tokenization_from(raw: Option<&Value>) -> GatewayTokenization {
    match raw.and_then(Value::as_object) {
        Some(object) => GatewayTokenization {
            precedence: string_list(object.get("precedence")),
            levels: string_list(object.get("levels")),
            revision_binding: object
                .get("revisionBinding")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
        None => GatewayTokenization {
            precedence: Vec::new(),
            levels: Vec::new(),
            revision_binding: false,
        },
    }
}

/// 把 sidecar `gateway.status` 应答**安全地**收敛成 `GatewayStatus`。
///
/// - 非对象 / 未知 `state` → `degraded`（fail-closed，绝不 panic）。
/// - 其余字段逐项取 `as_str` / `as_bool` / `as_array`，坏类型取安全默认。
/// - `sidecar_state` 恒取函数实参（忽略 payload 里的同名字段）。
/// - `protocol_version` 只认 payload `protocolVersion`，缺失即 `None`。
pub fn decode_gateway_status(
    value: &Value,
    sidecar_state: crate::sidecar::dto::SidecarState,
) -> GatewayStatus {
    let Some(object) = value.as_object() else {
        return GatewayStatus::degraded(sidecar_state, SHAPE_ERROR);
    };
    let state = match object.get("state").and_then(Value::as_str) {
        Some("ready") => GatewayState::Ready,
        Some("degraded") => GatewayState::Degraded,
        Some("unavailable") => GatewayState::Unavailable,
        _ => return GatewayStatus::degraded(sidecar_state, SHAPE_ERROR),
    };

    GatewayStatus {
        state,
        // 关键：sidecar_state 永远来自 Rust 侧实参，payload 无权覆盖。
        sidecar_state,
        mode: opt_string(object.get("mode")).unwrap_or_else(|| "unified".to_string()),
        providers: providers_list(object.get("providers")),
        tokenization: tokenization_from(object.get("tokenization")),
        capabilities: string_list(object.get("capabilities")),
        protocol_version: opt_string(object.get("protocolVersion")),
        last_error: opt_string(object.get("lastError")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::dto::SidecarState;
    use serde_json::{json, Value};

    /// (a) 跨 IPC 契约：完整 `GatewayStatus` 必须精确输出 8 个 camelCase 顶层键，
    /// 嵌套 providers[] 4 键、tokenization 3 键，且不得泄漏任何 snake_case。
    #[test]
    fn gateway_status_serializes_with_locked_camel_case_keys() {
        let status = GatewayStatus {
            state: GatewayState::Ready,
            sidecar_state: SidecarState::Ready,
            mode: "unified".to_string(),
            providers: vec![
                GatewayProvider {
                    id: "litellm".to_string(),
                    mode: "unified".to_string(),
                    importable: false,
                    detail: "optional SDK not installed".to_string(),
                },
                GatewayProvider {
                    id: "openai".to_string(),
                    mode: "native".to_string(),
                    importable: false,
                    detail: "optional SDK not installed".to_string(),
                },
                GatewayProvider {
                    id: "anthropic".to_string(),
                    mode: "native".to_string(),
                    importable: false,
                    detail: "optional SDK not installed".to_string(),
                },
                GatewayProvider {
                    id: "google".to_string(),
                    mode: "native".to_string(),
                    importable: false,
                    detail: "optional SDK not installed".to_string(),
                },
            ],
            tokenization: GatewayTokenization {
                precedence: vec![
                    "provider_reported".to_string(),
                    "provider_count_api".to_string(),
                    "local_exact".to_string(),
                    "estimated".to_string(),
                ],
                levels: vec!["local_exact".to_string(), "estimated".to_string()],
                revision_binding: true,
            },
            capabilities: F21_CAPABILITIES.iter().map(|s| (*s).to_string()).collect(),
            protocol_version: Some("1.0".to_string()),
            last_error: None,
        };

        let value = serde_json::to_value(&status).expect("序列化 GatewayStatus");
        assert_eq!(
            value,
            json!({
                "state": "ready",
                "sidecarState": "ready",
                "mode": "unified",
                "providers": [
                    {"id": "litellm", "mode": "unified", "importable": false, "detail": "optional SDK not installed"},
                    {"id": "openai", "mode": "native", "importable": false, "detail": "optional SDK not installed"},
                    {"id": "anthropic", "mode": "native", "importable": false, "detail": "optional SDK not installed"},
                    {"id": "google", "mode": "native", "importable": false, "detail": "optional SDK not installed"}
                ],
                "tokenization": {
                    "precedence": ["provider_reported", "provider_count_api", "local_exact", "estimated"],
                    "levels": ["local_exact", "estimated"],
                    "revisionBinding": true
                },
                "capabilities": [
                    "ping", "runtime.info", "shutdown", "gateway.status", "gateway.routing",
                    "gateway.streaming", "gateway.errors", "tokenization.precedence",
                    "tokenization.estimate", "tokenization.revision"
                ],
                "protocolVersion": "1.0",
                "lastError": null
            })
        );

        // 顶层恰好 8 键，且不得泄漏任何 snake_case / 本该在嵌套里的键。
        let object = value.as_object().expect("GatewayStatus 必须是对象");
        assert_eq!(object.len(), 8, "顶层键数锁定为 8");
        for leaked in [
            "sidecar_state",
            "protocol_version",
            "last_error",
            "revision_binding",
            "precedence",
            "levels",
        ] {
            assert!(
                !object.contains_key(leaked),
                "顶层泄漏 snake_case：{leaked}"
            );
        }

        let first_provider = value["providers"][0]
            .as_object()
            .expect("providers[0] 必须是对象");
        assert_eq!(first_provider.len(), 4, "providers[] 必须是 4 键");
        for key in ["id", "mode", "importable", "detail"] {
            assert!(first_provider.contains_key(key), "providers[] 缺键：{key}");
        }

        let tokenization = value["tokenization"]
            .as_object()
            .expect("tokenization 必须是对象");
        assert_eq!(tokenization.len(), 3, "tokenization 必须是 3 键");
        assert!(
            !tokenization.contains_key("revision_binding"),
            "tokenization 泄漏 snake_case revision_binding"
        );
    }

    /// (b) 降级构造器必须产出完整可解析对象（4 providers / 10 capabilities），
    /// 而不是空数组；state 与 sidecarState 必须逐值正确。
    #[test]
    fn gateway_unavailable_and_degraded_constructors_produce_complete_objects() {
        let unavailable = GatewayStatus::unavailable(SidecarState::Stopped, "sidecar 没起来");
        assert_eq!(unavailable.state, GatewayState::Unavailable);
        assert_eq!(unavailable.sidecar_state, SidecarState::Stopped);
        assert_eq!(unavailable.mode, "unified");
        assert_eq!(unavailable.providers.len(), 4);
        assert_eq!(unavailable.capabilities.len(), 10);
        assert_eq!(unavailable.tokenization.precedence.len(), 4);
        assert_eq!(unavailable.tokenization.levels.len(), 2);
        assert!(unavailable.tokenization.revision_binding);
        assert!(unavailable.protocol_version.is_none());
        assert_eq!(unavailable.last_error.as_deref(), Some("sidecar 没起来"));

        let u = serde_json::to_value(&unavailable).expect("序列化 unavailable");
        assert_eq!(u["state"], "unavailable");
        assert_eq!(u["sidecarState"], "stopped");
        assert_eq!(u["lastError"], "sidecar 没起来");
        assert_eq!(u["protocolVersion"], serde_json::Value::Null);
        assert_eq!(u["providers"].as_array().expect("providers 数组").len(), 4);
        assert_eq!(
            u["capabilities"]
                .as_array()
                .expect("capabilities 数组")
                .len(),
            10
        );
        assert_eq!(
            u["tokenization"]["precedence"]
                .as_array()
                .expect("precedence 数组")
                .len(),
            4
        );
        assert_eq!(u["tokenization"]["revisionBinding"], true);

        let degraded = GatewayStatus::degraded(SidecarState::Degraded, "RPC 挂了");
        assert_eq!(degraded.state, GatewayState::Degraded);
        assert_eq!(degraded.sidecar_state, SidecarState::Degraded);
        assert_eq!(degraded.providers.len(), 4);
        assert_eq!(degraded.capabilities.len(), 10);
        assert_eq!(degraded.tokenization.precedence.len(), 4);
        assert_eq!(degraded.last_error.as_deref(), Some("RPC 挂了"));

        let d = serde_json::to_value(&degraded).expect("序列化 degraded");
        assert_eq!(d["state"], "degraded");
        assert_eq!(d["sidecarState"], "degraded");
        assert_eq!(d["lastError"], "RPC 挂了");
        assert_eq!(d["protocolVersion"], serde_json::Value::Null);
        assert_eq!(d["providers"].as_array().expect("providers 数组").len(), 4);
    }

    /// (c1) 完整合法 payload 逐字段 round-trip。
    #[test]
    fn decode_gateway_status_round_trips_full_valid_payload() {
        let payload = json!({
            "state": "ready",
            "sidecarState": "degraded",
            "mode": "unified",
            "protocolVersion": "1.0",
            "providers": [
                {"id": "litellm", "mode": "unified", "importable": false, "detail": "optional SDK not installed"},
                {"id": "openai", "mode": "native", "importable": false, "detail": "optional SDK not installed"},
                {"id": "anthropic", "mode": "native", "importable": false, "detail": "optional SDK not installed"},
                {"id": "google", "mode": "native", "importable": false, "detail": "optional SDK not installed"}
            ],
            "tokenization": {
                "precedence": ["provider_reported", "provider_count_api", "local_exact", "estimated"],
                "levels": ["local_exact", "estimated"],
                "revisionBinding": true
            },
            "capabilities": [
                "ping", "runtime.info", "shutdown", "gateway.status", "gateway.routing",
                "gateway.streaming", "gateway.errors", "tokenization.precedence",
                "tokenization.estimate", "tokenization.revision"
            ],
            "methods": ["ping", "runtime.info"],
            "lastError": null
        });
        let status = decode_gateway_status(&payload, SidecarState::Ready);
        assert_eq!(status.state, GatewayState::Ready);
        assert_eq!(status.sidecar_state, SidecarState::Ready);
        assert_eq!(status.mode, "unified");
        assert_eq!(status.protocol_version.as_deref(), Some("1.0"));
        assert_eq!(status.providers.len(), 4);
        assert_eq!(status.providers[0].id, "litellm");
        assert_eq!(status.providers[0].mode, "unified");
        assert_eq!(status.providers[3].mode, "native");
        assert_eq!(status.tokenization.precedence.len(), 4);
        assert_eq!(status.tokenization.levels.len(), 2);
        assert!(status.tokenization.revision_binding);
        assert_eq!(status.capabilities.len(), 10);
        assert!(status.last_error.is_none());

        let value = serde_json::to_value(&status).expect("序列化 decode 结果");
        assert_eq!(value["state"], "ready");
        assert_eq!(value["sidecarState"], "ready");
        assert_eq!(value["protocolVersion"], "1.0");
        assert_eq!(
            value["providers"].as_array().expect("providers 数组").len(),
            4
        );
    }

    /// (c2) 坏输入（非对象 / 未知 state / 全错类型）一律 fail-closed 成 degraded
    /// 完整对象，绝不 panic。
    #[test]
    fn decode_gateway_status_is_fail_closed_on_bad_inputs() {
        let bad_inputs: Vec<Value> = vec![
            json!(null),
            json!([]),
            json!("not-an-object"),
            json!({"state": "weird"}),
            json!({
                "state": 42,
                "mode": true,
                "providers": 7,
                "tokenization": [],
                "capabilities": {},
                "protocolVersion": false,
                "lastError": 1
            }),
        ];
        for (index, input) in bad_inputs.iter().enumerate() {
            let status = decode_gateway_status(input, SidecarState::Degraded);
            let value = serde_json::to_value(&status)
                .unwrap_or_else(|_| panic!("坏输入 #{index} 必须可序列化"));
            assert_eq!(value["state"], "degraded", "坏输入 #{index}");
            assert_eq!(value["sidecarState"], "degraded", "坏输入 #{index}");
            assert_eq!(value["mode"], "unified", "坏输入 #{index}");
            assert_eq!(
                value["protocolVersion"],
                serde_json::Value::Null,
                "坏输入 #{index}"
            );
            assert!(value["lastError"].is_string(), "坏输入 #{index}");
            assert_eq!(
                value["providers"].as_array().expect("providers 数组").len(),
                4,
                "坏输入 #{index}"
            );
            assert_eq!(
                value["capabilities"]
                    .as_array()
                    .expect("capabilities 数组")
                    .len(),
                10,
                "坏输入 #{index}"
            );
            assert_eq!(
                value["tokenization"]["precedence"]
                    .as_array()
                    .expect("precedence 数组")
                    .len(),
                4,
                "坏输入 #{index}"
            );
            assert!(value.get("sidecar_state").is_none(), "坏输入 #{index}");
        }
    }

    /// (c3) state 合法但其它字段全是坏类型时，逐字段取安全默认、不 panic。
    #[test]
    fn decode_gateway_status_uses_safe_defaults_for_wrong_field_types() {
        let payload = json!({
            "state": "ready",
            "sidecarState": 12345,
            "mode": false,
            "providers": "not-an-array",
            "tokenization": "not-an-object",
            "capabilities": 99,
            "protocolVersion": ["not", "a", "string"],
            "lastError": 42
        });
        let status = decode_gateway_status(&payload, SidecarState::Ready);
        assert_eq!(status.state, GatewayState::Ready);
        assert_eq!(status.sidecar_state, SidecarState::Ready);
        assert_eq!(status.mode, "unified");
        assert!(status.providers.is_empty());
        assert!(status.capabilities.is_empty());
        assert!(status.tokenization.precedence.is_empty());
        assert!(status.tokenization.levels.is_empty());
        assert!(!status.tokenization.revision_binding);
        assert!(status.protocol_version.is_none());
        assert!(status.last_error.is_none());
        let value = serde_json::to_value(&status).expect("坏字段必须可序列化");
        assert_eq!(value["state"], "ready");
        assert_eq!(value["sidecarState"], "ready");
        assert!(value.get("protocol_version").is_none());
        assert!(value.get("last_error").is_none());
    }

    /// (c4) sidecarState 永远取函数实参；protocolVersion 只认 payload，缺失即 None。
    #[test]
    fn decode_gateway_status_argument_wins_and_protocol_version_is_payload_only() {
        let payload = json!({
            "state": "ready",
            "sidecarState": "degraded",
            "protocolVersion": "1.0"
        });
        let status = decode_gateway_status(&payload, SidecarState::Ready);
        assert_eq!(status.sidecar_state, SidecarState::Ready);
        let value = serde_json::to_value(&status).expect("序列化");
        assert_eq!(value["sidecarState"], "ready", "实参必须压过 payload");
        assert_eq!(value["protocolVersion"], "1.0");

        let missing = json!({"state": "ready", "sidecarState": "stopped"});
        let status = decode_gateway_status(&missing, SidecarState::Degraded);
        assert_eq!(status.protocol_version, None);
        assert_eq!(status.sidecar_state, SidecarState::Degraded);
        let value = serde_json::to_value(&status).expect("序列化");
        assert_eq!(value["protocolVersion"], serde_json::Value::Null);
    }

    /// (c5) 字符串按 MAX_FIELD_LEN 截断，数组按 MAX_ITEMS 截顶。
    #[test]
    fn decode_gateway_status_truncates_strings_and_caps_arrays() {
        let long = "x".repeat(300);
        let many_providers: Vec<Value> = (0..100)
            .map(|_| json!({"id": "p", "mode": "native", "importable": true, "detail": "d"}))
            .collect();
        let many_strings: Vec<Value> = (0..100).map(|i| json!(format!("item-{i}"))).collect();
        let payload = json!({
            "state": "ready",
            "mode": long.clone(),
            "providers": many_providers,
            "tokenization": {
                "precedence": many_strings.clone(),
                "levels": many_strings.clone(),
                "revisionBinding": true
            },
            "capabilities": many_strings.clone(),
            "protocolVersion": long.clone(),
            "lastError": long.clone()
        });
        let status = decode_gateway_status(&payload, SidecarState::Ready);
        assert_eq!(status.mode.chars().count(), MAX_FIELD_LEN);
        assert_eq!(status.providers.len(), MAX_ITEMS);
        assert_eq!(status.tokenization.precedence.len(), MAX_ITEMS);
        assert_eq!(status.tokenization.levels.len(), MAX_ITEMS);
        assert_eq!(status.capabilities.len(), MAX_ITEMS);
        let pv = status
            .protocol_version
            .as_deref()
            .expect("protocolVersion 应存在");
        assert_eq!(pv.chars().count(), MAX_FIELD_LEN);
        let le = status.last_error.as_deref().expect("lastError 应存在");
        assert_eq!(le.chars().count(), MAX_FIELD_LEN);
        let value = serde_json::to_value(&status).expect("截断结果必须可序列化");
        assert_eq!(
            value["mode"].as_str().expect("mode 字符串").chars().count(),
            MAX_FIELD_LEN
        );
    }

    /// (d) 静态声明常量与 §3.2 逐项一致（顺序有意义）。
    #[test]
    fn gateway_f21_static_constants_lock_declared_capabilities_and_methods() {
        assert_eq!(F21_CAPABILITIES.len(), 10);
        assert_eq!(
            F21_CAPABILITIES,
            &[
                "ping",
                "runtime.info",
                "shutdown",
                "gateway.status",
                "gateway.routing",
                "gateway.streaming",
                "gateway.errors",
                "tokenization.precedence",
                "tokenization.estimate",
                "tokenization.revision",
            ][..]
        );
        assert_eq!(F21_METHODS.len(), 12);
        assert_eq!(
            F21_METHODS,
            &[
                "ping",
                "runtime.info",
                "initialize",
                "shutdown",
                "gateway.status",
                "gateway.providers",
                "gateway.route",
                "gateway.normalize.stream",
                "gateway.normalize.error",
                "tokenization.estimate",
                "tokenization.precedence",
                "tokenization.revision",
            ][..]
        );
    }
}
