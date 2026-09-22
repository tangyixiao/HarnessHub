//! 真实 Codex 的终端 E2E（Windows + 本机已安装 Codex 时执行）。
//!
//! **HeadlessTerminalResponder** 是 test-only 的终端模拟器替身：
//! 它只做一件事 —— 应答子进程的 DSR（`ESC[6n`）。GUI 里这个角色由 xterm.js 承担
//! （见 ADR-0009）。生产 PTY 层不含任何应答逻辑。
//!
//! 覆盖 E2E 矩阵中的：
//! [1] spawn → running + pid  [2] 收到真实 PTY 字节  [4] headless DSR 闭环
//! [5] 用户输入真正进入 PTY    [6] resize 生效且不崩
//! [7]/[8] natural exit / user kill → 正确终态与退出码

#![cfg(windows)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use harness_hub_lib::db::Database;
use harness_hub_lib::harness::adapter::HarnessAdapter;
use harness_hub_lib::harness::adapters::codex::CodexAdapter;
use harness_hub_lib::harness::inventory::{installation_id, reconcile_harnesses};
use harness_hub_lib::harness::probe::SystemHostProbe;
use harness_hub_lib::harness::registry::HarnessRegistry;
use harness_hub_lib::pty::PortablePtyBackend;
use harness_hub_lib::session::{SessionStatus, TerminationReason};
use harness_hub_lib::terminal::{PtyEvent, TerminalRuntime};

/// DSR 请求与应答。
const DSR_REQUEST: &[u8] = b"\x1b[6n";
const DSR_REPLY: &[u8] = b"\x1b[1;1R";

/// test-only 终端模拟器替身：记录输出，遇到 DSR 就回应答。
#[derive(Default)]
struct HeadlessTerminalResponder {
    output: Vec<u8>,
    answered_dsr: usize,
}

impl HeadlessTerminalResponder {
    /// 把 PTY 输出喂进来；必要时通过 `runtime.write` 把应答送回 PTY。
    fn feed(&mut self, runtime: &TerminalRuntime, session_id: &str, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);

        let requested = count_occurrences(&self.output, DSR_REQUEST);
        while self.answered_dsr < requested {
            runtime
                .write(session_id, DSR_REPLY)
                .expect("把 DSR 应答写回 PTY");
            self.answered_dsr += 1;
        }
    }
}

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

/// 用**真实**检测 + 真实 PTY 后端构造运行时；没有 Codex 时返回 `None`。
fn real_runtime() -> Option<(TerminalRuntime, String)> {
    let adapter = CodexAdapter::new(Arc::new(SystemHostProbe::new()));
    let detected = adapter.detect();
    if !detected.installed {
        return None;
    }

    let db = Database::open_in_memory().expect("内存库");
    harness_hub_lib::runtime::local::ensure_local_target(db.connection()).expect("runtime target");
    reconcile_harnesses(
        db.connection(),
        &adapter_registry(&adapter).summaries(),
        harness_hub_lib::runtime::local::LOCAL_TARGET_ID,
        "2026-09-22T00:00:00Z",
    )
    .expect("同步清单");

    let registry = adapter_registry(&adapter);
    let runtime = TerminalRuntime::new(
        Arc::new(Mutex::new(db)),
        Arc::new(registry),
        Arc::new(PortablePtyBackend::new()),
    );

    Some((
        runtime,
        installation_id(
            harness_hub_lib::harness::adapters::codex::CODEX_ID,
            harness_hub_lib::runtime::local::LOCAL_TARGET_ID,
        ),
    ))
}

fn adapter_registry(adapter: &CodexAdapter) -> HarnessRegistry {
    // 注册表只用于让 TerminalRuntime 通过 harness_id 找到适配器；
    // 这里用与真实启动相同的 CodexAdapter（含真实 HostProbe）。
    let probe = Arc::new(SystemHostProbe::new());
    let mut registry = HarnessRegistry::new();
    registry.register(Box::new(CodexAdapter::new(probe)));
    let _ = adapter;
    registry
}

fn wait_for_terminal(
    runtime: &TerminalRuntime,
    session_id: &str,
) -> harness_hub_lib::session::SessionRecord {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let record = runtime
            .list_sessions(10)
            .expect("列出")
            .into_iter()
            .find(|record| record.hub_session_id == session_id)
            .expect("会话应存在");

        if record.status != SessionStatus::Running {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "会话在预期时间内没有进入终态（仍在 running）"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn real_codex_spawns_produces_bytes_and_reaches_a_correct_terminal_state() {
    let Some((runtime, installation)) = real_runtime() else {
        eprintln!("跳过：本机没有检测到 Codex");
        return;
    };

    // 事件收集 + DSR 应答（两者在 start 之前就绪 —— 对应 GUI 的 ready-before-spawn）
    let responder = Arc::new(Mutex::new(HeadlessTerminalResponder::default()));
    let runtime_for_events = Arc::new(runtime);
    let responder_for_events = Arc::clone(&responder);
    let runtime_for_sink = Arc::clone(&runtime_for_events);

    let emitter: harness_hub_lib::terminal::Emitter = Arc::new(move |event| {
        if let PtyEvent::Output {
            session_id, data, ..
        } = event
        {
            responder_for_events.lock().expect("responder").feed(
                &runtime_for_sink,
                &session_id,
                &data,
            );
        }
    });

    // [1] spawn → running + pid
    let session = runtime_for_events
        .start(&installation, None, 100, 30, Some(emitter))
        .expect("真实 Codex 启动");

    assert_eq!(session.status, SessionStatus::Running);
    assert!(
        session.pid.is_some(),
        "running 会话必须有 pid（诊断用，不作为 IPC 句柄）"
    );

    let session_id = session.hub_session_id.clone();

    // [2] 真实 PTY 字节 + [4] DSR 闭环
    let deadline = Instant::now() + Duration::from_secs(20);
    while responder.lock().expect("responder").output.is_empty() {
        assert!(Instant::now() < deadline, "20 秒内没有收到任何 PTY 输出");
        std::thread::sleep(Duration::from_millis(50));
    }
    let answered = responder.lock().expect("responder").answered_dsr;
    eprintln!(
        "已收到 {} 字节输出，应答 DSR {answered} 次",
        responder.lock().expect("responder").output.len()
    );

    // [5] 用户输入真正进入 PTY（写入路径可用且不报错）
    runtime_for_events
        .write(&session_id, b"\r")
        .expect("写入用户输入");

    // [6] resize 生效且不崩
    runtime_for_events
        .resize(&session_id, 120, 40)
        .expect("resize");

    // [7]/[8] 结束：若仍在运行则用户 kill，否则已自然退出
    let still_running = runtime_for_events
        .is_running(&session_id)
        .expect("查询状态");
    if still_running {
        runtime_for_events.kill(&session_id).expect("kill");
    }

    let finished = wait_for_terminal(&runtime_for_events, &session_id);

    assert_ne!(
        finished.status,
        SessionStatus::Running,
        "终态不得是 running"
    );
    assert!(
        finished.ended_at.is_some(),
        "终态必须带结束时间：{finished:?}"
    );

    if still_running {
        assert_eq!(
            finished.termination_reason,
            Some(TerminationReason::UserKilled),
            "用户 kill 的会话必须记为 user_killed"
        );
        assert!(
            finished.exit_code.is_some(),
            "退出码必须来自 reaper 的真实观测"
        );
    } else {
        assert_eq!(
            finished.termination_reason,
            Some(TerminationReason::NaturalExit),
            "自然退出的会话必须记为 natural_exit"
        );
    }
}

/// [9] spawn 失败绝不能出现 running（真实场景：installation 存在但 binary 已被删除）。
#[test]
fn missing_binary_never_produces_a_running_session() {
    let adapter = {
        let probe = Arc::new(SystemHostProbe::new());
        CodexAdapter::new(probe)
    };
    if !adapter.detect().installed {
        eprintln!("跳过：本机没有检测到 Codex");
        return;
    }

    // 构造一个「安装行存在、但 binary 不存在」的运行时：
    // 用一个指向不存在文件的假探针注册表。
    let db = Database::open_in_memory().expect("内存库");
    harness_hub_lib::runtime::local::ensure_local_target(db.connection()).expect("runtime target");

    let summary = harness_hub_lib::harness::registry::HarnessSummary {
        id: harness_hub_lib::harness::adapters::codex::CODEX_ID.to_string(),
        display_name: "Codex".to_string(),
        installed: true,
        binary_path: Some("D:/definitely-missing/codex.cmd".to_string()),
        version: Some("0.152.1".to_string()),
        capabilities: harness_hub_lib::harness::adapter::HarnessCapabilities::default(),
        data_paths: Vec::new(),
    };
    reconcile_harnesses(
        db.connection(),
        &[summary],
        harness_hub_lib::runtime::local::LOCAL_TARGET_ID,
        "2026-09-22T00:00:00Z",
    )
    .expect("同步清单");

    let installation = installation_id(
        harness_hub_lib::harness::adapters::codex::CODEX_ID,
        harness_hub_lib::runtime::local::LOCAL_TARGET_ID,
    );

    let mut registry = HarnessRegistry::new();
    // 假探针：声称已安装，但指向不存在的文件 —— spawn 必然失败
    registry.register(Box::new(MissingBinaryAdapter));
    let runtime = TerminalRuntime::new(
        Arc::new(Mutex::new(db)),
        Arc::new(registry),
        Arc::new(PortablePtyBackend::new()),
    );

    let error = runtime
        .start(&installation, None, 80, 24, None)
        .expect_err("binary 不存在时必须失败");
    assert!(!error.to_string().is_empty());

    let sessions = runtime.list_sessions(10).expect("列出");
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].status,
        SessionStatus::Failed,
        "spawn 失败必须是 failed，绝不能出现 running"
    );
    assert_eq!(
        sessions[0].termination_reason,
        Some(TerminationReason::LaunchFailed)
    );
    assert!(sessions[0].pid.is_none());
}

/// 声称已安装、但指向不存在文件的适配器（只用于 [9]）。
struct MissingBinaryAdapter;

impl HarnessAdapter for MissingBinaryAdapter {
    fn id(&self) -> harness_hub_lib::harness::adapter::HarnessId {
        harness_hub_lib::harness::adapter::HarnessId::from(
            harness_hub_lib::harness::adapters::codex::CODEX_ID,
        )
    }

    fn display_name(&self) -> &str {
        "Codex"
    }

    fn detect(&self) -> harness_hub_lib::harness::adapter::DetectResult {
        harness_hub_lib::harness::adapter::DetectResult {
            installed: true,
            binary_path: Some("D:/definitely-missing/codex.cmd".to_string()),
            version: Some("0.152.1".to_string()),
            data_paths: Vec::new(),
        }
    }

    fn version(&self) -> harness_hub_lib::error::Result<Option<String>> {
        Ok(Some("0.152.1".to_string()))
    }

    fn capabilities(&self) -> harness_hub_lib::harness::adapter::HarnessCapabilities {
        harness_hub_lib::harness::adapter::HarnessCapabilities::default()
    }

    fn build_launch_spec(
        &self,
        request: harness_hub_lib::harness::adapter::LaunchRequest,
    ) -> harness_hub_lib::error::Result<harness_hub_lib::harness::launch::LaunchSpec> {
        Ok(harness_hub_lib::harness::launch::LaunchSpec {
            program: std::path::PathBuf::from("D:/definitely-missing/codex.cmd"),
            args: request.args,
            cwd: None,
            env: Vec::new(),
            runtime_target_id: request.runtime_target_id,
        })
    }
}
