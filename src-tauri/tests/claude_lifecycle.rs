//! Task 7B.1：Claude 侧剩余 lifecycle 的**纯取证**（不新增设计、不改 Runtime Core）。
//!
//! 每个场景都**新开一个 Session**，这样 DB 里能留下干净的链：
//!
//! ```text
//! S1 → user_killed
//! S2 → natural_exit
//! S3 → host_shutdown
//! S4 → lost
//! ```
//!
//! S3/S4 直接调用**生产收敛函数** `SessionService::reconcile_orphans`（启动时与
//! `RunEvent::Exit` 走的就是它），而不是在测试里另写一套收敛逻辑。
//!
//! cwd 指向专用 E2E 目录：GUI 验收已经为该目录记录了 trust，因此 Claude 直接进入聊天界面
//! （无需在 headless 里应答 trust 菜单）。本机没装 Claude 或该目录不存在时明确跳过。

#![cfg(windows)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use harness_hub_lib::db::Database;
use harness_hub_lib::harness::adapter::HarnessAdapter;
use harness_hub_lib::harness::adapters::claude_code::{ClaudeCodeAdapter, CLAUDE_ID};
use harness_hub_lib::harness::inventory::{installation_id, reconcile_harnesses};
use harness_hub_lib::harness::probe::SystemHostProbe;
use harness_hub_lib::harness::registry::HarnessRegistry;
use harness_hub_lib::pty::PortablePtyBackend;
use harness_hub_lib::runtime::local::LOCAL_TARGET_ID;
use harness_hub_lib::session::service::SessionService;
use harness_hub_lib::session::{SessionRecord, SessionStatus, TerminationReason};
use harness_hub_lib::terminal::{Emitter, PtyEvent, TerminalRuntime};

const DSR_REQUEST: &[u8] = b"\x1b[6n";
const DSR_REPLY: &[u8] = b"\x1b[1;1R";
const E2E_WORKSPACE: &str = r"D:\HarnessHub-E2E\claude-terminal";

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

#[derive(Default)]
struct Sink {
    events: Vec<PtyEvent>,
}

fn emitter_into(sink: Arc<Mutex<Sink>>) -> Emitter {
    Arc::new(move |event| sink.lock().expect("sink").events.push(event))
}

/// test-only 终端模拟器：只做「看到 ESC[6n 就回 ESC[1;1R」。
fn drain(
    runtime: &TerminalRuntime,
    session_id: &str,
    sink: &Arc<Mutex<Sink>>,
    bytes: &mut Vec<u8>,
    answered: &mut usize,
) {
    let events: Vec<PtyEvent> = std::mem::take(&mut sink.lock().expect("sink").events);
    for event in events {
        if let PtyEvent::Output { data, .. } = event {
            bytes.extend_from_slice(&data);
            let requested = count(bytes, DSR_REQUEST);
            while *answered < requested {
                runtime.write(session_id, DSR_REPLY).expect("应答 DSR");
                *answered += 1;
            }
        }
    }
}

struct Harness {
    /// 与 [`TerminalRuntime`] 共用**同一个** DB（收敛函数要拿 Connection）。
    db: Arc<Mutex<Database>>,
    runtime: TerminalRuntime,
    sink: Arc<Mutex<Sink>>,
    installation: String,
}

fn prepare() -> Option<Harness> {
    let adapter = ClaudeCodeAdapter::new(Arc::new(SystemHostProbe::new()));
    if !adapter.detect().installed || !Path::new(E2E_WORKSPACE).is_dir() {
        eprintln!("跳过：本机没有 Claude Code，或 {E2E_WORKSPACE} 不存在");
        return None;
    }

    let db = Arc::new(Mutex::new(Database::open_in_memory().expect("内存库")));
    {
        let guard = db.lock().expect("db");
        harness_hub_lib::runtime::local::ensure_local_target(guard.connection())
            .expect("runtime target");
        let mut registry = HarnessRegistry::new();
        registry.register(Box::new(ClaudeCodeAdapter::new(Arc::new(
            SystemHostProbe::new(),
        ))));
        reconcile_harnesses(
            guard.connection(),
            &registry.summaries(LOCAL_TARGET_ID),
            LOCAL_TARGET_ID,
            "2026-09-23T00:00:00Z",
        )
        .expect("同步清单");
    }

    let mut registry = HarnessRegistry::new();
    registry.register(Box::new(ClaudeCodeAdapter::new(Arc::new(
        SystemHostProbe::new(),
    ))));
    let runtime = TerminalRuntime::new(
        Arc::clone(&db),
        Arc::new(registry),
        Arc::new(PortablePtyBackend::new()),
    );

    Some(Harness {
        db,
        runtime,
        sink: Arc::new(Mutex::new(Sink::default())),
        installation: installation_id(CLAUDE_ID, LOCAL_TARGET_ID),
    })
}

impl Harness {
    fn start(&self) -> (String, Vec<u8>) {
        let started = self
            .runtime
            .start(
                &self.installation,
                Some(E2E_WORKSPACE),
                100,
                30,
                Some(emitter_into(Arc::clone(&self.sink))),
            )
            .expect("启动 Claude");
        let session_id = started.hub_session_id.clone();

        let mut bytes = Vec::new();
        let mut answered = 0usize;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            drain(
                &self.runtime,
                &session_id,
                &self.sink,
                &mut bytes,
                &mut answered,
            );
            if bytes.len() > 300 && answered >= 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(120));
        }
        assert!(answered >= 1, "Claude 会发 DSR，responder 必须应答过");
        (session_id, bytes)
    }

    fn wait_terminal(&self, session_id: &str, seconds: u64) -> SessionRecord {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            let record = self
                .runtime
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
                "会话 {session_id} 在 {seconds}s 内没有进入终态"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn running(&self) -> usize {
        self.runtime
            .list_sessions(50)
            .expect("列出")
            .into_iter()
            .filter(|record| record.status == SessionStatus::Running)
            .count()
    }

    fn converge(&self, reason: TerminationReason) -> usize {
        let guard = self.db.lock().expect("db");
        SessionService::new(guard.connection())
            .reconcile_orphans(reason)
            .expect("收敛")
    }
}

/// S1：用户 kill（GUI 已有证据，这里在 headless 复现，保持四条链整齐）。
#[test]
fn s1_user_kill_records_user_killed() {
    let Some(harness) = prepare() else {
        return;
    };
    let (session_id, _) = harness.start();

    harness.runtime.kill(&session_id).expect("kill");
    let finished = harness.wait_terminal(&session_id, 40);

    assert_eq!(
        finished.termination_reason,
        Some(TerminationReason::UserKilled)
    );
    assert!(finished.exit_code.is_some(), "必须记录真实退出码");
    assert_eq!(harness.running(), 0);
    eprintln!("S1 user_killed: exit_code={:?}", finished.exit_code);
}

/// S2：Claude **自己**退出（发 `/exit`，**不**调用 kill_terminal）→ natural_exit。
#[test]
fn s2_claude_can_exit_naturally() {
    let Some(harness) = prepare() else {
        return;
    };
    let (session_id, bytes) = harness.start();
    eprintln!(
        "S2 首屏 {} bytes，聊天界面 = {}",
        bytes.len(),
        String::from_utf8_lossy(&bytes).contains("for shortcuts")
    );

    harness
        .runtime
        .write(&session_id, b"/exit\r")
        .expect("写入 /exit");

    let finished = harness.wait_terminal(&session_id, 60);
    assert_eq!(
        finished.termination_reason,
        Some(TerminationReason::NaturalExit),
        "没有用户 kill 意图时必须记为 natural_exit，实际 {:?}",
        finished.termination_reason
    );
    assert!(finished.exit_code.is_some(), "必须记录真实退出码");
    assert_eq!(harness.running(), 0);
    eprintln!("S2 natural_exit: exit_code={:?}", finished.exit_code);
}

/// S3：应用**正常关闭**（`RunEvent::Exit` 走的就是这个生产函数）→ host_shutdown。
#[test]
fn s3_graceful_shutdown_converges_to_host_shutdown() {
    let Some(harness) = prepare() else {
        return;
    };
    let (session_id, _) = harness.start();
    assert_eq!(harness.running(), 1);

    let converged = harness.converge(TerminationReason::HostShutdown);

    assert_eq!(converged, 1, "必须收敛 1 条 running 会话");
    let record = harness.wait_terminal(&session_id, 10);
    assert_eq!(record.status, SessionStatus::Unknown);
    assert_eq!(
        record.termination_reason,
        Some(TerminationReason::HostShutdown)
    );
    assert_eq!(harness.running(), 0, "不得留下 ghost running");
    eprintln!("S3 host_shutdown: status={:?}", record.status);
}

/// S4：应用被**强杀**（DB 暂留 running）→ 下次启动用 Lost 收敛；
/// PID 是否存活**绝不能**让它恢复成 running。
#[test]
fn s4_hard_kill_converges_to_lost_and_never_resurrects_running() {
    let Some(harness) = prepare() else {
        return;
    };
    let (session_id, _) = harness.start();
    let pid = harness
        .runtime
        .list_sessions(10)
        .expect("列出")
        .into_iter()
        .find(|record| record.hub_session_id == session_id)
        .expect("会话")
        .pid;
    assert_eq!(harness.running(), 1, "强杀前 DB 里是 running");

    // 模拟强杀：不通知子进程，直接走「下次启动」的收敛路径。
    assert_eq!(harness.converge(TerminationReason::Lost), 1);

    let record = harness.wait_terminal(&session_id, 10);
    assert_eq!(record.status, SessionStatus::Unknown);
    assert_eq!(record.termination_reason, Some(TerminationReason::Lost));
    assert_eq!(harness.running(), 0, "哪怕 PID 还活着也不得恢复成 running");

    assert_eq!(
        harness.converge(TerminationReason::Lost),
        0,
        "幂等：已经没有 running 了"
    );
    eprintln!("S4 lost: pid={pid:?}（PID 是否存活不影响状态）");
}
