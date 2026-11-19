//! F23 模型资产平面：跨 IPC DTO + fail-closed decoder + sidecar 编排
//! （docs/specs/2026-11-09-f23-model-assets-design.md §3.2–§3.6）。
//!
//! **契约纪律（AGENTS.md）**：所有 DTO 的序列化形状由 `commands.rs` 的
//! serialization contract test 逐键锁定（camelCase、嵌套形状、无 snake_case
//! 泄漏），与 F21 `gateway.rs` / F22 `client_bridge.rs` 同规则。
//!
//! 架构归属（spec §1）：Python 拥有网络与缓存扫描；Rust 只做参数校验（§2 红线 4）、
//! 编排与 DTO。**env 值绝不进入 IPC**（HF token 只经环境继承进 sidecar 进程）。
//!
//! 薄命令纪律：任何失败（sidecar 未就绪 / RPC 失败 / 应答形状非法）都收敛成
//! `available: false` 的完整降级对象，**永不返回 IPC Err**（ADR-0006）。
//! `models.download.start` 在 sidecar 未就绪时收敛为 `{state: "unavailable"}`
//! （spec §3.6 原文）。

use serde::Serialize;
use serde_json::Value;

use crate::sidecar::manager::SidecarManager;

// ---- 冻结常量（与 Python models/dto.py 对齐，spec §3.2–§3.5） ----

/// models 平面协议版本（spec §3.2 `protocolVersion: "1.0"`）。
pub const MODELS_PROTOCOL_VERSION: &str = "1.0";

/// 下载状态集（spec §3.2，顺序即契约）。
pub const MODELS_DOWNLOAD_STATES: &[&str] = &[
    "idle",
    "preparing",
    "downloading",
    "completed",
    "failed",
    "cancelled",
];

/// 字符串字段统一截断（防御性；spec 只明确 lastError ≤ 256）。
pub const MODELS_MAX_STRING: usize = 256;
/// 路径类字段（cacheRoot）的防御性截断上限。
pub const MODELS_MAX_PATH: usize = 1024;
/// downloads 数组上限（Python 侧终态保留上限就是 64）。
pub const MODELS_MAX_ITEMS: usize = 64;
/// cache entries 上限（spec §3.4）。
pub const MODELS_MAX_CACHE_ENTRIES: usize = 128;
/// models.info files 上限（spec §3.3）。
pub const MODELS_INFO_MAX_FILES: usize = 64;

const SHAPE_ERROR: &str = "models 应答形状非法";

// ---- 参数校验（spec §2 红线 4：Rust 下发前校验，Python 双保险） ----

/// repo_id / revision 白名单：`^[A-Za-z0-9._/-]+$` 且拒绝空段 / `.` / `..` 段。
pub fn validate_model_identifier(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("repoId/revision 不能为空".to_string());
    }
    if trimmed.chars().count() > MODELS_MAX_STRING {
        return Err("repoId/revision 超长".to_string());
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        return Err("repoId/revision 含非法字符（只允许 A-Za-z0-9._-/）".to_string());
    }
    for segment in trimmed.split('/') {
        if matches!(segment, "" | "." | "..") {
            return Err("repoId/revision 含非法路径段（拒绝空段 / . / ..）".to_string());
        }
    }
    Ok(trimmed.to_string())
}

/// files 白名单：None 透传；数组逐项过同一条校验；空数组拒绝（语义歧义，fail-closed）。
pub fn validate_model_file_whitelist(
    files: Option<&Vec<String>>,
) -> Result<Option<Vec<String>>, String> {
    let Some(files) = files else {
        return Ok(None);
    };
    if files.is_empty() {
        return Err("files 不允许是空数组（省略即全量）".to_string());
    }
    let mut validated = Vec::with_capacity(files.len());
    for file in files {
        validated.push(validate_model_identifier(file)?);
    }
    Ok(Some(validated))
}

// ---- DTO（只派生 Serialize；decoder 从 &Value 手工取值，与 F21 同纪律） ----

fn clamp_str(raw: &str, limit: usize) -> String {
    raw.chars().take(limit).collect()
}

fn opt_string(raw: Option<&Value>, limit: usize) -> Option<String> {
    raw.and_then(Value::as_str)
        .map(|value| clamp_str(value, limit))
}

fn opt_i64(raw: Option<&Value>) -> Option<i64> {
    raw.and_then(Value::as_i64)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsSdkStatus {
    pub huggingface_hub: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadStateDto {
    pub repo_id: String,
    pub revision: String,
    pub state: String,
    pub via: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsStatus {
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub sdk: ModelsSdkStatus,
    pub cache_root: Option<String>,
    pub downloads: Vec<ModelDownloadStateDto>,
    pub protocol_version: Option<String>,
}

impl ModelsStatus {
    /// sidecar 未就绪 / RPC 失败 / 形状非法时的完整降级对象。
    pub fn unavailable(reason: &str) -> Self {
        Self {
            available: false,
            unavailable_reason: Some(reason.to_string()),
            sdk: ModelsSdkStatus {
                huggingface_hub: false,
            },
            cache_root: None,
            downloads: Vec::new(),
            protocol_version: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelAssetEntry {
    pub repo_id: String,
    pub revisions: Vec<String>,
    pub size_bytes: i64,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelsCacheListing {
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub cache_root: Option<String>,
    pub entries: Vec<ModelAssetEntry>,
}

impl ModelsCacheListing {
    pub fn unavailable(reason: &str) -> Self {
        Self {
            available: false,
            unavailable_reason: Some(reason.to_string()),
            cache_root: None,
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFileInfoDto {
    pub path: String,
    pub size_bytes: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRepoInfo {
    pub repo_id: String,
    pub revision: String,
    pub size_bytes: Option<i64>,
    pub file_count: i64,
    pub files: Vec<ModelFileInfoDto>,
    pub files_truncated: bool,
    pub gated: bool,
    pub via: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfoOutcome {
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub info: Option<ModelRepoInfo>,
}

impl ModelInfoOutcome {
    pub fn unavailable(reason: &str) -> Self {
        Self {
            available: false,
            unavailable_reason: Some(reason.to_string()),
            info: None,
        }
    }
}

/// `models.download.start` 的 Rust 侧 outcome（spec §3.5 两分支 + §3.6 unavailable）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadStartOutcome {
    /// "preparing" | "needsConfirmation" | "unavailable" | "error"
    pub state: String,
    pub download_id: Option<String>,
    pub estimated_bytes: Option<i64>,
    pub reason: Option<String>,
}

impl ModelDownloadStartOutcome {
    pub fn unavailable(reason: &str) -> Self {
        Self {
            state: "unavailable".to_string(),
            download_id: None,
            estimated_bytes: None,
            reason: Some(reason.to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadProgress {
    pub repo_id: String,
    pub revision: String,
    pub state: String,
    pub bytes_done: i64,
    pub bytes_total: Option<i64>,
    pub file_done: i64,
    pub file_count: i64,
    pub current_file: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadProgressOutcome {
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub progress: Option<ModelDownloadProgress>,
}

impl ModelDownloadProgressOutcome {
    pub fn unavailable(reason: &str) -> Self {
        Self {
            available: false,
            unavailable_reason: Some(reason.to_string()),
            progress: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadCancelOutcome {
    pub available: bool,
    pub unavailable_reason: Option<String>,
    pub download_id: Option<String>,
    pub state: Option<String>,
}

impl ModelDownloadCancelOutcome {
    pub fn unavailable(reason: &str) -> Self {
        Self {
            available: false,
            unavailable_reason: Some(reason.to_string()),
            download_id: None,
            state: None,
        }
    }
}

// ---- fail-closed decoder（&Value 手工取值；坏类型取安全默认，绝不 panic） ----

fn downloads_list(raw: Option<&Value>) -> Vec<ModelDownloadStateDto> {
    match raw.and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .take(MODELS_MAX_ITEMS)
            .filter_map(|item| {
                let object = item.as_object()?;
                Some(ModelDownloadStateDto {
                    repo_id: opt_string(object.get("repoId"), MODELS_MAX_STRING)
                        .unwrap_or_default(),
                    revision: opt_string(object.get("revision"), MODELS_MAX_STRING)
                        .unwrap_or_default(),
                    state: opt_string(object.get("state"), MODELS_MAX_STRING).unwrap_or_default(),
                    via: opt_string(object.get("via"), MODELS_MAX_STRING),
                })
            })
            .collect(),
        None => Vec::new(),
    }
}

/// 把 sidecar `models.status` 应答安全收敛成 payload。外层非对象 → Err（形状非法）。
pub fn decode_models_status(value: &Value) -> Result<ModelsStatus, String> {
    let Some(object) = value.as_object() else {
        return Err(SHAPE_ERROR.to_string());
    };
    let sdk = object
        .get("sdk")
        .and_then(Value::as_object)
        .and_then(|sdk| sdk.get("huggingfaceHub"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok(ModelsStatus {
        available: true,
        unavailable_reason: None,
        sdk: ModelsSdkStatus {
            huggingface_hub: sdk,
        },
        cache_root: opt_string(object.get("cacheRoot"), MODELS_MAX_PATH),
        downloads: downloads_list(object.get("downloads")),
        protocol_version: opt_string(object.get("protocolVersion"), MODELS_MAX_STRING),
    })
}

fn entries_list(raw: Option<&Value>) -> Vec<ModelAssetEntry> {
    match raw.and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .take(MODELS_MAX_CACHE_ENTRIES)
            .filter_map(|item| {
                let object = item.as_object()?;
                Some(ModelAssetEntry {
                    repo_id: opt_string(object.get("repoId"), MODELS_MAX_STRING)
                        .unwrap_or_default(),
                    revisions: match object.get("revisions").and_then(Value::as_array) {
                        Some(list) => list
                            .iter()
                            .filter_map(|entry| entry.as_str())
                            .take(MODELS_MAX_CACHE_ENTRIES)
                            .map(|entry| clamp_str(entry, MODELS_MAX_STRING))
                            .collect(),
                        None => Vec::new(),
                    },
                    size_bytes: opt_i64(object.get("sizeBytes")).unwrap_or(0),
                    last_used_at: opt_string(object.get("lastUsedAt"), MODELS_MAX_STRING),
                })
            })
            .collect(),
        None => Vec::new(),
    }
}

/// 把 sidecar `models.cache.list` 应答安全收敛成 listing。外层非对象 → Err。
pub fn decode_model_cache_entries(value: &Value) -> Result<ModelsCacheListing, String> {
    let Some(object) = value.as_object() else {
        return Err(SHAPE_ERROR.to_string());
    };
    Ok(ModelsCacheListing {
        available: true,
        unavailable_reason: None,
        cache_root: opt_string(object.get("cacheRoot"), MODELS_MAX_PATH),
        entries: entries_list(object.get("entries")),
    })
}

fn files_list(raw: Option<&Value>) -> Vec<ModelFileInfoDto> {
    match raw.and_then(Value::as_array) {
        Some(items) => items
            .iter()
            .take(MODELS_INFO_MAX_FILES)
            .filter_map(|item| {
                let object = item.as_object()?;
                Some(ModelFileInfoDto {
                    path: opt_string(object.get("path"), MODELS_MAX_STRING).unwrap_or_default(),
                    size_bytes: opt_i64(object.get("sizeBytes")),
                })
            })
            .collect(),
        None => Vec::new(),
    }
}

/// 把 sidecar `models.info` 应答安全收敛成 info。外层非对象 → Err。
pub fn decode_model_repo_info(value: &Value) -> Result<ModelRepoInfo, String> {
    let Some(object) = value.as_object() else {
        return Err(SHAPE_ERROR.to_string());
    };
    Ok(ModelRepoInfo {
        repo_id: opt_string(object.get("repoId"), MODELS_MAX_STRING).unwrap_or_default(),
        revision: opt_string(object.get("revision"), MODELS_MAX_STRING).unwrap_or_default(),
        size_bytes: opt_i64(object.get("sizeBytes")),
        file_count: opt_i64(object.get("fileCount")).unwrap_or(0),
        files: files_list(object.get("files")),
        files_truncated: object
            .get("filesTruncated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        gated: object
            .get("gated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        via: opt_string(object.get("via"), MODELS_MAX_STRING).unwrap_or_default(),
    })
}

/// 把 sidecar `models.download.start` 应答收敛成 outcome（两分支 + 未知形状 → error）。
pub fn decode_download_start(value: &Value) -> ModelDownloadStartOutcome {
    let Some(object) = value.as_object() else {
        return ModelDownloadStartOutcome {
            state: "error".to_string(),
            download_id: None,
            estimated_bytes: None,
            reason: Some(SHAPE_ERROR.to_string()),
        };
    };
    let state = object.get("state").and_then(Value::as_str);
    match state {
        Some("preparing") => ModelDownloadStartOutcome {
            state: "preparing".to_string(),
            download_id: opt_string(object.get("downloadId"), MODELS_MAX_STRING),
            estimated_bytes: None,
            reason: None,
        },
        // spec §3.5 预算门分支：Python 直接回 {needsConfirmation, estimatedBytes}（无 state 字段）。
        _ if object
            .get("needsConfirmation")
            .and_then(Value::as_bool)
            .unwrap_or(false) =>
        {
            ModelDownloadStartOutcome {
                state: "needsConfirmation".to_string(),
                download_id: None,
                estimated_bytes: opt_i64(object.get("estimatedBytes")),
                reason: None,
            }
        }
        _ => ModelDownloadStartOutcome {
            state: "error".to_string(),
            download_id: None,
            estimated_bytes: None,
            reason: Some(SHAPE_ERROR.to_string()),
        },
    }
}

/// 把 sidecar `models.download.progress` 应答安全收敛成 progress。外层非对象 → Err。
pub fn decode_download_progress(value: &Value) -> Result<ModelDownloadProgress, String> {
    let Some(object) = value.as_object() else {
        return Err(SHAPE_ERROR.to_string());
    };
    Ok(ModelDownloadProgress {
        repo_id: opt_string(object.get("repoId"), MODELS_MAX_STRING).unwrap_or_default(),
        revision: opt_string(object.get("revision"), MODELS_MAX_STRING).unwrap_or_default(),
        state: opt_string(object.get("state"), MODELS_MAX_STRING).unwrap_or_default(),
        bytes_done: opt_i64(object.get("bytesDone")).unwrap_or(0),
        bytes_total: opt_i64(object.get("bytesTotal")),
        file_done: opt_i64(object.get("fileDone")).unwrap_or(0),
        file_count: opt_i64(object.get("fileCount")).unwrap_or(0),
        current_file: opt_string(object.get("currentFile"), MODELS_MAX_STRING),
        last_error: opt_string(object.get("lastError"), MODELS_MAX_STRING),
    })
}

fn decode_download_cancel(value: &Value) -> Option<(String, String)> {
    let object = value.as_object()?;
    let download_id = opt_string(object.get("downloadId"), MODELS_MAX_STRING)?;
    let state = opt_string(object.get("state"), MODELS_MAX_STRING)?;
    Some((download_id, state))
}

// ---- 编排（薄命令的唯一领域入口；manager.call 失败全部收敛为降级对象） ----

/// `models.status` 编排：sidecar 未就绪 / RPC 失败 / 形状非法 → unavailable。
pub fn status(manager: &SidecarManager) -> ModelsStatus {
    match manager.call("models.status", &serde_json::json!({})) {
        Ok(value) => match decode_models_status(&value) {
            Ok(status) => status,
            Err(reason) => ModelsStatus::unavailable(&reason),
        },
        Err(message) => ModelsStatus::unavailable(&format!("models.status 调用失败：{message}")),
    }
}

/// `models.cache.list` 编排（零网络的只读投影）。
pub fn cache_list(manager: &SidecarManager) -> ModelsCacheListing {
    match manager.call("models.cache.list", &serde_json::json!({})) {
        Ok(value) => match decode_model_cache_entries(&value) {
            Ok(listing) => listing,
            Err(reason) => ModelsCacheListing::unavailable(&reason),
        },
        Err(message) => {
            ModelsCacheListing::unavailable(&format!("models.cache.list 调用失败：{message}"))
        }
    }
}

/// `models.info` 编排：入参在 Rust 先过白名单校验（spec §2 红线 4），失败不触 sidecar。
pub fn info(manager: &SidecarManager, repo_id: &str, revision: &str) -> ModelInfoOutcome {
    let repo_id = match validate_model_identifier(repo_id) {
        Ok(value) => value,
        Err(reason) => return ModelInfoOutcome::unavailable(&format!("参数非法：{reason}")),
    };
    let revision = match validate_model_identifier(revision) {
        Ok(value) => value,
        Err(reason) => return ModelInfoOutcome::unavailable(&format!("参数非法：{reason}")),
    };
    match manager.call(
        "models.info",
        &serde_json::json!({ "repoId": repo_id, "revision": revision }),
    ) {
        Ok(value) => match decode_model_repo_info(&value) {
            Ok(info) => ModelInfoOutcome {
                available: true,
                unavailable_reason: None,
                info: Some(info),
            },
            Err(reason) => ModelInfoOutcome::unavailable(&reason),
        },
        Err(message) => ModelInfoOutcome::unavailable(&format!("models.info 调用失败：{message}")),
    }
}

/// `models.download.start` 编排：预算确认门由 Python 执行（预估来自 info），
/// Rust 只校验参数白名单并透传 `confirmedDiskBudget`；env 值绝不入参。
pub fn download_start(
    manager: &SidecarManager,
    repo_id: &str,
    revision: &str,
    files: Option<&Vec<String>>,
    confirmed_disk_budget: bool,
) -> ModelDownloadStartOutcome {
    let repo_id = match validate_model_identifier(repo_id) {
        Ok(value) => value,
        Err(reason) => {
            return ModelDownloadStartOutcome {
                state: "error".to_string(),
                download_id: None,
                estimated_bytes: None,
                reason: Some(format!("参数非法：{reason}")),
            }
        }
    };
    let revision = match validate_model_identifier(revision) {
        Ok(value) => value,
        Err(reason) => {
            return ModelDownloadStartOutcome {
                state: "error".to_string(),
                download_id: None,
                estimated_bytes: None,
                reason: Some(format!("参数非法：{reason}")),
            }
        }
    };
    let whitelist = match validate_model_file_whitelist(files) {
        Ok(value) => value,
        Err(reason) => {
            return ModelDownloadStartOutcome {
                state: "error".to_string(),
                download_id: None,
                estimated_bytes: None,
                reason: Some(format!("参数非法：{reason}")),
            }
        }
    };
    let mut params = serde_json::json!({
        "repoId": repo_id,
        "revision": revision,
        "confirmedDiskBudget": confirmed_disk_budget,
    });
    if let Some(files) = whitelist {
        params["files"] = serde_json::json!(files);
    }
    match manager.call("models.download.start", &params) {
        Ok(value) => decode_download_start(&value),
        Err(message) => ModelDownloadStartOutcome::unavailable(&format!(
            "models.download.start 调用失败：{message}"
        )),
    }
}

/// `models.download.progress` 编排。
pub fn download_progress(
    manager: &SidecarManager,
    download_id: &str,
) -> ModelDownloadProgressOutcome {
    if download_id.trim().is_empty() {
        return ModelDownloadProgressOutcome::unavailable("参数非法：downloadId 不能为空");
    }
    match manager.call(
        "models.download.progress",
        &serde_json::json!({ "downloadId": download_id }),
    ) {
        Ok(value) => match decode_download_progress(&value) {
            Ok(progress) => ModelDownloadProgressOutcome {
                available: true,
                unavailable_reason: None,
                progress: Some(progress),
            },
            Err(reason) => ModelDownloadProgressOutcome::unavailable(&reason),
        },
        Err(message) => ModelDownloadProgressOutcome::unavailable(&format!(
            "models.download.progress 调用失败：{message}"
        )),
    }
}

/// `models.download.cancel` 编排（幂等；未知 id 由 Python 收敛为 -32602 → unavailable 语义）。
pub fn download_cancel(manager: &SidecarManager, download_id: &str) -> ModelDownloadCancelOutcome {
    if download_id.trim().is_empty() {
        return ModelDownloadCancelOutcome::unavailable("参数非法：downloadId 不能为空");
    }
    match manager.call(
        "models.download.cancel",
        &serde_json::json!({ "downloadId": download_id }),
    ) {
        Ok(value) => match decode_download_cancel(&value) {
            Some((download_id, state)) => ModelDownloadCancelOutcome {
                available: true,
                unavailable_reason: None,
                download_id: Some(download_id),
                state: Some(state),
            },
            None => ModelDownloadCancelOutcome::unavailable(SHAPE_ERROR),
        },
        Err(message) => ModelDownloadCancelOutcome::unavailable(&format!(
            "models.download.cancel 调用失败：{message}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn cold_manager() -> std::sync::Arc<SidecarManager> {
        SidecarManager::new(crate::sidecar::config::SidecarConfig {
            python: PathBuf::from("no-such-python-interpreter-9f3b.exe"),
            runtime_root: PathBuf::from("python"),
            host_version: "0.5.0".to_string(),
        })
    }

    // ---- 参数校验 ----

    #[test]
    fn identifier_validation_locks_the_frozen_whitelist() {
        for value in [
            "bert",
            "org/name",
            "org/sub/name",
            "x.y-z_1/Name.v2",
            "pr/1",
        ] {
            assert_eq!(
                validate_model_identifier(value).expect(value),
                value,
                "{value} 应通过"
            );
        }
        for value in [
            "",
            "org name",
            "a\\b",
            "..",
            "org/../name",
            "../etc",
            "org//name",
            "/org",
            "org/",
        ] {
            assert!(validate_model_identifier(value).is_err(), "{value} 应拒绝");
        }
    }

    #[test]
    fn file_whitelist_validation_rejects_empty_and_traversal() {
        assert_eq!(validate_model_file_whitelist(None).expect("None"), None);
        let files = vec![
            "model.safetensors".to_string(),
            "sub/tokenizer.json".to_string(),
        ];
        assert_eq!(
            validate_model_file_whitelist(Some(&files)).expect("合法"),
            Some(files.clone())
        );
        assert!(
            validate_model_file_whitelist(Some(&Vec::new())).is_err(),
            "空数组拒绝"
        );
        let bad = vec!["../config".to_string()];
        assert!(
            validate_model_file_whitelist(Some(&bad)).is_err(),
            "穿越拒绝"
        );
    }

    // ---- decoder：fail-closed ----

    #[test]
    fn decode_models_status_round_trips_and_is_fail_closed() {
        let payload = json!({
            "sdk": {"huggingfaceHub": true},
            "cacheRoot": "C:/cache/hub",
            "downloads": [
                {"repoId": "org/name", "revision": "main", "state": "completed", "via": "stdlib"}
            ],
            "protocolVersion": "1.0"
        });
        let status = decode_models_status(&payload).expect("合法 payload");
        assert!(status.available);
        assert!(status.sdk.huggingface_hub);
        assert_eq!(status.cache_root.as_deref(), Some("C:/cache/hub"));
        assert_eq!(status.downloads.len(), 1);
        assert_eq!(status.downloads[0].repo_id, "org/name");
        assert_eq!(status.downloads[0].state, "completed");
        assert_eq!(status.protocol_version.as_deref(), Some("1.0"));

        for bad in [json!(null), json!([]), json!("nope")] {
            assert!(decode_models_status(&bad).is_err(), "外层非对象必须 Err");
        }
        // 内层坏类型 → 安全默认，不 panic。
        let wrong = json!({"sdk": 7, "cacheRoot": 42, "downloads": "nope", "protocolVersion": []});
        let status = decode_models_status(&wrong).expect("对象仍可解");
        assert!(!status.sdk.huggingface_hub);
        assert!(status.cache_root.is_none());
        assert!(status.downloads.is_empty());
        assert!(status.protocol_version.is_none());
    }

    #[test]
    fn decode_model_cache_entries_caps_and_defaults() {
        let payload = json!({
            "cacheRoot": "/cache",
            "entries": [
                {"repoId": "org/name", "revisions": ["main", "v1"], "sizeBytes": 2150, "lastUsedAt": "2026-11-19T00:00:00Z"},
                {"repoId": "bad", "revisions": 7, "sizeBytes": "x", "lastUsedAt": 9}
            ]
        });
        let listing = decode_model_cache_entries(&payload).expect("合法 payload");
        assert!(listing.available);
        assert_eq!(listing.cache_root.as_deref(), Some("/cache"));
        assert_eq!(listing.entries.len(), 2);
        assert_eq!(listing.entries[0].size_bytes, 2150);
        assert_eq!(listing.entries[0].revisions, vec!["main", "v1"]);
        assert_eq!(
            listing.entries[1].revisions,
            Vec::<String>::new(),
            "坏类型取空"
        );
        assert_eq!(listing.entries[1].size_bytes, 0, "坏类型取 0");
        assert!(decode_model_cache_entries(&json!([])).is_err());
    }

    #[test]
    fn decode_model_repo_info_is_fail_closed() {
        let payload = json!({
            "repoId": "org/name", "revision": "v1", "sizeBytes": 300, "fileCount": 2,
            "files": [{"path": "a.bin", "sizeBytes": 100}, {"path": "b.bin"}],
            "filesTruncated": false, "gated": "auto", "via": "api"
        });
        let info = decode_model_repo_info(&payload).expect("合法 payload");
        assert_eq!(info.size_bytes, Some(300));
        assert_eq!(info.files.len(), 2);
        assert_eq!(info.files[1].size_bytes, None, "未知大小如实 None");
        assert!(!info.gated, "非 bool 的 gated 取安全默认 false");
        assert_eq!(info.via, "api");
        assert!(decode_model_repo_info(&json!(3)).is_err());
    }

    #[test]
    fn decode_download_start_maps_both_branches_and_unknown_shapes() {
        let started = decode_download_start(&json!({"downloadId": "dl-1", "state": "preparing"}));
        assert_eq!(started.state, "preparing");
        assert_eq!(started.download_id.as_deref(), Some("dl-1"));

        let gated = decode_download_start(
            &json!({"needsConfirmation": true, "estimatedBytes": 2147483649i64}),
        );
        assert_eq!(
            gated.state, "needsConfirmation",
            "spec §3.5 预算门分支按 state 呈现"
        );
        assert_eq!(gated.estimated_bytes, Some(2147483649i64));

        let weird = decode_download_start(&json!("nope"));
        assert_eq!(weird.state, "error");
        assert!(weird.reason.is_some());
    }

    #[test]
    fn decode_download_progress_is_fail_closed() {
        let payload = json!({
            "repoId": "org/name", "revision": "main", "state": "downloading",
            "bytesDone": 10, "bytesTotal": 20, "fileDone": 0, "fileCount": 1,
            "currentFile": "a.bin", "lastError": null
        });
        let progress = decode_download_progress(&payload).expect("合法 payload");
        assert_eq!(progress.bytes_done, 10);
        assert_eq!(progress.bytes_total, Some(20));
        assert_eq!(progress.current_file.as_deref(), Some("a.bin"));
        assert!(progress.last_error.is_none());
        assert!(decode_download_progress(&json!(null)).is_err());
    }

    // ---- 编排：冷启动收敛路径（sidecar 未就绪 → unavailable，不 panic） ----

    #[test]
    fn orchestration_converges_when_sidecar_is_not_ready() {
        let manager = cold_manager();
        assert_eq!(
            manager.status().state,
            crate::sidecar::dto::SidecarState::Stopped
        );

        let status = status(&manager);
        assert!(!status.available);
        assert!(status
            .unavailable_reason
            .as_deref()
            .unwrap_or_default()
            .contains("models.status"));

        let listing = cache_list(&manager);
        assert!(!listing.available);
        assert!(listing.entries.is_empty());

        let info = info(&manager, "org/name", "main");
        assert!(!info.available);
        assert!(info.info.is_none());

        let start = download_start(&manager, "org/name", "main", None, false);
        assert_eq!(
            start.state, "unavailable",
            "spec §3.6：download 未就绪 → unavailable"
        );

        let progress = download_progress(&manager, "dl-1");
        assert!(!progress.available);

        let cancel = download_cancel(&manager, "dl-1");
        assert!(!cancel.available);
    }

    #[test]
    fn orchestration_rejects_bad_identifiers_before_touching_the_sidecar() {
        let manager = cold_manager();
        let info = info(&manager, "bad name", "main");
        assert!(info
            .unavailable_reason
            .as_deref()
            .unwrap_or_default()
            .contains("参数非法"));

        let start = download_start(&manager, "org/name", "..", None, false);
        assert_eq!(start.state, "error");
        assert!(start
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("参数非法"));

        let files = vec!["../x".to_string()];
        let start = download_start(&manager, "org/name", "main", Some(&files), false);
        assert_eq!(start.state, "error");
    }

    // ---- 冻结常量 ----

    #[test]
    fn frozen_constants_match_the_spec() {
        assert_eq!(MODELS_PROTOCOL_VERSION, "1.0");
        assert_eq!(
            MODELS_DOWNLOAD_STATES,
            &[
                "idle",
                "preparing",
                "downloading",
                "completed",
                "failed",
                "cancelled"
            ][..]
        );
        assert_eq!(MODELS_MAX_CACHE_ENTRIES, 128);
        assert_eq!(MODELS_INFO_MAX_FILES, 64);
    }
}
