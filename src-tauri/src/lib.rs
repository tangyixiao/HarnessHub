//! Harness Hub —— Rust Control Plane。
//!
//! 分层约束（见 docs/CONTEXT-MAP.md）：
//! - 领域模块（`db` / `harness` / `usage` / `session` / …）不得依赖 Tauri GUI 类型；
//! - 只有 `commands` 与 [`run`] 直接接触 Tauri；
//! - SQLite 由本 crate 独占持有，Python sidecar 不得直连。

pub mod clock;
pub mod commands;
pub mod db;
pub mod error;
pub mod eval;
pub mod git;
pub mod harness;
pub mod hhar;
pub mod permissions;
pub mod plugins;
pub mod process;
pub mod pty;
pub mod runtime;
pub mod session;
pub mod sidecar;
pub mod terminal;
pub mod trace;
pub mod usage;
pub mod watcher;

#[cfg(test)]
pub(crate) mod test_support;

use std::sync::{Arc, Mutex};

use tauri::Manager;

use crate::db::Database;
use crate::harness::adapters::codex::CodexAdapter;
use crate::harness::inventory::reconcile_harnesses;
use crate::harness::probe::SystemHostProbe;
use crate::harness::registry::HarnessRegistry;
use crate::runtime::local::{ensure_local_target, LOCAL_TARGET_ID};
use crate::session::{service::SessionService, TerminationReason};

/// 应用级共享状态。
///
/// - `db`：数据库句柄，命令层与领域服务共用同一个连接。
/// - `harnesses`：已注册的 Harness 适配器。启动时一次性装配，之后只读，
///   因此不需要加锁。**真实检测**发生在读取或同步清单时（`detect()`），
///   不在启动时缓存 —— 用户可能在应用运行期间安装/卸载 Harness。
pub struct AppState {
    pub db: Mutex<Database>,
    pub harnesses: HarnessRegistry,
}

/// 数据库文件名。放在 Tauri 的 app data 目录下，保持 local-first。
pub const DATABASE_FILE_NAME: &str = "harness-hub.sqlite3";

/// 装配 Harness 注册表。
///
/// v0.1 只注册 Codex：**不为「看起来支持很多」而注册没有真实检测的适配器**
/// （ADR-0022：能检测到不算支持，可稳定回归才算）。
fn build_harness_registry() -> HarnessRegistry {
    let mut registry = HarnessRegistry::new();
    registry.register(Box::new(CodexAdapter::new(
        Arc::new(SystemHostProbe::new()),
    )));
    registry
}

/// 启动桌面应用。
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            let database = Database::open(data_dir.join(DATABASE_FILE_NAME))?;
            let harnesses = build_harness_registry();

            // 启动时的清单同步：这是**明确的写路径**（detect → reconcile → SQLite），
            // 与只读的 list_harnesses 分开。见 harness::inventory。
            ensure_local_target(database.connection())?;
            reconcile_harnesses(
                database.connection(),
                &harnesses.summaries(),
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

            app.manage(AppState {
                db: Mutex::new(database),
                harnesses,
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::db_health,
            commands::create_session,
            commands::finish_session,
            commands::list_harnesses,
            commands::list_sessions,
            commands::refresh_harnesses
        ])
        .build(tauri::generate_context!())
        .expect("构建 Harness Hub 失败")
        .run(|app_handle, event| {
            // **正常关闭**：把仍在 running 的会话收敛为 unknown/host_shutdown。
            // 来不及落库（崩溃/强杀）的情形由下次启动的孤儿收敛兜底。
            if let tauri::RunEvent::Exit = event {
                if let Some(state) = app_handle.try_state::<AppState>() {
                    if let Ok(database) = state.db.lock() {
                        let _ = SessionService::new(database.connection())
                            .reconcile_orphans(TerminationReason::HostShutdown);
                    }
                }
            }
        });
}
