//! 跨 Tauri IPC 的 sidecar DTO（F20 spec §4.1）。
//!
//! **契约纪律（AGENTS.md）**：所有 DTO 都带 `#[serde(rename_all = "camelCase")]`，
//! 且 JSON 的实际键名与嵌套形状必须由本文件的 serialization contract test 逐键锁定，
//! 不能靠「看起来应该是 camelCase」猜。枚举只用单元变体，规避 `rename_all` 不改
//! 结构变体字段名的坑。

use serde::Serialize;

/// Python runtime 自报的运行期信息（来自 `initialize` / `runtime.info`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    pub name: String,
    pub version: String,
    pub transport: String,
    pub python: String,
    pub protocol_version: String,
    pub providers: Vec<String>,
    pub capabilities: Vec<String>,
    pub methods: Vec<String>,
}

/// sidecar 生命周期状态。**只用单元变体**：`rename_all` 只改体名，
/// 结构变体字段需要 `rename_all_fields`，会埋下跨 IPC 形状漂移的坑。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SidecarState {
    Stopped,
    Ready,
    Degraded,
}

/// 对外暴露的状态快照。IPC 命令 `sidecar_*` 一律返回这个结构，
/// 失败也收敛到这里（`state: Degraded` + `last_error`），**不向上传播为 IPC 错误**。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarStatus {
    pub state: SidecarState,
    pub pid: Option<u32>,
    pub runtime: Option<RuntimeInfo>,
    pub negotiated_capabilities: Vec<String>,
    pub protocol_version: Option<String>,
    pub last_error: Option<String>,
    pub restart_count: u32,
}

impl SidecarStatus {
    /// `Stopped` 的默认快照：构造 manager 时用它，避免手写一堆 `None` / `vec![]`。
    pub fn stopped() -> Self {
        Self {
            state: SidecarState::Stopped,
            pid: None,
            runtime: None,
            negotiated_capabilities: vec![],
            protocol_version: None,
            last_error: None,
            restart_count: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_runtime() -> RuntimeInfo {
        RuntimeInfo {
            name: "harness-hub-runtime".to_string(),
            version: "0.5.0".to_string(),
            transport: "stdio-jsonrpc".to_string(),
            python: "3.14.3".to_string(),
            protocol_version: "1.0".to_string(),
            providers: vec![],
            capabilities: vec!["ping".to_string()],
            methods: vec!["ping".to_string()],
        }
    }

    fn sample_status() -> SidecarStatus {
        SidecarStatus {
            state: SidecarState::Ready,
            pid: Some(1234),
            runtime: Some(sample_runtime()),
            negotiated_capabilities: vec![
                "ping".to_string(),
                "runtime.info".to_string(),
                "shutdown".to_string(),
            ],
            protocol_version: Some("1.0".to_string()),
            last_error: None,
            restart_count: 0,
        }
    }

    /// 三个单元变体必须序列化成小写字符串字面量（非 snake_case、非 SCREAMING）。
    #[test]
    fn sidecar_state_unit_variants_serialize_as_lowercase_literals() {
        assert_eq!(
            serde_json::to_value(SidecarState::Stopped).expect("序列化"),
            json!("stopped")
        );
        assert_eq!(
            serde_json::to_value(SidecarState::Ready).expect("序列化"),
            json!("ready")
        );
        assert_eq!(
            serde_json::to_value(SidecarState::Degraded).expect("序列化"),
            json!("degraded")
        );
    }

    /// `RuntimeInfo` 逐键锁定 camelCase，尤其是 `protocolVersion`。
    #[test]
    fn runtime_info_locks_camel_case_keys_and_nested_shape() {
        let value = serde_json::to_value(sample_runtime()).expect("序列化");

        assert_eq!(
            value,
            json!({
                "name": "harness-hub-runtime",
                "version": "0.5.0",
                "transport": "stdio-jsonrpc",
                "python": "3.14.3",
                "protocolVersion": "1.0",
                "providers": [],
                "capabilities": ["ping"],
                "methods": ["ping"]
            })
        );

        // 逐键断言，防止 `#[serde(rename_all)]` 漏网成 snake_case。
        let object = value.as_object().expect("RuntimeInfo 必须是 JSON 对象");
        for key in [
            "name",
            "version",
            "transport",
            "python",
            "protocolVersion",
            "providers",
            "capabilities",
            "methods",
        ] {
            assert!(object.contains_key(key), "RuntimeInfo 缺少键 {key}");
        }
        assert!(
            !object.contains_key("protocol_version"),
            "protocol_version 必须被重命名成 protocolVersion"
        );
        assert_eq!(object.len(), 8, "RuntimeInfo 不应多出或漏掉字段");
    }

    /// `SidecarStatus` 顶层 7 个键 + 嵌套 `runtime` 形状整体锁定。
    #[test]
    fn sidecar_status_locks_full_json_shape_key_by_key() {
        let value = serde_json::to_value(sample_status()).expect("序列化");

        assert_eq!(
            value,
            json!({
                "state": "ready",
                "pid": 1234,
                "runtime": {
                    "name": "harness-hub-runtime",
                    "version": "0.5.0",
                    "transport": "stdio-jsonrpc",
                    "python": "3.14.3",
                    "protocolVersion": "1.0",
                    "providers": [],
                    "capabilities": ["ping"],
                    "methods": ["ping"]
                },
                "negotiatedCapabilities": ["ping", "runtime.info", "shutdown"],
                "protocolVersion": "1.0",
                "lastError": null,
                "restartCount": 0
            })
        );

        let object = value.as_object().expect("SidecarStatus 必须是 JSON 对象");
        for key in [
            "state",
            "pid",
            "runtime",
            "negotiatedCapabilities",
            "protocolVersion",
            "lastError",
            "restartCount",
        ] {
            assert!(object.contains_key(key), "SidecarStatus 缺少键 {key}");
        }
        assert_eq!(object.len(), 7, "SidecarStatus 不应多出或漏掉字段");

        // 嵌套形状：runtime 是对象、negotiatedCapabilities 是数组、pid 是数字。
        assert!(object["runtime"].is_object(), "runtime 必须是嵌套对象");
        assert!(
            object["negotiatedCapabilities"].is_array(),
            "negotiatedCapabilities 必须是数组"
        );
        assert_eq!(object["pid"], json!(1234));
        assert_eq!(object["lastError"], json!(null));
        assert_eq!(object["restartCount"], json!(0));
    }

    /// `None` / 空值分支也要锁：`Stopped` 状态序列化后不能凭空多出字段。
    #[test]
    fn stopped_status_locks_null_and_empty_defaults() {
        let value = serde_json::to_value(SidecarStatus {
            state: SidecarState::Stopped,
            pid: None,
            runtime: None,
            negotiated_capabilities: vec![],
            protocol_version: None,
            last_error: None,
            restart_count: 0,
        })
        .expect("序列化");

        assert_eq!(
            value,
            json!({
                "state": "stopped",
                "pid": null,
                "runtime": null,
                "negotiatedCapabilities": [],
                "protocolVersion": null,
                "lastError": null,
                "restartCount": 0
            })
        );
    }

    /// `stopped()` 构造器与手写 `Stopped` 快照逐字段一致。
    #[test]
    fn stopped_constructor_matches_explicit_stopped_status() {
        assert_eq!(
            SidecarStatus::stopped(),
            SidecarStatus {
                state: SidecarState::Stopped,
                pid: None,
                runtime: None,
                negotiated_capabilities: vec![],
                protocol_version: None,
                last_error: None,
                restart_count: 0,
            }
        );
    }
}
