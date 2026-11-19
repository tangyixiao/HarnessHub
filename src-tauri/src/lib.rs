//! Harness Hub —— Rust Control Plane。
//!
//! 分层约束（见 docs/CONTEXT-MAP.md）：
//! - 领域模块（`db` / `harness` / `usage` / `session` / …）不得依赖 Tauri GUI 类型；
//! - 只有 `commands` 与 [`run`] 直接接触 Tauri；
//! - SQLite 由本 crate 独占持有，Python sidecar 不得直连。

pub mod activity;
pub mod clock;
pub mod commands;
pub mod credentials;
pub mod db;
pub mod error;
pub mod eval;
pub mod git;
pub mod harness;
pub mod hhar;
pub mod mcp;
pub mod permissions;
pub mod plugins;
pub mod process;
pub mod profile;
pub mod project;
pub mod prompts;
pub mod pty;
pub mod replay;
pub mod runtime;
pub mod session;
pub mod sidecar;
pub mod skills;
pub mod terminal;
pub mod trace;
pub mod tray_menu;
pub mod usage;
pub mod watcher;

#[cfg(test)]
pub(crate) mod test_support;

use std::sync::{Arc, Mutex};

use tauri::Manager;

use crate::db::Database;
use crate::harness::adapters::claude_code::ClaudeCodeAdapter;
use crate::harness::adapters::codebuddy::CodeBuddyAdapter;
use crate::harness::adapters::codex::CodexAdapter;
use crate::harness::adapters::copilot::CopilotAdapter;
use crate::harness::adapters::gemini_cli::GeminiCliAdapter;
use crate::harness::adapters::opencode::{OpenCodeAdapter, OpenCodeXdgRoots};
use crate::harness::inventory::reconcile_harnesses;
use crate::harness::probe::SystemHostProbe;
use crate::harness::registry::HarnessRegistry;
use crate::pty::PortablePtyBackend;
use crate::runtime::local::{ensure_local_target, LOCAL_TARGET_ID};
use crate::session::{service::SessionService, TerminationReason};
use crate::sidecar::config::SidecarConfig;
use crate::sidecar::manager::SidecarManager;
use crate::terminal::TerminalRuntime;

/// 应用级共享状态。
///
/// - `db`：数据库句柄。用 `Arc` 是因为 [`TerminalRuntime`] 的 reaper 回调
///   也需要写终态（终态事实由进程退出驱动，不是由命令驱动）。
/// - `harnesses`：已注册的 Harness 适配器，启动后只读。
/// - `terminal`：终端运行编排（PTY + 会话状态）。
/// - `git_probe`：Project registry 的 repo root 探测（组合根注入，见
///   `project::git_root`）。
/// - `plugins_root`：插件扫描根。在组合根解析一次（环境变量 `HARNESS_HUB_PLUGINS_DIR`
///   或 `<app_data_dir>/plugins`），命令层不再读环境——否则单测会依赖真实环境。
/// - `sidecar`：Python runtime 的生命周期管理器（F20）。**懒启动**：构造时不拉起
///   进程，Python 缺失也只让这一张状态卡片降级，PTY / Git / Session 不受影响。
pub struct AppState {
    pub db: Arc<Mutex<Database>>,
    pub harnesses: Arc<HarnessRegistry>,
    pub terminal: TerminalRuntime,
    pub git_probe: Arc<dyn crate::project::git_root::GitRootProbe>,
    pub plugins_root: std::path::PathBuf,
    pub sidecar: Arc<SidecarManager>,
}

/// 数据库文件名。放在 Tauri 的 app data 目录下，保持 local-first。
pub const DATABASE_FILE_NAME: &str = "harness-hub.sqlite3";

/// 装配 Harness 注册表。
///
/// 这是**唯一的组合根**：Core 其它地方不得认识具体 Harness（ADR-0002）。
/// 注册顺序即 UI 展示顺序（`HarnessRegistry` 用 `Vec` 持有适配器，顺序确定，
/// 不依赖 HashMap 迭代顺序）。
///
/// 只注册**已实现适配器**的 Harness：Codex 与 Claude Code 提供
/// detect / launch / terminal / usage；Gemini CLI、CodeBuddy、OpenCode 与 GitHub Copilot CLI 仅检测，
/// 能力矩阵为全 false。
///
/// `pub` 是为了让集成测试用**生产组合**（而不是测试里重造一份注册表）验证
/// 「两个 Harness 同时 reconcile」这类事实。
pub fn build_harness_registry() -> HarnessRegistry {
    let mut registry = HarnessRegistry::new();
    registry.register(Box::new(CodexAdapter::new(
        Arc::new(SystemHostProbe::new()),
    )));
    registry.register(Box::new(ClaudeCodeAdapter::new(Arc::new(
        SystemHostProbe::new(),
    ))));
    registry.register(Box::new(GeminiCliAdapter::new(
        Arc::new(SystemHostProbe::new()),
        std::env::var_os("GEMINI_CLI_HOME")
            .filter(|root| !root.is_empty())
            .map(std::path::PathBuf::from),
    )));
    registry.register(Box::new(CodeBuddyAdapter::new(Arc::new(
        SystemHostProbe::new(),
    ))));
    // OpenCode 的四个 XDG 根同样只在组合根读一次环境：适配器本身不读环境，
    // 单测因此不依赖真实机器状态。
    registry.register(Box::new(OpenCodeAdapter::new(
        Arc::new(SystemHostProbe::new()),
        OpenCodeXdgRoots::from_env(),
    )));
    registry.register(Box::new(CopilotAdapter::new(Arc::new(
        SystemHostProbe::new(),
    ))));
    registry
}

/// 托盘 id（`tray_by_id` 用它做重聚焦时的菜单重建）。
const TRAY_ID: &str = "main-tray";

/// 菜单项 id 前缀约定：`tray-<action>`（与 [`tray_menu::TrayAction`] 对应）。
const TRAY_OPEN_ID: &str = "tray-open-dashboard";
const TRAY_QUIT_ID: &str = "tray-quit";

/// running 会话计数（托盘「活动会话」只读项的数据源）。
fn active_session_count(state: &AppState) -> u32 {
    let Ok(database) = state.db.lock() else {
        return 0;
    };
    database
        .connection()
        .query_row(
            "SELECT count(*) FROM sessions WHERE status = 'running'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count as u32)
        .unwrap_or(0)
}

/// 纯模型 → Tauri 菜单（薄接线；模型与文案在 tray_menu，可单测）。
fn build_tray_menu<M: tauri::Manager<tauri::Wry>>(
    manager: &M,
    active_sessions: u32,
) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let mut builder = tauri::menu::MenuBuilder::new(manager);
    for (index, item) in crate::tray_menu::build_menu(active_sessions)
        .iter()
        .enumerate()
    {
        let id = match item.action {
            Some(crate::tray_menu::TrayAction::OpenDashboard) => TRAY_OPEN_ID,
            Some(crate::tray_menu::TrayAction::Quit) => TRAY_QUIT_ID,
            None => "tray-sessions",
        };
        builder = builder.item(&tauri::menu::MenuItem::with_id(
            manager,
            id,
            &item.label,
            !item.disabled,
            None::<&str>,
        )?);
        if index + 1 < 3 {
            builder = builder.item(&tauri::menu::PredefinedMenuItem::separator(manager)?);
        }
    }
    builder.build()
}

/// 构建托盘。失败**不阻断主功能**（headless / 无系统托盘环境下降级为没有托盘）。
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let count = active_session_count(&app.state::<AppState>());
    let menu = build_tray_menu(app.handle(), count)?;
    let mut builder = tauri::tray::TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TRAY_OPEN_ID => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            TRAY_QUIT_ID => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}

/// 应用标识（F22 S3：headless MCP server 模式的数据目录归属）。
///
/// 必须与 `tauri.conf.json` 的 `identifier` 一致 —— 由下方的契约测试锁死，
/// 防止两处各自漂移（spec §4.1）。
pub const APP_IDENTIFIER: &str = "dev.harnesshub.desktop";

/// `HARNESS_HUB_DB_PATH` 环境变量：显式覆盖主库路径（集成测试 / 嵌入式调用用）。
const DB_PATH_ENV: &str = "HARNESS_HUB_DB_PATH";

/// 解析 headless 模式的主库路径（spec §4.1）：
/// `HARNESS_HUB_DB_PATH` 显式覆盖；否则按平台取应用数据目录
/// （Windows `%APPDATA%` / Linux `$XDG_DATA_HOME` 或 `~/.local/share` /
/// macOS `~/Library/Application Support`）。
fn mcp_db_path() -> Result<std::path::PathBuf, String> {
    if let Some(override_path) = std::env::var_os(DB_PATH_ENV).filter(|value| !value.is_empty()) {
        return Ok(std::path::PathBuf::from(override_path));
    }

    let base = match std::env::consts::OS {
        "windows" => {
            let appdata = std::env::var_os("APPDATA")
                .ok_or_else(|| "APPDATA 未设置，无法定位数据库目录".to_string())?;
            std::path::PathBuf::from(appdata).join(APP_IDENTIFIER)
        }
        "macos" => {
            let home = std::env::var_os("HOME")
                .ok_or_else(|| "HOME 未设置，无法定位数据库目录".to_string())?;
            std::path::PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(APP_IDENTIFIER)
        }
        _ => {
            let home = std::env::var_os("HOME")
                .ok_or_else(|| "HOME 未设置，无法定位数据库目录".to_string())?;
            let data_home = std::env::var_os("XDG_DATA_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from(home).join(".local").join("share"));
            data_home.join(APP_IDENTIFIER)
        }
    };

    Ok(base.join(DATABASE_FILE_NAME))
}

/// F22 S3：`harness-hub --mcp-serve` 的进程入口（spec §4.1）。
///
/// 同步 stdio 循环（不需要 tokio）：DB 打不开 → eprintln + 非 0 退出，
/// **绝不假装可用**；成功 → [`crate::mcp::server::run_stdio`] 循环 → 0。
/// 本函数绝不初始化 Tauri Builder / webview / 托盘。
pub fn run_mcp_server() -> i32 {
    let db_path = match mcp_db_path() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("harness-hub --mcp-serve：数据库路径解析失败：{error}");
            return 2;
        }
    };

    // 与桌面端 run() 同纪律：目录不存在就创建（SQLite 不会自动建目录）。
    if let Some(parent) = db_path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!(
                "harness-hub --mcp-serve：数据目录创建失败（{}）：{error}",
                parent.display()
            );
            return 2;
        }
    }

    let database = match Database::open(&db_path) {
        Ok(database) => database,
        Err(error) => {
            eprintln!(
                "harness-hub --mcp-serve：数据库打开失败（{}）：{error}；拒绝以不可用状态继续",
                db_path.display()
            );
            return 1;
        }
    };

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    match crate::mcp::server::run_stdio(database.connection(), stdin.lock(), stdout.lock()) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("harness-hub --mcp-serve：stdio 读写失败，退出：{error}");
            1
        }
    }
}

/// 启动桌面应用。
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            let database = Database::open(data_dir.join(DATABASE_FILE_NAME))?;
            let harnesses = Arc::new(build_harness_registry());
            let db = Arc::new(Mutex::new(database));

            // 启动时的清单同步：这是**明确的写路径**（detect → reconcile → SQLite），
            // 与只读的 list_harnesses 分开。见 harness::inventory。
            let database = db.lock().map_err(|_| "数据库锁中毒")?;
            ensure_local_target(database.connection())?;
            reconcile_harnesses(
                database.connection(),
                &harnesses.summaries(LOCAL_TARGET_ID),
                LOCAL_TARGET_ID,
                &clock::now_rfc3339(),
            )?;

            // **启动时的孤儿收敛**（约束 2）：上一个实例遗留的 running 会话
            // 在当前版本无法重新附着 PTY，因此绝不恢复成 running ——
            // 即使 PID 还活着，也只说明「进程可能还在，但 PTY 控制已丢失」。
            let converged = SessionService::new(database.connection())
                .reconcile_orphans(TerminationReason::Lost)
                .unwrap_or(0);
            if converged > 0 {
                eprintln!("启动收敛：{converged} 条遗留 running 会话被标记为 lost");
            }
            drop(database);

            // 插件扫描根在组合根解析一次：命令层只拿结果，不读环境变量。
            // 目录**不存在就创建**——首次启动时用户需要有个地方放插件，
            // 而 `plugin_refresh` 在空目录上返回空报告是合法结果。
            let plugins_root = crate::plugins::discovery::default_root(&data_dir);
            if let Err(error) = std::fs::create_dir_all(&plugins_root) {
                // 建不出来不该阻止应用启动：用户可能把根指到了一个无写权限的位置，
                // 此时插件功能降级为「一个都扫不到」，其余功能照常。
                eprintln!(
                    "插件目录创建失败（{}）：{error}；插件功能将不可用",
                    plugins_root.display()
                );
            }

            // F20：只构造管理器，不主动 spawn Python（懒启动）。`from_env()` 读
            // HARNESS_HUB_PYTHON / HARNESS_HUB_RUNTIME_ROOT，解析失败也不会 panic；
            // Python 缺失时这里照样成功，第一次 start() 才收敛成 Degraded。
            let sidecar = SidecarManager::new(SidecarConfig::from_env());

            app.manage(AppState {
                db: Arc::clone(&db),
                harnesses: Arc::clone(&harnesses),
                terminal: TerminalRuntime::new(db, harnesses, Arc::new(PortablePtyBackend::new())),
                git_probe: Arc::new(crate::project::git_root::SystemGitRootProbe),
                plugins_root,
                sidecar,
            });

            // System Tray（E19）：菜单模型在 tray_menu（纯模块，可单测），
            // 这里只做 Tauri 接线。计数 = running 会话数；菜单在构建与主窗口
            // 重新聚焦时重建（应用内既有事件，不做后台轮询）。
            // 失败不阻断主功能（headless / 无系统托盘环境降级为没有托盘）。
            if let Err(error) = setup_tray(app) {
                eprintln!("托盘初始化失败（不影响主功能）：{error}");
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // 主窗口重新聚焦 → 重建托盘菜单（活动会话计数可能已变化）。
            let focused = matches!(event, tauri::WindowEvent::Focused(true));
            if !(focused && window.label() == "main") {
                return;
            }
            let Some(tray) = window.app_handle().tray_by_id(TRAY_ID) else {
                return;
            };
            let count = active_session_count(&window.app_handle().state::<AppState>());
            let menu = build_tray_menu(window.app_handle(), count);
            if let Ok(menu) = menu {
                let _ = tray.set_menu(Some(menu));
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::activity_timeline,
            commands::app_info,
            commands::credential_list,
            commands::credential_register,
            commands::credential_remove,
            commands::db_health,
            commands::config_doctor,
            commands::create_session,
            commands::finish_session,
            commands::get_project_sessions,
            commands::get_usage_sources,
            commands::gateway_status,
            commands::kill_terminal,
            commands::list_harnesses,
            commands::list_projects,
            commands::list_sessions,
            commands::mcp_binding_add,
            commands::mcp_binding_list,
            commands::mcp_binding_remove,
            commands::mcp_import,
            commands::mcp_runtime_connect,
            commands::mcp_runtime_disconnect,
            commands::mcp_runtime_status,
            commands::mcp_runtime_tool_call,
            commands::mcp_runtime_tools_list,
            commands::mcp_server_list,
            commands::mcp_server_remove,
            commands::mcp_server_upsert,
            commands::mcp_sync_apply,
            commands::mcp_sync_preview,
            commands::plugin_list,
            commands::plugin_refresh,
            commands::profile_backups,
            commands::profile_discover,
            commands::profile_project,
            commands::profile_restore,
            commands::prompt_list,
            commands::prompt_remove,
            commands::prompt_render_apply,
            commands::prompt_render_preview,
            commands::prompt_upsert,
            commands::refresh_harnesses,
            commands::refresh_usage,
            commands::register_project,
            commands::replay_list_sessions,
            commands::replay_read_session,
            commands::replay_search,
            commands::replay_search_reindex,
            commands::replay_search_settings_get,
            commands::replay_search_settings_set,
            commands::remove_project,
            commands::resize_terminal,
            commands::resume_terminal,
            commands::session_git_snapshots,
            commands::session_git_summary,
            commands::sidecar_restart,
            commands::sidecar_start,
            commands::sidecar_status,
            commands::sidecar_stop,
            commands::skills_list,
            commands::start_terminal,
            commands::start_terminal_in_worktree,
            commands::usage_summary,
            commands::worktree_add,
            commands::worktree_list,
            commands::worktree_merge,
            commands::worktree_remove,
            commands::worktree_status,
            commands::write_terminal
        ])
        .build(tauri::generate_context!())
        .expect("构建 Harness Hub 失败")
        .run(|app_handle, event| {
            // **正常关闭**：把仍在 running 的会话收敛为 unknown/host_shutdown。
            // 来不及落库（崩溃/强杀）的情形由下次启动的孤儿收敛兜底。
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app_handle.try_state::<AppState>() {
                    // F20：显式回收 Python sidecar 进程树（Drop 是双保险，不是唯一路径）。
                    state.sidecar.shutdown();
                    if let Ok(database) = state.db.lock() {
                        let _ = SessionService::new(database.connection())
                            .reconcile_orphans(TerminationReason::HostShutdown);
                    }
                }
            }
        });
}

#[cfg(test)]
mod mcp_server_contract_tests {
    /// 防漂移契约（spec §4.1）：headless 模式按 `APP_IDENTIFIER` 定位数据库目录，
    /// 它必须与 Tauri 的应用标识一致，否则 GUI 与 `--mcp-serve` 会各看一个库。
    #[test]
    fn app_identifier_matches_tauri_conf() {
        let conf = include_str!("../tauri.conf.json");
        let value: serde_json::Value = serde_json::from_str(conf).expect("tauri.conf.json 可解析");
        assert_eq!(value["identifier"], super::APP_IDENTIFIER);
    }
}
