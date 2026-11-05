//! Sidecar 生命周期状态机与降级恢复（F20 spec §4.4）。
//!
//! **验收 #5 的关键纪律**：`status / start / stop / restart` 一律不返回 `Err`。
//! Python 缺失、PYTHONPATH 错、握手失败、进程崩溃全部收敛成
//! `SidecarStatus { state: Degraded, last_error: Some(..) }` —— 只让设置页
//! 那一状态卡片降级，PTY / Git / Session / 插件宿主零感知（ADR-0006）。
//!
//! `call()` 单独保留 `Result`，供 F21 的 Provider 调用使用（当前 UI 不调）。
//!
//! 状态机：
//! ```text
//! Stopped   --start()-->              Ready | Degraded
//! Ready     --进程死亡(status 检出)--> Degraded   last_error = "sidecar 进程已退出"
//! Degraded  --start()/restart()-->    Ready | Degraded   restart_count += 1
//! 任意      --stop()-->               Stopped
//! ```
//!
//! 冷启动纪律：`new()` 不 spawn 任何进程，`start()` 才走完整
//! `spawn → initialize 握手`，测试不允许夹具直接注入 Ready。

use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::Value;

use crate::error::Error;
use crate::sidecar::config::SidecarConfig;
use crate::sidecar::dto::{SidecarState, SidecarStatus};
use crate::sidecar::process::SidecarProcess;

struct ManagerInner {
    /// 当前活进程；`None` = 没启动 / 已停止 / 已回收。
    process: Option<SidecarProcess>,
    /// 对外快照（IPC 命令直接 clone 它）。
    status: SidecarStatus,
}

/// 线程安全的 sidecar 生命周期管理器。`Tauri State` 持有 `Arc<SidecarManager>`。
pub struct SidecarManager {
    inner: Mutex<ManagerInner>,
    config: SidecarConfig,
}

impl SidecarManager {
    /// 只构造状态，**懒启动**：不拉起 Python，解释器缺失也不影响主功能。
    pub fn new(config: SidecarConfig) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(ManagerInner {
                process: None,
                status: SidecarStatus::stopped(),
            }),
            config,
        })
    }

    /// 锁中毒也不 panic：拿到被污染的数据照样对外返回状态。
    fn lock(&self) -> MutexGuard<'_, ManagerInner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// 当前状态快照。**Ready 时顺手做崩溃检出**（验收 #4 的检出点）。
    pub fn status(&self) -> SidecarStatus {
        let mut inner = self.lock();
        if inner.status.state == SidecarState::Ready {
            let dead = match inner.process.as_mut() {
                Some(process) => !process.is_alive(),
                None => true,
            };
            if dead {
                // 取出后 Drop 会 reap 子进程，不留僵尸。
                inner.process = None;
                inner.status.state = SidecarState::Degraded;
                inner.status.pid = None;
                inner.status.last_error = Some("sidecar 进程已退出".to_string());
            }
        }
        inner.status.clone()
    }

    /// 启动（幂等）。成功 → Ready；任何失败 → Degraded + 可读原因，**绝不返回 Err**。
    pub fn start(&self) -> SidecarStatus {
        let mut inner = self.lock();

        // 已经 Ready 且进程还活着：直接返回，别重复 spawn。
        if inner.status.state == SidecarState::Ready {
            if let Some(process) = inner.process.as_mut() {
                if process.is_alive() {
                    return inner.status.clone();
                }
            }
        }

        // 清掉可能存在的陈旧进程（Degraded 残留 / 已死进程），Drop 会回收。
        if let Some(mut stale) = inner.process.take() {
            stale.shutdown();
        }

        match self.try_start(inner.status.restart_count) {
            Ok((process, status)) => {
                inner.process = Some(process);
                inner.status = status.clone();
                status
            }
            Err(message) => {
                let mut status = inner.status.clone();
                status.state = SidecarState::Degraded;
                status.pid = None;
                status.runtime = None;
                status.negotiated_capabilities = vec![];
                status.protocol_version = None;
                status.last_error = Some(message);
                inner.status = status.clone();
                status
            }
        }
    }

    /// 停止（幂等），保留 `restart_count` 使其单调。
    pub fn stop(&self) -> SidecarStatus {
        let mut inner = self.lock();
        if let Some(mut process) = inner.process.take() {
            process.shutdown();
        }
        let mut status = SidecarStatus::stopped();
        status.restart_count = inner.status.restart_count;
        inner.status = status.clone();
        status
    }

    /// 重启 = stop + start，并自增 `restart_count`（崩溃恢复的可见证据）。
    pub fn restart(&self) -> SidecarStatus {
        self.stop();
        let _ = self.start();
        let mut inner = self.lock();
        inner.status.restart_count += 1;
        inner.status.clone()
    }

    /// 显式回收（`lib.rs` 的 `RunEvent::Exit` 会调；`Drop` 是双保险）。
    pub fn shutdown(&self) {
        let mut inner = self.lock();
        if let Some(mut process) = inner.process.take() {
            process.shutdown();
        }
        inner.status = SidecarStatus::stopped();
    }

    /// 调一次方法（供 F21）。`Err` 也会把状态收敛成 Degraded，方便 UI 自愈。
    pub fn call(&self, method: &str, params: &Value) -> std::result::Result<Value, String> {
        let mut inner = self.lock();
        let outcome = match inner.process.as_mut() {
            None => Err(Error::SidecarUnavailable("sidecar 未启动".to_string()).to_string()),
            Some(process) => process
                .call(method, params)
                .map_err(|error| error.to_string()),
        };
        match outcome {
            Ok(value) => Ok(value),
            Err(message) => {
                inner.status.state = SidecarState::Degraded;
                inner.status.last_error = Some(message.clone());
                inner.process = None; // Drop 回收坏流
                Err(message)
            }
        }
    }

    /// 真正的冷启动：spawn → 握手。失败时 `process` 在此 Drop，自动回收。
    fn try_start(
        &self,
        restart_count: u32,
    ) -> std::result::Result<(SidecarProcess, SidecarStatus), String> {
        let mut process = SidecarProcess::spawn(&self.config).map_err(|error| error.to_string())?;
        let info = process
            .handshake(&self.config)
            .map_err(|error| error.to_string())?;
        let status = SidecarStatus {
            state: SidecarState::Ready,
            pid: Some(process.pid()),
            runtime: Some(info.runtime),
            negotiated_capabilities: info.negotiated_capabilities,
            protocol_version: Some(info.protocol_version),
            last_error: None,
            restart_count,
        };
        Ok((process, status))
    }
}

impl Drop for SidecarManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar::dto::RuntimeInfo;
    use serde_json::json;

    fn config_with_missing_python() -> SidecarConfig {
        SidecarConfig {
            python: std::path::PathBuf::from("no-such-python-interpreter-9f3b.exe"),
            runtime_root: std::path::PathBuf::from("python"),
            host_version: "0.5.0".to_string(),
        }
    }

    /// 构造 manager 不 spawn：初始必须是 Stopped 的干净快照。
    #[test]
    fn new_is_lazy_and_reports_stopped_without_spawning() {
        let manager = SidecarManager::new(config_with_missing_python());
        assert_eq!(manager.status(), SidecarStatus::stopped());
        assert_eq!(manager.status().state, SidecarState::Stopped);
    }

    /// 解释器缺失：start() 收敛成 Degraded 而不是 panic / Err。
    #[test]
    fn start_degrades_without_error_when_interpreter_is_missing() {
        let manager = SidecarManager::new(config_with_missing_python());
        let status = manager.start();
        assert_eq!(status.state, SidecarState::Degraded);
        assert!(
            status
                .last_error
                .as_deref()
                .unwrap_or_default()
                .contains("启动失败"),
            "{status:?}"
        );
        assert_eq!(status.pid, None);
        assert_eq!(status.runtime, None);
        assert_eq!(manager.status().state, SidecarState::Degraded);
        assert_eq!(manager.stop().state, SidecarState::Stopped);
    }

    /// stop() 幂等，且保留 restart_count 的单调性。
    #[test]
    fn stop_is_idempotent_and_keeps_restart_count() {
        let manager = SidecarManager::new(config_with_missing_python());
        assert_eq!(manager.stop().state, SidecarState::Stopped);
        assert_eq!(manager.stop().state, SidecarState::Stopped);
        let _ = manager.restart();
        let after = manager.stop();
        assert!(after.restart_count >= 1, "{after:?}");
        assert_eq!(after.state, SidecarState::Stopped);
    }

    /// call() 在未启动时返回可读错误，不 panic。
    #[test]
    fn call_before_start_returns_unavailable() {
        let manager = SidecarManager::new(config_with_missing_python());
        let error = manager.call("ping", &json!({})).expect_err("未启动应失败");
        assert!(error.contains("不可用"), "{error}");
        assert_eq!(manager.status().state, SidecarState::Degraded);
    }

    /// 锁中毒也不 panic：用一次 panic 污染锁之后 status() 仍要能返回。
    #[test]
    fn poisoned_lock_does_not_panic_the_host() {
        let manager = SidecarManager::new(config_with_missing_python());
        let clone = Arc::clone(&manager);
        let _ = std::thread::spawn(move || {
            let _guard = clone.inner.lock().expect("拿锁");
            panic!("故意污染锁");
        })
        .join();
        // 不管是否真的污染到，status() 都必须返回而不是 panic。
        let status = manager.status();
        assert_eq!(status.state, SidecarState::Stopped);
    }

    /// Ready → 死亡 → Degraded 的状态迁移逻辑（用手工状态，不 spawn 真进程）。
    #[test]
    fn ready_without_live_process_is_detected_as_degraded() {
        let manager = SidecarManager::new(config_with_missing_python());
        {
            let mut inner = manager.lock();
            inner.status = SidecarStatus {
                state: SidecarState::Ready,
                pid: Some(4242),
                runtime: Some(RuntimeInfo {
                    name: "harness-hub-runtime".to_string(),
                    version: "0.5.0".to_string(),
                    transport: "stdio-jsonrpc".to_string(),
                    python: "3.14.3".to_string(),
                    protocol_version: "1.0".to_string(),
                    providers: vec![],
                    capabilities: vec![],
                    methods: vec![],
                }),
                negotiated_capabilities: vec![],
                protocol_version: Some("1.0".to_string()),
                last_error: None,
                restart_count: 0,
            };
            // 刻意不放 process：模拟「进程已经没了但快照还是 Ready」。
            inner.process = None;
        }
        let status = manager.status();
        assert_eq!(status.state, SidecarState::Degraded);
        assert_eq!(status.pid, None);
        assert!(
            status
                .last_error
                .as_deref()
                .unwrap_or_default()
                .contains("进程已退出"),
            "{status:?}"
        );
    }
}
