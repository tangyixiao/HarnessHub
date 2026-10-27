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
use crate::harness::probe::SystemHostProbe;
use crate::harness::registry::HarnessSummary;
use crate::session::{service::SessionService, SessionRecord, TerminationReason};
use crate::terminal::{Emitter, PtyEvent};
use crate::usage::adapter::{merge_known_version, CcusageAdapter, UsageSourceAdapter};
use crate::usage::runner::SystemCommandRunner;
use crate::usage::UsageSource;
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
    state.harnesses.summaries(runtime::local::LOCAL_TARGET_ID)
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
        &state.harnesses.summaries(runtime::local::LOCAL_TARGET_ID),
        runtime::local::LOCAL_TARGET_ID,
        &clock::now_rfc3339(),
    )
}

/// Session 列表的默认条数上限。
pub const DEFAULT_SESSION_LIMIT: u32 = 50;

/// 一次 Usage 刷新的结果：审计行 + 同快照对账。
///
/// 跨 IPC，因此有独立的序列化契约测试。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRefreshReport {
    pub import: crate::usage::UsageImport,
    pub reconciliation: crate::usage::importer::Reconciliation,
}

/// Usage 数据源的只读视图。
///
/// **本命令是纯读**：`detect()` 只看 PATH、`merge_known_version()` 只读 SQLite，
/// 全程**不起任何外部进程**（`npx` 一次要一两秒，放进会被反复调用的只读接口
/// 既慢又会把「读」变成「干活」）。
///
/// `version` 的语义因此是「上一次导入实际看到的版本」，而不是「此刻探测到的版本」；
/// 要真正探测版本请调用 [`refresh_usage`]（它必然要起进程）。
#[tauri::command]
pub fn get_usage_sources(state: State<'_, AppState>) -> Result<Vec<UsageSource>> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    let probe = SystemHostProbe::new();
    let executor = SystemCommandRunner::new();
    let adapter = CcusageAdapter::new(&probe, &executor, None);

    let source = merge_known_version(database.connection(), adapter.detect())?;
    Ok(vec![source])
}

/// 显式刷新 Usage：`ccusage` → 归一化 → 幂等落库。
///
/// 语义上是「允许修改状态」的操作（会起外部进程、会写 `usage_imports` /
/// `usage_events`），因此前端必须由用户显式触发，不得在渲染时自动调用。
/// 失败时数据库里会留下一条 `status = 'failed'` 的审计行，错误同时返回给调用方。
#[tauri::command]
pub fn refresh_usage(state: State<'_, AppState>) -> Result<UsageRefreshReport> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    let probe = SystemHostProbe::new();
    let executor = SystemCommandRunner::new();
    let adapter = CcusageAdapter::new(&probe, &executor, None);

    let outcome = adapter.import(database.connection())?;
    Ok(UsageRefreshReport {
        import: outcome.import,
        reconciliation: outcome.reconciliation,
    })
}

/// 单次查询允许的最大条数，避免前端传入超大 limit 拖垮查询。
pub const MAX_SESSION_LIMIT: u32 = 500;

/// Activity 事件流的默认 / 最大条数（聚合不受它约束）。
pub const DEFAULT_ACTIVITY_LIMIT: u32 = 200;
pub const MAX_ACTIVITY_LIMIT: u32 = 1000;

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
        .summaries(runtime::local::LOCAL_TARGET_ID)
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
    let updated = {
        let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
        SessionService::new(database.connection()).finish(&hub_session_id, exit_code, reason)
    }?;

    // 终态真的写入后才补拍 end 快照（best-effort，后台线程；git 子进程不进数据库锁）。
    if updated {
        crate::git::service::spawn_capture(
            Arc::clone(&state.db),
            &hub_session_id,
            crate::git::SnapshotPhase::End,
        );
    }

    Ok(updated)
}

/// 某个会话的 Git 快照（start / end）。**纯读**：只查 SQLite（ADR-0012）。
///
/// 没有行不代表「没有变化」：cwd 缺失 / 非 Git 仓库 / 拍摄失败都不落行
/// （见 `git::service`）。UI 文案必须遵守 R5：快照只描述
/// 「会话期间的变更」，不得声称代码由 AI 编写。
#[tauri::command]
pub fn session_git_snapshots(
    state: State<'_, AppState>,
    hub_session_id: String,
) -> Result<Vec<crate::git::SessionGitSnapshot>> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    crate::git::GitSnapshotStore::new(database.connection()).list_by_session(&hub_session_id)
}

/// 会话窗口 Git 摘要（ROADMAP A3）：start..end 的 commit 数与 ±行数。
///
/// 纯读 + 按需现算（不落库、不缓存）：短锁读快照输入，**锁外**跑至多两条
/// 有界 git 命令（每条 3 秒上限）。`None` = 没有窗口（缺快照 / unborn HEAD /
/// 仓库不可达 / 同 HEAD 之外形态不认识）—— UI 如实不显示，不猜。
#[tauri::command]
pub fn session_git_summary(
    state: State<'_, AppState>,
    hub_session_id: String,
) -> Result<Option<crate::git::WindowSummary>> {
    let inputs = {
        let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
        crate::git::window::window_inputs(database.connection(), &hub_session_id)?
    };

    Ok(inputs.and_then(|inputs| {
        crate::git::window::window_summary(
            &inputs.worktree_root,
            &inputs.start_head,
            &inputs.end_head,
        )
    }))
}

/* ------------------------------------------------------------------ *
 * 只读会话回放（ROADMAP B6）
 *
 * 能力门控：只对 `replay` capability 为 true 的 Harness 开放（当前只有
 * Claude Code）。**IPC 不接受文件系统路径**：按 session_id 由后端从自己的
 * 发现结果解析（路径白名单，防任意文件读取）。
 * ------------------------------------------------------------------ */

/// 回放数据根：来自 harness 检测的 data_paths（只读发现，ADR-0003）。
fn replay_roots(state: &State<'_, AppState>, harness_id: &str) -> Result<Vec<std::path::PathBuf>> {
    let summary = state
        .harnesses
        .summaries(runtime::local::LOCAL_TARGET_ID)
        .into_iter()
        .find(|summary| summary.id == harness_id)
        .ok_or_else(|| Error::InvalidInput(format!("未注册的 Harness：{harness_id}")))?;

    if !summary.capabilities.replay {
        return Err(Error::InvalidInput(format!(
            "{harness_id} 不支持回放（capability replay = false）"
        )));
    }

    Ok(summary
        .data_paths
        .iter()
        .map(std::path::PathBuf::from)
        .collect())
}

/// 某个 Harness 的可回放会话列表（只读发现 + 有界预读，不落库）。
#[tauri::command]
pub fn replay_list_sessions(
    state: State<'_, AppState>,
    harness_id: String,
) -> Result<Vec<crate::replay::ReplaySessionSummary>> {
    if harness_id != crate::harness::adapters::claude_code::CLAUDE_ID {
        // 其它 harness 尚未实现回放解析：空列表（capability 层也拦着，这里双保险）。
        return Ok(Vec::new());
    }
    Ok(crate::replay::claude::list_sessions(&replay_roots(
        &state,
        &harness_id,
    )?))
}

/// 读取一条回放（按 session_id，路径由后端白名单解析）。
///
/// 没有命中发现结果 → `None`；转录超过大小上限 → 明确报错（不猜）。
#[tauri::command]
pub fn replay_read_session(
    state: State<'_, AppState>,
    harness_id: String,
    session_id: String,
) -> Result<Option<crate::replay::ReplaySession>> {
    if harness_id != crate::harness::adapters::claude_code::CLAUDE_ID {
        return Ok(None);
    }
    crate::replay::claude::read_session_by_id(&replay_roots(&state, &harness_id)?, &session_id)
}

/* ------------------------------------------------------------------ *
 * 回放搜索（ROADMAP B7，migration 0011）
 *
 * 设置与搜索是纯 SQLite 操作；reindex 是显式用户动作（读外部转录、
 * 写索引表），按 ADR-0012 的读写分离纪律不得在渲染路径自动触发。
 * ------------------------------------------------------------------ */

/* ------------------------------------------------------------------ *
 * Worktree（ROADMAP C10）
 *
 * list 纯读；add 是**显式用户动作**（写磁盘），repo_root 必须命中已登记
 * 项目（防任意路径写），目标路径不允许位于仓库内部（git::worktree 红线）。
 * ------------------------------------------------------------------ */

/// 按传入 repoRoot 找已登记项目（兼容 repo_root 的两种存储形态）。
fn require_registered_project(
    state: &State<'_, AppState>,
    repo_root: &str,
) -> Result<crate::project::ProjectRecord> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    let store = crate::project::store::ProjectStore::new(database.connection());
    store
        .get_by_repo_root(repo_root)?
        .or_else(|| {
            // projects.repo_root 由 GitRootProbe 落库（canonicalize 形态）；
            // 前端可能传同一路径的其它表示。
            std::path::Path::new(repo_root)
                .canonicalize()
                .ok()
                .and_then(|canonical| {
                    store
                        .get_by_repo_root(&canonical.to_string_lossy())
                        .unwrap_or(None)
                })
        })
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "未登记的项目根：{repo_root}（先在 Projects 页登记）"
            ))
        })
}

/// 某个已登记项目的全部 worktree。**纯读**。
#[tauri::command]
pub fn worktree_list(
    state: State<'_, AppState>,
    repo_root: String,
) -> Result<Vec<crate::git::WorktreeEntry>> {
    require_registered_project(&state, &repo_root)?;
    crate::git::worktree::list(std::path::Path::new(&repo_root))
}

/// 创建 worktree（显式用户动作；前端必须二次确认「将在磁盘创建目录」）。
#[tauri::command]
pub fn worktree_add(
    state: State<'_, AppState>,
    repo_root: String,
    path: String,
    branch: String,
    create_branch: bool,
) -> Result<crate::git::WorktreeEntry> {
    require_registered_project(&state, &repo_root)?;
    crate::git::worktree::add(
        std::path::Path::new(&repo_root),
        std::path::Path::new(&path),
        &branch,
        create_branch,
    )
}

/// 某 worktree 的未提交修改计数。**纯读**（ROADMAP C11）。
#[tauri::command]
pub fn worktree_status(
    state: State<'_, AppState>,
    repo_root: String,
    worktree_path: String,
) -> Result<crate::git::WorktreeStatusCounts> {
    require_registered_project(&state, &repo_root)?;
    crate::git::worktree::status_counts(
        std::path::Path::new(&repo_root),
        std::path::Path::new(&worktree_path),
    )
}

/// 删除一条**干净**的链接 worktree（显式用户动作；脏树/主树拒绝）。
#[tauri::command]
pub fn worktree_remove(
    state: State<'_, AppState>,
    repo_root: String,
    worktree_path: String,
) -> Result<()> {
    require_registered_project(&state, &repo_root)?;
    crate::git::worktree::remove(
        std::path::Path::new(&repo_root),
        std::path::Path::new(&worktree_path),
    )
}

/// 把分支快进合并到主工作树（显式用户动作；主树脏/非 ff 拒绝）。
#[tauri::command]
pub fn worktree_merge(
    state: State<'_, AppState>,
    repo_root: String,
    branch: String,
) -> Result<crate::git::MergeOutcome> {
    require_registered_project(&state, &repo_root)?;
    crate::git::worktree::merge_ff_only(std::path::Path::new(&repo_root), &branch)
}

/// 回放搜索设置（默认关闭；关闭时后端已清空索引）。
#[tauri::command]
pub fn replay_search_settings_get(
    state: State<'_, AppState>,
) -> Result<crate::replay::index::SearchSettings> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    crate::replay::index::settings(database.connection())
}

/// 更新设置：关闭 = 清空索引；收紧排除名单 = 立即清掉对应索引行。
#[tauri::command]
pub fn replay_search_settings_set(
    state: State<'_, AppState>,
    settings: crate::replay::index::SearchSettings,
) -> Result<()> {
    let mut database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    crate::replay::index::set_settings(database.connection_mut(), &settings)
}

/// Provider 配置现状的只读发现（ROADMAP E14）。
///
/// **纯读**：解析现有配置文件，不写任何外部文件（投影是 E15）。
/// secret 值在 Rust 层 redact（`<redacted>` / `set`/`unset`），契约测试锁定。
#[tauri::command]
pub fn profile_discover(
    _state: State<'_, AppState>,
    harness_id: String,
) -> Result<Option<crate::profile::ProfileSummary>> {
    use crate::harness::probe::HostProbe;
    let probe = crate::harness::probe::SystemHostProbe::new();
    let home = probe
        .home_dir()
        .ok_or_else(|| Error::InvalidInput("无法确定 home 目录".to_string()))?;
    match harness_id.as_str() {
        crate::harness::adapters::claude_code::CLAUDE_ID => crate::profile::claude::discover(&home),
        "codex" => crate::profile::codex::discover(&home),
        "gemini" => crate::profile::gemini::discover(&home),
        other => Err(Error::InvalidInput(format!(
            "{other} 的配置发现尚未实现（当前支持 claude/codex/gemini）"
        ))),
    }
}

/// 配置体检（ROADMAP E17）。**纯读**：只诊断 + 建议，
/// 不自动修复（修复走 E15 投影/恢复，显式动作）。
#[tauri::command]
pub fn config_doctor(
    _state: State<'_, AppState>,
    harness_id: String,
) -> Result<Vec<crate::profile::doctor::CheckItem>> {
    use crate::harness::probe::HostProbe;
    let probe = crate::harness::probe::SystemHostProbe::new();
    let home = probe
        .home_dir()
        .ok_or_else(|| Error::InvalidInput("无法确定 home 目录".to_string()))?;
    crate::profile::doctor::diagnose(&home, &harness_id)
}

/// 显式重建/增量更新索引（用户动作；未启用时报明确错误）。
#[tauri::command]
pub fn replay_search_reindex(
    state: State<'_, AppState>,
) -> Result<crate::replay::index::ReindexReport> {
    let mut database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    let roots = replay_roots(&state, crate::harness::adapters::claude_code::CLAUDE_ID)?;
    crate::replay::index::reindex(database.connection_mut(), &roots)
}

/// 搜索已索引的回放内容（纯读 SQLite；<3 字符报错——trigram 下限）。
#[tauri::command]
pub fn replay_search(
    state: State<'_, AppState>,
    query: String,
) -> Result<Vec<crate::replay::index::SearchHit>> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    crate::replay::index::search(database.connection(), &query)
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
 * Project registry（§20 第 14 步）
 * ------------------------------------------------------------------ */

/// 登记一个本地 Git 仓库目录为项目：自动识别 repo root，按 root 幂等 upsert。
#[tauri::command]
pub fn register_project(
    state: State<'_, AppState>,
    path: String,
    name: Option<String>,
) -> Result<crate::project::ProjectRecord> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    let request = crate::project::RegisterRequest { path, name };
    crate::project::service::ProjectService::new(database.connection(), state.git_probe.clone())
        .register(&request)
}

/// 最近项目，按 last_opened_at / created_at 倒序。
#[tauri::command]
pub fn list_projects(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<crate::project::ProjectRecord>> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    let limit = limit
        .unwrap_or(DEFAULT_SESSION_LIMIT)
        .min(MAX_SESSION_LIMIT);

    crate::project::service::ProjectService::new(database.connection(), state.git_probe.clone())
        .list_recent(limit)
}

/// 删除项目（幂等）。Session 保留但失去归属；项目级设置级联删除。
#[tauri::command]
pub fn remove_project(state: State<'_, AppState>, project_id: String) -> Result<bool> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    crate::project::service::ProjectService::new(database.connection(), state.git_probe.clone())
        .remove(&project_id)
}

/// 项目下的 Session 历史。
#[tauri::command]
pub fn get_project_sessions(
    state: State<'_, AppState>,
    project_id: String,
    limit: Option<u32>,
) -> Result<Vec<SessionRecord>> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    let limit = limit
        .unwrap_or(DEFAULT_SESSION_LIMIT)
        .min(MAX_SESSION_LIMIT);

    crate::project::service::ProjectService::new(database.connection(), state.git_probe.clone())
        .sessions(&project_id, limit)
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

/// 启动并绑定 worktree 的终端会话（ROADMAP C10）。
///
/// `worktreePath` 必须是 `worktree_list(repoRoot)` 返回过的路径（前端从列表选）；
/// 后端不重复校验路径合法性——校验发生在 `worktree_add` / `worktree_list`。
#[tauri::command]
pub fn start_terminal_in_worktree(
    state: State<'_, AppState>,
    installation_id: String,
    cwd: Option<String>,
    worktree_path: String,
    cols: u16,
    rows: u16,
    output: Channel<PtyEvent>,
) -> Result<SessionRecord> {
    let emitter: Emitter = Arc::new(move |event| {
        let _ = output.send(event);
    });

    state.terminal.start_with_worktree(
        &installation_id,
        cwd.as_deref(),
        Some(&worktree_path),
        cols,
        rows,
        Some(emitter),
    )
}

/// 把用户输入（含终端协议响应，例如 xterm.js 对 DSR 的回答）写入 PTY。
#[tauri::command]
pub fn write_terminal(state: State<'_, AppState>, session_id: String, data: Vec<u8>) -> Result<()> {
    state.terminal.write(&session_id, &data)
}

/// 恢复一条外部会话（ROADMAP B9）：`--resume <source_session_id>`，
/// capability `resume` 为 false 的 harness 在适配器层报明确错误。
#[tauri::command]
pub fn resume_terminal(
    state: State<'_, AppState>,
    installation_id: String,
    source_session_id: String,
    cwd: Option<String>,
    cols: u16,
    rows: u16,
    output: Channel<PtyEvent>,
) -> Result<SessionRecord> {
    let emitter: Emitter = Arc::new(move |event| {
        let _ = output.send(event);
    });

    state.terminal.start_resume(
        &installation_id,
        &source_session_id,
        cwd.as_deref(),
        cols,
        rows,
        Some(emitter),
    )
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

/// Dashboard 的聚合查询。**纯读**：只查 SQLite，不起进程、不写库、不碰 ccusage（ADR-0012）。
///
/// `range` 是 `today` / `7d` / `30d` / `all`；`timezone_offset_minutes` 是
/// 「加到 UTC 上得到本地时间的分钟数」（UTC+8 → 480），时间边界由后端据此计算。
/// 参数不合法必须报错，不得退化成「全部时间」。
#[tauri::command]
pub fn usage_summary(
    state: State<'_, AppState>,
    range: String,
    timezone_offset_minutes: i32,
) -> Result<crate::usage::summary::UsageSummary> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    let range = crate::usage::summary::parse_range(&range)?;
    let window = crate::usage::summary::range_bounds(
        database.connection(),
        range,
        &clock::now_rfc3339(),
        timezone_offset_minutes,
    )?;

    crate::usage::summary::summary(database.connection(), &window)
}

/// Activity 页的活动流查询。**纯读**：只查 SQLite，不起进程、不写库（ADR-0012）。
///
/// 与 `usage_summary` 同形：`range` 是 `today` / `7d` / `30d` / `all`，
/// `timezone_offset_minutes` 决定本地日 / 本地小时的归属。`limit` 只截断事件流
/// （`has_more` 告知前端「还有更多」），不截断按天 / 按小时的聚合。
#[tauri::command]
pub fn activity_timeline(
    state: State<'_, AppState>,
    range: String,
    timezone_offset_minutes: i32,
    limit: Option<u32>,
) -> Result<crate::activity::ActivityTimeline> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    let range = crate::usage::summary::parse_range(&range)?;
    let window = crate::usage::summary::range_bounds(
        database.connection(),
        range,
        &clock::now_rfc3339(),
        timezone_offset_minutes,
    )?;

    crate::activity::query::timeline(
        database.connection(),
        &window,
        limit
            .unwrap_or(DEFAULT_ACTIVITY_LIMIT)
            .min(MAX_ACTIVITY_LIMIT),
    )
}

/// 插件能力的**只读快照**，八个布尔与 manifest 的 `[capabilities]` 一一对应。
///
/// 之所以不直接把 `capabilities_json` 原文透给前端：那样 TS 侧就得自己解析一段
/// 未校验的 JSON，而 IPC 契约纪律要求形状在 Rust 侧定型（AGENTS.md）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCapabilities {
    pub detect: bool,
    pub launch: bool,
    pub resume: bool,
    pub usage: bool,
    pub trace: bool,
    pub profiles: bool,
    pub mcp: bool,
    pub skills: bool,
}

/// 一个已缓存的 skill（注册时从插件拉取，v0.1 不热更新）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSkill {
    pub skill_id: String,
    pub name: String,
    pub description: String,
    /// `SKILL.md` 原文。宿主不解析（spec §3），这里同样只透传。
    pub body: String,
}

/// 插件列表项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub path: String,
    pub status: String,
    pub capabilities: PluginCapabilities,
    pub skills: Vec<PluginSkill>,
    pub registered_at: String,
}

/// 已登记插件列表。**纯读**：不扫目录、不起进程（ADR-0012）。
///
/// 要发现新插件或复活 degraded 插件请调用 [`plugin_refresh`]。
#[tauri::command]
pub fn plugin_list(state: State<'_, AppState>) -> Result<Vec<PluginSummary>> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    let records = crate::plugins::registry::list_plugins(database.connection())?;
    Ok(records.into_iter().map(plugin_summary).collect())
}

/// 一次 refresh 的结果，透给调用方作为「刷新已生效」的证据。
///
/// 刻意不用 `Result`：单个插件失败是正常结果（R14），见 `service::RefreshReport`。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRefreshReport {
    pub registered: Vec<String>,
    pub errors: Vec<String>,
}

/// 显式刷新插件：扫描目录 → 登记 → 握手 → 拉取 skill 缓存。
///
/// 语义上是**写操作**（起子进程、写 `plugins` 表），因此只允许用户动作触发，
/// 绝不在渲染时调用——与 [`refresh_usage`] 同一条纪律。
#[tauri::command]
pub fn plugin_refresh(state: State<'_, AppState>) -> Result<PluginRefreshReport> {
    let database = state.db.lock().map_err(|_| Error::StateLockPoisoned)?;
    let report = crate::plugins::service::refresh(database.connection(), &state.plugins_root);
    Ok(PluginRefreshReport {
        registered: report.registered,
        errors: report.errors,
    })
}

/// 注册表行 → IPC 视图。
///
/// 解析失败**降级成空值而不是报错**：一条坏行不该让整个插件列表打不开
/// （`capabilities_json` / `skills_cache_json` 只由本宿主写入，坏掉说明库被外部改过）。
fn plugin_summary(record: crate::plugins::registry::PluginRecord) -> PluginSummary {
    PluginSummary {
        id: record.id,
        name: record.name,
        version: record.version,
        api_version: record.api_version,
        path: record.path,
        status: record.status,
        capabilities: parse_capabilities(&record.capabilities_json),
        skills: parse_skills(record.skills_cache_json.as_deref()),
        registered_at: record.registered_at,
    }
}

fn parse_capabilities(raw: &str) -> PluginCapabilities {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return PluginCapabilities::default();
    };
    let flag = |key: &str| value.get(key).and_then(serde_json::Value::as_bool) == Some(true);
    PluginCapabilities {
        detect: flag("detect"),
        launch: flag("launch"),
        resume: flag("resume"),
        usage: flag("usage"),
        trace: flag("trace"),
        profiles: flag("profiles"),
        mcp: flag("mcp"),
        skills: flag("skills"),
    }
}

/// 缓存形状是 `{"skills":[…]}`（见 `service::fetch_skills`）；`NULL` = 还没拉过。
fn parse_skills(raw: Option<&str>) -> Vec<PluginSkill> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return Vec::new();
    };
    let Some(entries) = value.get("skills").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };

    entries
        .iter()
        .filter_map(|entry| {
            // 缺 skill_id 的条目直接丢弃：没有 id 的 skill 无法被引用。
            let skill_id = entry.get("skill_id")?.as_str()?.to_string();
            let text = |key: &str| {
                entry
                    .get(key)
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            Some(PluginSkill {
                name: text("name"),
                description: text("description"),
                body: text("body"),
                skill_id,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::importer::Reconciliation;
    use crate::usage::{ImportStatus, RunnerKind, UsageImport};
    use std::collections::BTreeMap;
    /// 跨 IPC 契约测试：`refresh_usage` 的返回形状。
    ///
    /// 前端 `src/lib/ipc.ts` 的 `normalizeUsageRefreshReport` 依赖这些键，
    /// 而且它必须能一眼看到「对账是否成立」与「差多少微单位」。
    #[test]
    fn usage_refresh_report_serializes_with_camel_case_keys() {
        let report = UsageRefreshReport {
            import: UsageImport {
                id: "import-1".to_string(),
                source: "ccusage".to_string(),
                source_version: Some("ccusage 20.0.24".to_string()),
                runner: Some(RunnerKind::ManagedNpx),
                report_kind: Some("session".to_string()),
                status: ImportStatus::Succeeded,
                started_at: "2026-09-22T10:00:00Z".to_string(),
                completed_at: Some("2026-09-22T10:00:01Z".to_string()),
                records_seen: 7,
                records_inserted: 7,
                records_updated: 0,
                records_skipped: 0,
                records_timestampless: 1,
                error: None,
            },
            reconciliation: Reconciliation {
                events_total_tokens: 600,
                report_total_tokens: Some(1_510),
                row_total_residual: 910,
                cost_microunits_delta: Some(4),
                unpriced_events: 1,
                timestampless: 1,
                tokens_identity_holds: Some(true),
            },
        };

        let json = serde_json::to_value(&report).expect("序列化");

        assert_eq!(json["import"]["recordsInserted"], 7);
        assert_eq!(json["import"]["recordsTimestampless"], 1);
        assert_eq!(json["import"]["sourceVersion"], "ccusage 20.0.24");
        assert_eq!(json["import"]["runner"], "managed-npx");
        assert_eq!(json["reconciliation"]["eventsTotalTokens"], 600);
        assert_eq!(json["reconciliation"]["reportTotalTokens"], 1_510);
        assert_eq!(json["reconciliation"]["rowTotalResidual"], 910);
        assert_eq!(json["reconciliation"]["costMicrounitsDelta"], 4);
        assert_eq!(json["reconciliation"]["tokensIdentityHolds"], true);
        assert!(json.get("import").is_some() && json.get("reconciliation").is_some());
    }

    /// 跨 IPC 契约测试：`session_git_snapshots` 返回的快照形状（ROADMAP A1）。
    ///
    /// 前端 `normalizeSessionGitSnapshot`（src/lib/ipc.ts）按这些键取值：
    /// 结构体 camelCase，phase 枚举值 snake_case（"start" / "end"）。
    #[test]
    fn session_git_snapshot_serializes_with_camel_case_keys() {
        let snapshot = crate::git::SessionGitSnapshot {
            hub_session_id: "hub-1".to_string(),
            phase: crate::git::SnapshotPhase::Start,
            captured_at: "2026-10-24T10:00:01Z".to_string(),
            worktree_root: "D:/work/demo".to_string(),
            head: Some("a".repeat(40)),
            staged_files: 1,
            unstaged_files: 2,
            untracked_files: 3,
            conflicted_files: 0,
        };

        let json = serde_json::to_value(&snapshot).expect("序列化");

        assert_eq!(json["hubSessionId"], "hub-1");
        assert_eq!(json["phase"], "start");
        assert_eq!(json["capturedAt"], "2026-10-24T10:00:01Z");
        assert_eq!(json["worktreeRoot"], "D:/work/demo");
        assert_eq!(json["head"], "a".repeat(40));
        assert_eq!(json["stagedFiles"], 1);
        assert_eq!(json["unstagedFiles"], 2);
        assert_eq!(json["untrackedFiles"], 3);
        assert_eq!(json["conflictedFiles"], 0);
        for snake in [
            "hub_session_id",
            "captured_at",
            "worktree_root",
            "staged_files",
            "unstaged_files",
            "untracked_files",
            "conflicted_files",
        ] {
            assert!(
                json.get(snake).is_none(),
                "不得输出 snake_case 键 {snake}：{json}"
            );
        }

        // unborn HEAD（空仓库）必须是 null，而不是缺失或空串。
        let unborn = crate::git::SessionGitSnapshot {
            phase: crate::git::SnapshotPhase::End,
            head: None,
            ..snapshot
        };
        let unborn_json = serde_json::to_value(&unborn).expect("序列化");
        assert!(unborn_json["head"].is_null(), "实际：{unborn_json}");
        assert_eq!(unborn_json["phase"], "end");
    }

    /// 跨 IPC 契约测试：回放 DTO（ROADMAP B6）的键名形状。
    ///
    /// 结构体 camelCase；role 枚举值 snake_case；skipped/unsupported 计数是
    /// 字符串→数字的对象。前端 normalizeReplay*（src/lib/ipc.ts）按这些键取值。
    /// B8 起含 toolCalls / subagentThreads 嵌套形状。
    #[test]
    fn replay_dtos_serialize_with_camel_case_keys() {
        let message = crate::replay::ReplayMessage {
            seq: 3,
            role: crate::replay::ReplayRole::Assistant,
            sidechain: true,
            timestamp: Some("2026-10-25T10:00:05.000Z".to_string()),
            text_parts: vec!["回答".to_string()],
            thinking_parts: vec!["思考".to_string()],
            model: Some("claude-sonnet-4-6".to_string()),
            tool_calls: vec![crate::replay::ReplayToolCall {
                id: "call_1".to_string(),
                name: "Grep".to_string(),
                input_json: "{\"pattern\":\"TODO\"}".to_string(),
                result_json: Some("src/a.rs".to_string()),
                result_truncated: false,
                input_truncated: false,
            }],
            unsupported_content_kinds: BTreeMap::new(),
        };
        let json = serde_json::to_value(&message).expect("序列化消息");
        assert_eq!(json["seq"], 3);
        assert_eq!(json["role"], "assistant");
        assert_eq!(json["sidechain"], true);
        assert_eq!(json["textParts"], serde_json::json!(["回答"]));
        assert_eq!(json["thinkingParts"], serde_json::json!(["思考"]));
        assert_eq!(json["toolCalls"][0]["id"], "call_1");
        assert_eq!(json["toolCalls"][0]["name"], "Grep");
        assert_eq!(json["toolCalls"][0]["inputJson"], "{\"pattern\":\"TODO\"}");
        assert_eq!(json["toolCalls"][0]["resultJson"], "src/a.rs");
        assert_eq!(json["toolCalls"][0]["inputTruncated"], false);
        for snake in [
            "text_parts",
            "thinking_parts",
            "unsupported_content_kinds",
            "tool_calls",
        ] {
            assert!(json.get(snake).is_none(), "不得输出 snake_case 键：{json}");
        }

        let session = crate::replay::ReplaySession {
            harness_id: "claude".to_string(),
            session_id: "s1".to_string(),
            source_path: "D:/x.jsonl".to_string(),
            messages: vec![message],
            subagent_threads: vec![crate::replay::SubagentThread {
                message_seqs: vec![3, 4],
                started_at: Some("2026-10-25T10:00:05.000Z".to_string()),
                last_at: Some("2026-10-25T10:00:09.000Z".to_string()),
            }],
            skipped_kind_counts: [("summary".to_string(), 1u64)].into_iter().collect(),
            malformed_lines: 1,
        };
        let json = serde_json::to_value(&session).expect("序列化会话");
        assert_eq!(json["harnessId"], "claude");
        assert_eq!(json["skippedKindCounts"], serde_json::json!({"summary": 1}));
        assert_eq!(json["malformedLines"], 1);
        assert_eq!(json["messages"][0]["role"], "assistant");
        assert_eq!(
            json["subagentThreads"][0]["messageSeqs"],
            serde_json::json!([3, 4])
        );
        assert_eq!(
            json["subagentThreads"][0]["startedAt"],
            "2026-10-25T10:00:05.000Z"
        );
        assert!(json.get("subagent_threads").is_none());

        let summary = crate::replay::ReplaySessionSummary {
            harness_id: "claude".to_string(),
            session_id: "s1".to_string(),
            project_dir: "D--work-demo".to_string(),
            size_bytes: 1024,
            modified_ms: 1_761_000_000_000,
            first_user_preview: Some("预览".to_string()),
            started_at: Some("2026-10-25T10:00:00.000Z".to_string()),
            last_at: Some("2026-10-25T10:01:00.000Z".to_string()),
        };
        let json = serde_json::to_value(&summary).expect("序列化摘要");
        assert_eq!(json["sessionId"], "s1");
        assert_eq!(json["projectDir"], "D--work-demo");
        assert_eq!(json["sizeBytes"], 1024);
        assert_eq!(json["modifiedMs"], 1_761_000_000_000i64);
        assert_eq!(json["firstUserPreview"], "预览");
        assert_eq!(json["startedAt"], "2026-10-25T10:00:00.000Z");
    }

    /// 跨 IPC 契约测试：WorktreeEntry（ROADMAP C10）的键名形状。
    #[test]
    fn worktree_entry_serializes_with_camel_case_keys() {
        let entry = crate::git::WorktreeEntry {
            path: "D:/wt/feature".to_string(),
            head: Some("a".repeat(40)),
            branch: Some("feature".to_string()),
            main: false,
            detached: false,
            bare: false,
        };

        let json = serde_json::to_value(&entry).expect("序列化");
        assert_eq!(json["path"], "D:/wt/feature");
        assert_eq!(json["branch"], "feature");
        assert_eq!(json["main"], false);
        assert_eq!(json["detached"], false);
        assert_eq!(json["bare"], false);
        assert!(json["head"].is_string());

        // detached：branch 为 null 而不是缺失。
        let detached = crate::git::WorktreeEntry {
            branch: None,
            head: None,
            detached: true,
            ..entry
        };
        let json = serde_json::to_value(&detached).expect("序列化");
        assert!(json["branch"].is_null());
        assert!(json["head"].is_null());
        assert_eq!(json["detached"], true);
    }

    /// 跨 IPC 契约测试：ProfileSummary（E14）—— camelCase、嵌套字段 map、
    /// secret redact 已在 Rust 层完成（此测试锁定序列化不再引入明文）。
    #[test]
    fn profile_summary_serializes_with_camel_case_and_redaction() {
        let summary = crate::profile::ProfileSummary {
            harness_id: "claude".to_string(),
            source_path: "C:/Users/dev/.claude/settings.json".to_string(),
            fields: [
                (
                    "env.ANTHROPIC_AUTH_TOKEN".to_string(),
                    "<redacted>".to_string(),
                ),
                (
                    "env.ANTHROPIC_BASE_URL".to_string(),
                    "https://api.example.com".to_string(),
                ),
                ("env.ANTHROPIC_MODEL".to_string(), "test-model".to_string()),
            ]
            .into_iter()
            .collect(),
            parsed_at: "2026-10-25T10:00:00Z".to_string(),
        };

        let json = serde_json::to_value(&summary).expect("序列化");
        assert_eq!(json["harnessId"], "claude");
        assert_eq!(json["sourcePath"], "C:/Users/dev/.claude/settings.json");
        assert_eq!(json["parsedAt"], "2026-10-25T10:00:00Z");
        assert_eq!(json["fields"]["env.ANTHROPIC_AUTH_TOKEN"], "<redacted>");
        assert_eq!(
            json["fields"]["env.ANTHROPIC_BASE_URL"],
            "https://api.example.com"
        );
        for snake in ["harness_id", "source_path", "parsed_at"] {
            assert!(json.get(snake).is_none(), "不得输出 snake_case 键 {snake}");
        }
    }

    /// 跨 IPC 契约测试：C11 的 status/merge DTO 键名形状。
    #[test]
    fn worktree_c11_dtos_serialize_with_camel_case_keys() {
        let counts = crate::git::WorktreeStatusCounts {
            staged_files: 1,
            unstaged_files: 2,
            untracked_files: 3,
            conflicted_files: 0,
        };
        let json = serde_json::to_value(counts).expect("序列化");
        assert_eq!(json["stagedFiles"], 1);
        assert_eq!(json["unstagedFiles"], 2);
        assert_eq!(json["untrackedFiles"], 3);
        assert_eq!(json["conflictedFiles"], 0);

        let outcome = crate::git::MergeOutcome {
            previous_head: Some("a".repeat(40)),
            new_head: Some("b".repeat(40)),
        };
        let json = serde_json::to_value(&outcome).expect("序列化");
        assert_eq!(json["previousHead"], "a".repeat(40));
        assert_eq!(json["newHead"], "b".repeat(40));
        for snake in [
            "previous_head",
            "new_head",
            "staged_files",
            "untracked_files",
        ] {
            assert!(
                serde_json::to_value(counts).unwrap().get(snake).is_none()
                    && json.get(snake).is_none(),
                "不得输出 snake_case 键 {snake}"
            );
        }
    }

    /// 跨 IPC 契约测试：回放搜索 DTO（ROADMAP B7）的键名形状。
    #[test]
    fn replay_search_dtos_serialize_with_camel_case_keys() {
        let settings = crate::replay::index::SearchSettings {
            enabled: true,
            excluded_dirs: vec!["D--work-beta".to_string()],
        };
        // 输入方向（TS → Rust）也要能按 camelCase 反序列化。
        let roundtrip: crate::replay::index::SearchSettings =
            serde_json::from_value(serde_json::to_value(&settings).unwrap()).unwrap();
        assert_eq!(roundtrip, settings);

        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["enabled"], true);
        assert_eq!(json["excludedDirs"], serde_json::json!(["D--work-beta"]));

        let report = crate::replay::index::ReindexReport {
            scanned: 68,
            indexed: 7,
            unchanged: 60,
            excluded: 1,
            removed: 0,
        };
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["scanned"], 68);
        assert_eq!(json["indexed"], 7);
        assert_eq!(json["unchanged"], 60);
        assert_eq!(json["excluded"], 1);
        assert_eq!(json["removed"], 0);

        let hit = crate::replay::index::SearchHit {
            harness_id: "claude".to_string(),
            session_id: "s1".to_string(),
            seq: 3,
            role: "assistant".to_string(),
            snippet: "…«迁移»逻辑…".to_string(),
            first_user_preview: None,
        };
        let json = serde_json::to_value(&hit).unwrap();
        assert_eq!(json["sessionId"], "s1");
        assert_eq!(json["role"], "assistant");
        assert!(json["snippet"].as_str().unwrap().contains('«'));
        assert!(json["firstUserPreview"].is_null());
    }

    /// 跨 IPC 契约测试：`session_git_summary` 的返回形状（ROADMAP A3）。
    ///
    /// `Some` 时三个 camelCase 计数；`None`（没有窗口）必须是 JSON `null`——
    /// 前端据此区分「0 commits」与「没有摘要」。
    #[test]
    fn window_summary_serializes_with_camel_case_keys() {
        let summary = crate::git::WindowSummary {
            commits: 2,
            insertions: 41,
            deletions: 7,
        };

        let json = serde_json::to_value(summary).expect("序列化");
        assert_eq!(json["commits"], 2);
        assert_eq!(json["insertions"], 41);
        assert_eq!(json["deletions"], 7);
        for snake in ["commit_count", "insertions_total"] {
            assert!(json.get(snake).is_none(), "不得输出 snake_case 键：{json}");
        }

        let none =
            serde_json::to_value(Option::<crate::git::WindowSummary>::None).expect("序列化 None");
        assert!(none.is_null(), "None 必须是 null：{none}");
    }

    /// 跨 IPC 契约测试：`register_project` / `list_projects` 的返回形状。
    ///
    /// 前端 `normalizeProjectRecord`（src/lib/ipc.ts）按这些键取值。
    #[test]
    fn project_record_serializes_with_camel_case_keys() {
        let record = crate::project::ProjectRecord {
            id: "proj-1".to_string(),
            name: "demo".to_string(),
            repo_root: "D:/work/demo".to_string(),
            created_at: "2026-10-12T00:00:00Z".to_string(),
            updated_at: "2026-10-12T00:00:01Z".to_string(),
            last_opened_at: Some("2026-10-12T00:00:01Z".to_string()),
        };

        let json = serde_json::to_value(&record).expect("序列化");

        assert_eq!(json.as_object().expect("对象").len(), 6, "恰好 6 个键");
        for key in crate::project::ProjectRecord::keys() {
            assert!(json.get(key).is_some(), "缺键：{key}");
        }
        assert_eq!(json["id"], "proj-1");
        assert_eq!(json["repoRoot"], "D:/work/demo");
        assert_eq!(json["createdAt"], "2026-10-12T00:00:00Z");
        assert_eq!(json["lastOpenedAt"], "2026-10-12T00:00:01Z");
    }

    /// 跨 IPC 契约测试：`register_project` 的入参形状。
    #[test]
    fn register_request_deserializes_with_camel_case_keys() {
        let request: crate::project::RegisterRequest = serde_json::from_value(serde_json::json!({
            "path": "D:/work/demo",
            "name": "工作仓库"
        }))
        .expect("反序列化");

        assert_eq!(request.path, "D:/work/demo");
        assert_eq!(request.name.as_deref(), Some("工作仓库"));

        let minimal: crate::project::RegisterRequest =
            serde_json::from_value(serde_json::json!({ "path": "D:/work/demo" }))
                .expect("name 可省");
        assert_eq!(minimal.name, None);
    }

    /// 跨 IPC 契约测试：`activity_timeline` 的返回形状。
    ///
    /// 前端 `normalizeActivityTimeline`（src/lib/ipc.ts）按这些键取值；
    /// 顶层键名、`events` / `days` / `hours` 三张表的键名都要锁死。
    #[test]
    fn activity_timeline_serializes_with_camel_case_keys() {
        let timeline = crate::activity::ActivityTimeline {
            range: crate::usage::summary::UsageRangeWindow {
                kind: crate::usage::summary::UsageRange::Today,
                start_utc: Some("2026-09-22T16:00:00Z".to_string()),
                end_utc: Some("2026-09-23T16:00:00Z".to_string()),
                timezone_offset_minutes: 480,
                now_utc: "2026-09-22T23:30:00Z".to_string(),
            },
            events: vec![crate::activity::ActivityEvent {
                kind: crate::activity::ActivityEventKind::Usage,
                occurred_at: "2026-09-22T23:30:00Z".to_string(),
                day: "2026-09-23".to_string(),
                hour: 7,
                harness_id: "codex".to_string(),
                project_id: None,
                source_session_id: Some("s-1".to_string()),
                model: Some("gpt-5.6-sol".to_string()),
                total_tokens: 32,
                cost_microunits: Some(12_340_000),
                cost_is_lower_bound: false,
                status: None,
                launch_mode: None,
                duration_ms: None,
                cwd: None,
                hub_session_id: None,
            }],
            days: vec![crate::activity::ActivityDayBucket {
                day: "2026-09-23".to_string(),
                usage_event_count: 1,
                managed_session_count: 0,
                total_tokens: 32,
                cost_microunits: Some(12_340_000),
                cost_is_lower_bound: false,
            }],
            hours: vec![crate::activity::ActivityHourBucket {
                hour: 7,
                usage_event_count: 1,
                managed_session_count: 0,
                total_tokens: 32,
            }],
            usage_event_count: 1,
            managed_session_count: 0,
            active_days: 1,
            total_tokens: 32,
            max_day_tokens: 32,
            max_hour_tokens: 32,
            has_more: false,
        };

        let json = serde_json::to_value(&timeline).expect("序列化");
        let object = json.as_object().expect("对象");

        assert_eq!(
            object.len(),
            crate::activity::ActivityTimeline::keys().len(),
            "顶层键必须恰好是 keys() 列出的那些"
        );
        for key in crate::activity::ActivityTimeline::keys() {
            assert!(object.get(*key).is_some(), "缺键：{key}");
        }

        assert_eq!(json["range"]["kind"], "today");
        assert_eq!(json["range"]["timezoneOffsetMinutes"], 480);
        assert_eq!(json["range"]["startUtc"], "2026-09-22T16:00:00Z");
        assert_eq!(json["events"][0]["kind"], "usage");
        assert_eq!(json["events"][0]["occurredAt"], "2026-09-22T23:30:00Z");
        assert_eq!(json["events"][0]["day"], "2026-09-23");
        assert_eq!(json["events"][0]["hour"], 7);
        assert_eq!(json["events"][0]["totalTokens"], 32);
        assert_eq!(json["events"][0]["costIsLowerBound"], false);
        assert_eq!(json["days"][0]["usageEventCount"], 1);
        assert_eq!(json["hours"][0]["managedSessionCount"], 0);
        assert_eq!(json["hasMore"], false);

        // 不得出现 snake_case 孪生键（AGENTS.md 跨 IPC 契约纪律）。
        for leaked in [
            "usage_event_count",
            "managed_session_count",
            "max_day_tokens",
            "cost_is_lower_bound",
        ] {
            assert!(json.get(leaked).is_none(), "顶层泄漏：{leaked}");
            let event = json["events"][0].as_object().expect("events 对象");
            assert!(event.get(leaked).is_none(), "events 泄漏：{leaked}");
            let day = json["days"][0].as_object().expect("days 对象");
            assert!(day.get(leaked).is_none(), "days 泄漏：{leaked}");
        }
    }

    /// 跨 IPC 契约测试：`plugin_list` 的返回形状。
    ///
    /// 前端 `src/lib/ipc.ts` 的 `normalizePlugin` 依赖这些键名；插件列表一旦
    /// 泄漏 snake_case，UI 会静默渲染空列表而不是报错——这正是 AGENTS.md 里
    /// 记录过两次的同类事故，所以在这里逐字锁死。
    #[test]
    fn plugin_list_serializes_summary_with_camel_case_keys() {
        let summary = PluginSummary {
            id: "usage-connect".to_string(),
            name: "Usage Connect".to_string(),
            version: "0.1.0".to_string(),
            api_version: "1".to_string(),
            path: "/plugins/usage-connect".to_string(),
            status: "ready".to_string(),
            capabilities: PluginCapabilities {
                usage: true,
                skills: true,
                ..Default::default()
            },
            skills: vec![PluginSkill {
                skill_id: "demo".to_string(),
                name: "Demo".to_string(),
                description: "演示".to_string(),
                body: "---\nname: demo\n---\n".to_string(),
            }],
            registered_at: "2026-10-17T00:00:00Z".to_string(),
        };

        let json = serde_json::to_value(&summary).expect("序列化");

        assert_eq!(json["id"], "usage-connect");
        assert_eq!(json["apiVersion"], "1");
        assert_eq!(json["registeredAt"], "2026-10-17T00:00:00Z");
        assert_eq!(json["capabilities"]["usage"], true);
        assert_eq!(json["capabilities"]["trace"], false);
        assert_eq!(json["skills"][0]["skillId"], "demo");
        assert_eq!(json["skills"][0]["body"], "---\nname: demo\n---\n");

        // capabilities 必须八个键全在：消费者不该去猜缺键语义。
        let capabilities = json["capabilities"].as_object().expect("capabilities 对象");
        assert_eq!(capabilities.len(), 8);

        for leaked in ["api_version", "registered_at", "skill_id"] {
            assert!(json.get(leaked).is_none(), "顶层泄漏：{leaked}");
            let skill = json["skills"][0].as_object().expect("skills 对象");
            assert!(skill.get(leaked).is_none(), "skills 泄漏：{leaked}");
        }
    }
}
