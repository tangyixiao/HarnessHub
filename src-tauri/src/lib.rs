//! Harness Hub —— Rust Control Plane。
//!
//! 分层约束（见 docs/CONTEXT-MAP.md）：
//! - 领域模块（`db` / `harness` / `usage` / `session` / …）不得依赖 Tauri GUI 类型；
//! - 只有 `commands` 与 [`run`] 直接接触 Tauri；
//! - SQLite 由本 crate 独占持有，Python sidecar 不得直连。

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
pub mod trace;
pub mod usage;
pub mod watcher;

#[cfg(test)]
pub(crate) mod test_support;

use std::sync::Mutex;

use tauri::Manager;

use crate::db::Database;

/// 应用级共享状态。数据库句柄让命令层可以访问同一个连接。
pub struct AppState {
    pub db: Mutex<Database>,
}

/// 数据库文件名。放在 Tauri 的 app data 目录下，保持 local-first。
pub const DATABASE_FILE_NAME: &str = "harness-hub.sqlite3";

/// 启动桌面应用。
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;

            let database = Database::open(data_dir.join(DATABASE_FILE_NAME))?;
            app.manage(AppState {
                db: Mutex::new(database),
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::db_health
        ])
        .run(tauri::generate_context!())
        .expect("Harness Hub 启动失败");
}
