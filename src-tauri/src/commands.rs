//! Tauri IPC 命令层。
//!
//! 这一层是**唯一**允许接触 Tauri GUI 类型的领域入口（另一个是 [`crate::run`]）。
//! 每个命令都应当薄：只做参数整形 + 调用领域模块，业务逻辑留在各自的领域模块里。

use serde::Serialize;
use tauri::State;

use crate::db::DbHealth;
use crate::error::{Error, Result};
use crate::AppState;

/// 前端 Dashboard 顶部展示的应用信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    pub name: String,
    pub version: String,
    pub tauri_version: String,
}

/// 应用与运行时版本。
#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo {
        name: "Harness Hub".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        tauri_version: tauri::VERSION.to_string(),
    }
}

/// 数据库健康快照：schema 版本、已应用迁移、表数量与 pragma 基线。
///
/// 这是 Walking Skeleton 的「链路已通」证据：前端能拿到它，
/// 就说明 React → IPC → Rust → SQLite 全程可用。
#[tauri::command]
pub fn db_health(state: State<'_, AppState>) -> Result<DbHealth> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    database.health()
}
