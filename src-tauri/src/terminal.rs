//! 终端运行编排：把 `installation → LaunchSpec → PTY → Session 状态` 串起来。
//!
//! 这是唯一知道「一次终端启动意味着什么」的地方：
//!
//! ```text
//! start_from_installation   → created
//! build_launch_spec         → 结构化启动描述（平台差异在 Adapter 里解决）
//! PtyManager.spawn          → 成功：mark_running(pid) / 失败：fail()
//! 读线程 EOF                → finish(exit_code, termination_reason)
//! ```
//!
//! 事件通过 [`Emitter`] 推给调用方（GUI 里就是 Tauri Channel），
//! **输出保持原始 bytes**，不在这里做任何 UTF-8 转换或转义序列处理。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::db::Database;
use crate::error::{Error, Result};
use crate::harness::adapter::{HarnessId, LaunchRequest};
use crate::harness::registry::HarnessRegistry;
use crate::harness::store::find_installation;
use crate::pty::{ExitSink, OutputSink, PtyBackend, PtyManager};
use crate::session::{service::SessionService, SessionRecord, TerminationReason};

/// 推给前端的流式事件。`Output.data` 是**原始字节**：
/// xterm.js 的 `write(Uint8Array)` 自带跨 chunk 的有状态 UTF-8 解码，
/// 我们不做 lossy 转换（否则多字节字符会被破坏）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PtyEvent {
    Started {
        session_id: String,
        pid: Option<u32>,
    },
    Output {
        session_id: String,
        seq: u64,
        data: Vec<u8>,
    },
    Exited {
        session_id: String,
        exit_code: Option<i32>,
        reason: TerminationReason,
    },
    Error {
        session_id: String,
        message: String,
    },
}

/// 事件接收方（GUI 里是 Tauri Channel 的发送端；无头测试里是收集器）。
pub type Emitter = Arc<dyn Fn(PtyEvent) + Send + Sync>;

pub struct TerminalRuntime {
    db: Arc<Mutex<Database>>,
    registry: Arc<HarnessRegistry>,
    /// 用户主动结束的意图：进程退出时据此区分 `user_killed` 与 `natural_exit`。
    intents: Arc<Mutex<HashMap<String, TerminationReason>>>,
    emitters: Arc<Mutex<HashMap<String, Emitter>>>,
    pty: PtyManager,
}

impl TerminalRuntime {
    pub fn new(
        db: Arc<Mutex<Database>>,
        registry: Arc<HarnessRegistry>,
        backend: Arc<dyn PtyBackend>,
    ) -> Self {
        let intents: Arc<Mutex<HashMap<String, TerminationReason>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let emitters: Arc<Mutex<HashMap<String, Emitter>>> = Arc::new(Mutex::new(HashMap::new()));

        let output_emitters = Arc::clone(&emitters);
        let on_output: OutputSink = Arc::new(move |session, seq, bytes| {
            let emitter = output_emitters
                .lock()
                .ok()
                .and_then(|map| map.get(session).cloned());

            if let Some(emitter) = emitter {
                emitter(PtyEvent::Output {
                    session_id: session.to_string(),
                    seq,
                    // 原样字节：不解析、不改写
                    data: bytes.to_vec(),
                });
            }
        });

        let exit_db = Arc::clone(&db);
        let exit_emitters = Arc::clone(&emitters);
        let exit_intents = Arc::clone(&intents);
        let on_exit: ExitSink = Arc::new(move |session, exit_code| {
            let reason = exit_intents
                .lock()
                .ok()
                .and_then(|mut map| map.remove(session))
                .unwrap_or(TerminationReason::NaturalExit);

            if let Ok(database) = exit_db.lock() {
                let service = SessionService::new(database.connection());
                // 终态写入失败不能静默：至少让事件里带上错误。
                if let Err(error) = service.finish(session, exit_code, reason) {
                    if let Some(emitter) = exit_emitters
                        .lock()
                        .ok()
                        .and_then(|mut map| map.remove(session))
                    {
                        emitter(PtyEvent::Error {
                            session_id: session.to_string(),
                            message: format!("写入会话终态失败：{error}"),
                        });
                    }
                    return;
                }
            }

            if let Some(emitter) = exit_emitters
                .lock()
                .ok()
                .and_then(|mut map| map.remove(session))
            {
                emitter(PtyEvent::Exited {
                    session_id: session.to_string(),
                    exit_code,
                    reason,
                });
            }
        });

        Self {
            db,
            registry,
            intents,
            emitters,
            pty: PtyManager::new(backend, on_output, on_exit),
        }
    }

    fn with_db<T>(&self, action: impl FnOnce(&Database) -> Result<T>) -> Result<T> {
        let database = self.db.lock().map_err(|_| Error::StateLockPoisoned)?;
        action(&database)
    }

    fn emit(&self, event: PtyEvent) {
        let session_id = match &event {
            PtyEvent::Started { session_id, .. }
            | PtyEvent::Output { session_id, .. }
            | PtyEvent::Exited { session_id, .. }
            | PtyEvent::Error { session_id, .. } => session_id.clone(),
        };

        if let Some(emitter) = self
            .emitters
            .lock()
            .ok()
            .and_then(|map| map.get(&session_id).cloned())
        {
            emitter(event);
        }
    }

    /// 启动一次终端会话。
    ///
    /// **spawn 失败必须落到 `failed`（`launch_failed`），绝不能先伪装成 running。**
    pub fn start(
        &self,
        installation_id: &str,
        cwd: Option<&str>,
        cols: u16,
        rows: u16,
        emitter: Option<Emitter>,
    ) -> Result<SessionRecord> {
        let session = self.with_db(|database| {
            SessionService::new(database.connection()).start_from_installation(
                installation_id,
                None,
                cwd,
            )
        })?;
        let session_id = session.hub_session_id.clone();

        if let Some(emitter) = emitter {
            if let Ok(mut emitters) = self.emitters.lock() {
                emitters.insert(session_id.clone(), emitter);
            }
        }

        // 取安装 + 适配器 → 结构化 LaunchSpec（这里不拼任何命令字符串）
        let prepared = self.with_db(|database| {
            let installation = find_installation(database.connection(), installation_id)?
                .ok_or_else(|| Error::InvalidInput(format!("未知的安装：{installation_id}")))?;
            let adapter = self
                .registry
                .get(&HarnessId::from(installation.harness_id.as_str()))
                .ok_or_else(|| {
                    Error::InvalidInput(format!("未注册的 Harness：{}", installation.harness_id))
                })?;

            adapter.build_launch_spec(LaunchRequest {
                project_id: None,
                cwd: cwd.unwrap_or_default().to_string(),
                args: Vec::new(),
                runtime_target_id: installation.runtime_target_id.clone(),
            })
        });

        let spec = match prepared {
            Ok(spec) => spec,
            Err(error) => {
                self.fail_launch(&session_id, &error)?;
                return Err(error);
            }
        };

        match self.pty.spawn(&session_id, spec, cols, rows) {
            Ok(handle) => {
                // 顺序是刻意的：先把状态迁移到 running（并记下 pid），**再**开始读。
                // 否则进程若立刻退出，EOF 会早于状态迁移到达，会话会永久卡在 created。
                self.with_db(|database| {
                    SessionService::new(database.connection()).mark_running(&session_id, handle.pid)
                })?;
                self.pty.start_reading(&session_id)?;

                self.emit(PtyEvent::Started {
                    session_id: session_id.clone(),
                    pid: handle.pid,
                });

                self.reload(&session_id)
            }
            Err(error) => {
                self.fail_launch(&session_id, &error)?;
                Err(error)
            }
        }
    }

    /// 启动失败：写 `failed` / `launch_failed`，并把错误推给前端。
    fn fail_launch(&self, session_id: &str, error: &Error) -> Result<()> {
        self.with_db(|database| SessionService::new(database.connection()).fail(session_id))?;
        self.emit(PtyEvent::Error {
            session_id: session_id.to_string(),
            message: error.to_string(),
        });
        Ok(())
    }

    fn reload(&self, session_id: &str) -> Result<SessionRecord> {
        self.with_db(|database| {
            SessionService::new(database.connection())
                .get(session_id)?
                .ok_or_else(|| Error::InvalidInput(format!("会话不存在：{session_id}")))
        })
    }

    pub fn write(&self, session_id: &str, bytes: &[u8]) -> Result<()> {
        self.pty.write(session_id, bytes)
    }

    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<()> {
        self.pty.resize(session_id, cols, rows)
    }

    /// 用户主动结束：先记意图再 kill，这样退出回调能写对 `termination_reason`。
    pub fn kill(&self, session_id: &str) -> Result<()> {
        if let Ok(mut intents) = self.intents.lock() {
            intents.insert(session_id.to_string(), TerminationReason::UserKilled);
        }
        self.pty.kill(session_id)
    }

    pub fn is_running(&self, session_id: &str) -> Result<bool> {
        self.pty.is_running(session_id)
    }

    pub fn list_sessions(&self, limit: u32) -> Result<Vec<SessionRecord>> {
        self.with_db(|database| SessionService::new(database.connection()).list_recent(limit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::adapters::codex::CodexAdapter;
    use crate::harness::probe::HostProbe;
    use crate::pty::manager::fake::FakePtyBackend;
    use crate::session::SessionStatus;
    use crate::test_support::{detected_codex_summary, RUNTIME_TARGET_ID};

    use std::path::{Path, PathBuf};

    /// 假宿主：让 Codex 适配器「已安装」，从而能产出 LaunchSpec。
    struct InstalledProbe;

    impl HostProbe for InstalledProbe {
        fn find_executable(&self, _name: &str) -> Option<PathBuf> {
            Some(PathBuf::from("D:/npm-global/codex.cmd"))
        }

        fn read_version(&self, _executable: &Path) -> Result<Option<String>> {
            Ok(Some("0.152.1".to_string()))
        }

        fn dir_exists(&self, _path: &Path) -> bool {
            false
        }

        fn home_dir(&self) -> Option<PathBuf> {
            None
        }
    }

    fn runtime(backend: Arc<FakePtyBackend>) -> (TerminalRuntime, String) {
        let db = crate::test_support::empty_db();
        crate::runtime::local::ensure_local_target(db.connection()).expect("runtime target");
        crate::harness::inventory::reconcile_harnesses(
            db.connection(),
            &[detected_codex_summary()],
            RUNTIME_TARGET_ID,
            "2026-01-01T00:00:00Z",
        )
        .expect("同步清单");

        let mut registry = HarnessRegistry::new();
        registry.register(Box::new(CodexAdapter::new(Arc::new(InstalledProbe))));

        let installation = crate::harness::inventory::installation_id("codex", RUNTIME_TARGET_ID);
        (
            TerminalRuntime::new(
                Arc::new(Mutex::new(db)),
                Arc::new(registry),
                backend as Arc<dyn PtyBackend>,
            ),
            installation,
        )
    }

    fn collector() -> (Emitter, Arc<Mutex<Vec<PtyEvent>>>) {
        let events: Arc<Mutex<Vec<PtyEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        (
            Arc::new(move |event| sink.lock().expect("events").push(event)),
            events,
        )
    }

    #[test]
    fn start_marks_running_with_pid_and_emits_started() {
        let backend = Arc::new(FakePtyBackend::new());
        let (runtime, installation) = runtime(Arc::clone(&backend));
        let (emitter, events) = collector();

        let session = runtime
            .start(&installation, Some("D:/work"), 120, 30, Some(emitter))
            .expect("启动");

        assert_eq!(session.status, SessionStatus::Running);
        assert_eq!(session.pid, Some(4242), "running 会话必须记下 PID");
        assert_eq!(
            session.installation_id.as_deref(),
            Some(installation.as_str())
        );

        let started = events.lock().expect("events").clone();
        assert!(
            matches!(
                started.as_slice(),
                [PtyEvent::Started {
                    pid: Some(4242),
                    ..
                }]
            ),
            "必须发出 Started 事件并带 pid：{started:?}"
        );
    }

    /// spawn 失败绝不能先伪装成 running。
    #[test]
    fn spawn_failure_marks_launch_failed_and_never_running() {
        let backend = Arc::new(FakePtyBackend::new().failing_spawn());
        let (runtime, installation) = runtime(Arc::clone(&backend));
        let (emitter, events) = collector();

        let error = runtime
            .start(&installation, None, 80, 24, Some(emitter))
            .expect_err("spawn 失败必须返回错误");

        assert!(error.to_string().contains("spawn"), "错误信息：{error}");

        let sessions = runtime.list_sessions(10).expect("列出");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].status, SessionStatus::Failed);
        assert_eq!(
            sessions[0].termination_reason,
            Some(TerminationReason::LaunchFailed)
        );
        assert!(sessions[0].pid.is_none(), "启动失败不应留下 pid");

        let recorded = events.lock().expect("events").clone();
        assert!(
            matches!(recorded.as_slice(), [PtyEvent::Error { .. }]),
            "必须发出 Error 事件：{recorded:?}"
        );
    }

    #[test]
    fn natural_exit_finishes_the_session_and_emits_exit_code() {
        let backend = Arc::new(
            FakePtyBackend::new()
                .with_output(vec![b"codex-cli 0.152.1".to_vec()])
                .with_always_exited(Some(0)),
        );
        let (runtime, installation) = runtime(Arc::clone(&backend));
        let (emitter, events) = collector();

        let session = runtime
            .start(&installation, None, 80, 24, Some(emitter))
            .expect("启动");

        // 等读线程收到 EOF 并写终态
        let mut finished = None;
        for _ in 0..60 {
            if let Some(record) = runtime
                .list_sessions(10)
                .expect("列出")
                .into_iter()
                .find(|record| record.hub_session_id == session.hub_session_id)
            {
                if record.status != SessionStatus::Running {
                    finished = Some(record);
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }

        let finished = finished.expect("会话应进入终态");
        assert_eq!(
            finished.termination_reason,
            Some(TerminationReason::NaturalExit),
            "没有 kill 意图时按自然退出处理"
        );

        // 原始输出必须原样到达 emitter
        let recorded = events.lock().expect("events").clone();
        let output: Vec<u8> = recorded
            .iter()
            .filter_map(|event| match event {
                PtyEvent::Output { data, .. } => Some(data.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(
            String::from_utf8(output).expect("UTF-8"),
            "codex-cli 0.152.1"
        );
    }
}
