//! Tauri IPC 命令层。
//!
//! 这一层是**唯一**允许接触 Tauri GUI 类型的领域入口（另一个是 [`crate::run`]）。
//! 每个命令都应当薄：只做参数整形 + 调用领域模块，业务逻辑留在各自的领域模块里。

use serde::Serialize;
use tauri::State;

use crate::clock;
use crate::db::DbHealth;
use crate::error::{Error, Result};
use crate::harness::registry::HarnessSummary;
use crate::harness::upsert_harness;
use crate::session::{service::SessionService, SessionRecord};
use crate::{runtime, AppState};

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

/// 已注册 Harness 的**真实**检测结果与能力矩阵。
///
/// 刻意返回 `Vec` 而不是 `Result`：单个 Harness 检测失败必须表现为
/// 「该行 `installed: false`」，而不是让整个页面拿不到数据。
/// 这是 Walking Skeleton 里 UI 唯一被允许获取 Harness 信息的入口 ——
/// 前端不做任何自己的二进制探测。
#[tauri::command]
pub fn list_harnesses(state: State<'_, AppState>) -> Vec<HarnessSummary> {
    state.harnesses.summaries()
}

/// Session 列表的默认条数上限。
pub const DEFAULT_SESSION_LIMIT: u32 = 50;

/// 单次查询允许的最大条数，避免前端传入超大 limit 拖垮查询。
pub const MAX_SESSION_LIMIT: u32 = 500;

/// 新建一条 Session 记录（`status = running`）。
///
/// 注意：**这里不启动任何进程**。PTY 启动是 Task 4 的事；本命令只负责
/// 「统一标识 + 运行目标绑定 + 落库」这段编排，Task 4 会在启动进程前后复用它。
///
/// 落库前必须先把 harness 行写进 `harnesses`：`sessions.harness_id` 有外键，
/// 而检测结果平时只存在于内存注册表中（真实宿主上曾因此直接 FK 失败）。
#[tauri::command]
pub fn create_session(
    state: State<'_, AppState>,
    harness_id: String,
    project_id: Option<String>,
    cwd: Option<String>,
) -> Result<SessionRecord> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;

    let summary = state
        .harnesses
        .summaries()
        .into_iter()
        .find(|summary| summary.id == harness_id)
        .ok_or_else(|| Error::InvalidInput(format!("未注册的 Harness：{harness_id}")))?;

    upsert_harness(database.connection(), &summary, &clock::now_rfc3339())?;
    runtime::local::ensure_local_target(database.connection())?;

    SessionService::new(database.connection()).start(
        &harness_id,
        project_id.as_deref(),
        cwd.as_deref(),
    )
}

/// 结束一条仍处于 `running` 的会话；返回是否真的更新了行（幂等）。
#[tauri::command]
pub fn finish_session(
    state: State<'_, AppState>,
    hub_session_id: String,
    exit_code: Option<i32>,
) -> Result<bool> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    SessionService::new(database.connection()).finish(&hub_session_id, exit_code)
}

/// 最近会话，按开始时间倒序。
#[tauri::command]
pub fn list_sessions(state: State<'_, AppState>, limit: Option<u32>) -> Result<Vec<SessionRecord>> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    let limit = limit
        .unwrap_or(DEFAULT_SESSION_LIMIT)
        .min(MAX_SESSION_LIMIT);

    SessionService::new(database.connection()).list_recent(limit)
}
