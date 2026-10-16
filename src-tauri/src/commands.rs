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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::importer::Reconciliation;
    use crate::usage::{ImportStatus, RunnerKind, UsageImport};

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
}
