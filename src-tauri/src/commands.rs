//! Tauri IPC 命令层。
//!
//! 这一层是**唯一**允许接触 Tauri GUI 类型的领域入口（另一个是 [`crate::run`]）。
//! 每个命令都应当薄：只做参数整形 + 调用领域模块，业务逻辑留在各自的领域模块里。

use std::sync::Arc;

use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use crate::clock;
use crate::db::DbHealth;
use crate::error::{Error, Result};
use crate::harness::inventory::{installation_id, reconcile_harnesses, ReconcileReport};
use crate::harness::registry::HarnessSummary;
use crate::session::{service::SessionService, SessionRecord, TerminationReason};
use crate::terminal::{Emitter, PtyEvent};
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
///
/// **本命令是纯读**：它不写 `harnesses` / `harness_installations`。
/// 需要把检测结果落库时，请显式调用 [`refresh_harnesses`]
/// （list / get / inspect 不修改状态；refresh / sync 才允许）。
#[tauri::command]
pub fn list_harnesses(state: State<'_, AppState>) -> Vec<HarnessSummary> {
    state.harnesses.summaries()
}

/// 显式同步 Harness 清单：`detect` → `reconcile` → SQLite。
///
/// 语义上是「允许修改状态」的操作，调用点只有：应用启动、用户点 Refresh
/// （以及 `create_session` 的 invariant guard）。见 `harness::inventory`。
#[tauri::command]
pub fn refresh_harnesses(state: State<'_, AppState>) -> Result<ReconcileReport> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    runtime::local::ensure_local_target(database.connection())?;

    reconcile_harnesses(
        database.connection(),
        &state.harnesses.summaries(),
        runtime::local::LOCAL_TARGET_ID,
        &clock::now_rfc3339(),
    )
}

/// Session 列表的默认条数上限。
pub const DEFAULT_SESSION_LIMIT: u32 = 50;

/// 单次查询允许的最大条数，避免前端传入超大 limit 拖垮查询。
pub const MAX_SESSION_LIMIT: u32 = 500;

/// 登记一条 Session（`status = created`）。
///
/// 注意两件事：
///   1. **这里不启动任何进程**，因此状态是 `created` 而不是 `running`
///      （Task 4 起成功 spawn 后才 mark_running）；
///   2. 主同步路径是 [`refresh_harnesses`]；这里的 reconcile 只是
///      **invariant guard**，保证 FK 依赖的 harness 定义与安装行一定存在。
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

    // invariant guard：确保 harnesses 与 harness_installations 都有对应行，
    // 否则 sessions 的外键会直接失败（真实宿主上踩过）。
    runtime::local::ensure_local_target(database.connection())?;
    reconcile_harnesses(
        database.connection(),
        std::slice::from_ref(&summary),
        runtime::local::LOCAL_TARGET_ID,
        &clock::now_rfc3339(),
    )?;

    let installation = installation_id(&harness_id, runtime::local::LOCAL_TARGET_ID);

    // harness_id 与 runtime_target_id 由 installation 推导，
    // 调用方无法构造出矛盾的组合（数据库层另有复合外键兜底）。
    SessionService::new(database.connection()).start_from_installation(
        &installation,
        project_id.as_deref(),
        cwd.as_deref(),
    )
}

/// 结束一条仍处于 `running` 的会话；返回是否真的更新了行（幂等）。
///
/// 必须显式给出 `reason`：**非零退出码不等于同一种失败**。
/// 用户强杀、CLI 参数错误、Agent 工作失败、Harness Hub 自身故障含义完全不同
/// （见 migration 0004）。终态由 reason + exit_code 共同推导。
///
/// 对 `created`（从未启动）的会话会返回 `false` —— 结束一个从未运行的会话
/// 只会造出假历史。
#[tauri::command]
pub fn finish_session(
    state: State<'_, AppState>,
    hub_session_id: String,
    exit_code: Option<i32>,
    reason: TerminationReason,
) -> Result<bool> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    SessionService::new(database.connection()).finish(&hub_session_id, exit_code, reason)
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

/* ------------------------------------------------------------------ *
 * Terminal（Task 4）
 *
 * 公共句柄**只有 hub_session_id**：PID 仅用于诊断，不做权限句柄
 * （PID 会复用，而且「有进程」≠「我们仍拥有这个 PTY」）。
 * 这样 WSL / SSH / Container 未来可以保持同一套 API。
 * ------------------------------------------------------------------ */

/// 启动一次终端会话，返回它的 `hub_session_id`。
///
/// 输出通过 Tauri Channel 流式推送（有序、原始 bytes）。
/// **前端必须先就绪**（xterm 已 open、Channel 回调已挂、onData/resize 已挂）
/// 再调用本命令 —— 否则 Codex 首屏的 DSR 会早于 responder 就绪而卡住。
#[tauri::command]
pub fn start_terminal(
    state: State<'_, AppState>,
    installation_id: String,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
    output: Channel<PtyEvent>,
) -> Result<SessionRecord> {
    let emitter: Emitter = Arc::new(move |event| {
        // Channel 发送失败（前端已卸载）只意味着没人再听，不影响进程本身。
        let _ = output.send(event);
    });

    state
        .terminal
        .start(&installation_id, cwd.as_deref(), cols, rows, Some(emitter))
}

/// 把用户输入（含终端协议响应，例如 xterm.js 对 DSR 的回答）写入 PTY。
#[tauri::command]
pub fn write_terminal(state: State<'_, AppState>, session_id: String, data: Vec<u8>) -> Result<()> {
    state.terminal.write(&session_id, &data)
}

/// 调整 PTY 尺寸。
#[tauri::command]
pub fn resize_terminal(
    state: State<'_, AppState>,
    session_id: String,
    cols: u16,
    rows: u16,
) -> Result<()> {
    state.terminal.resize(&session_id, cols, rows)
}

/// 用户主动结束会话。终态由 reaper 依真实退出写入（见 ADR-0010）。
#[tauri::command]
pub fn kill_terminal(state: State<'_, AppState>, session_id: String) -> Result<()> {
    state.terminal.kill(&session_id)
}
