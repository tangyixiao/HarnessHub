//! Task「OpenCode Terminal 生命周期」：OpenCode 侧 lifecycle 的**纯取证**（不新增设计、
//! 不改 Runtime Core）。
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
//! cwd 指向专用 E2E 目录。spike 实测：OpenCode 是原生 exe，会渲染真 TUI
//! （alternate screen + bracketed paste），Ctrl+C×2 自然退出、退出码 0。
//! 本机 0 credentials 属 readiness（ADR-0005），不影响生命周期取证。
//! 本机没装 OpenCode 或该目录不存在时明确跳过。

#![cfg(windows)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use harness_hub_lib::db::Database;
use harness_hub_lib::harness::adapter::HarnessAdapter;
use harness_hub_lib::harness::adapters::opencode::{
    OpenCodeAdapter, OpenCodeXdgRoots, OPENCODE_ID,
};
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
const E2E_WORKSPACE: &str = r"D:\HarnessHub-E2E\opencode-terminal";

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
    let adapter = OpenCodeAdapter::new(
        Arc::new(SystemHostProbe::new()),
        OpenCodeXdgRoots::from_env(),
    );
    if !adapter.detect().installed || !Path::new(E2E_WORKSPACE).is_dir() {
        eprintln!("跳过：本机没有 OpenCode CLI，或 {E2E_WORKSPACE} 不存在");
        return None;
    }

    let db = Arc::new(Mutex::new(Database::open_in_memory().expect("内存库")));
    {
        let guard = db.lock().expect("db");
        harness_hub_lib::runtime::local::ensure_local_target(guard.connection())
            .expect("runtime target");
        let mut registry = HarnessRegistry::new();
        registry.register(Box::new(OpenCodeAdapter::new(
            Arc::new(SystemHostProbe::new()),
            OpenCodeXdgRoots::from_env(),
        )));
        reconcile_harnesses(
            guard.connection(),
            &registry.summaries(LOCAL_TARGET_ID),
            LOCAL_TARGET_ID,
            "2026-10-12T00:00:00Z",
        )
        .expect("同步清单");
    }

    let mut registry = HarnessRegistry::new();
    registry.register(Box::new(OpenCodeAdapter::new(
        Arc::new(SystemHostProbe::new()),
        OpenCodeXdgRoots::from_env(),
    )));
    let runtime = TerminalRuntime::new(
        Arc::clone(&db),
        Arc::new(registry),
        Arc::new(PortablePtyBackend::new()),
    );

    Some(Harness {
        db,
        runtime,
        sink: Arc::new(Mutex::new(Sink::default())),
        installation: installation_id(OPENCODE_ID, LOCAL_TARGET_ID),
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
            .expect("启动 OpenCode");
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
        assert!(answered >= 1, "OpenCode 会发 DSR，responder 必须应答过");
        (session_id, bytes)
    }

    /// 等 TUI 输出静默（模拟「用户看到画面后才按键」）：Bun 启动 + TUI 挂载需要
    /// 时间，Ctrl+C 发得太早会被还在初始化的进程吞掉。
    fn wait_quiet(&self, session_id: &str, bytes: &mut Vec<u8>, answered: &mut usize) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let before = bytes.len();
            std::thread::sleep(Duration::from_millis(1200));
            drain(&self.runtime, session_id, &self.sink, bytes, answered);
            if bytes.len() == before || Instant::now() >= deadline {
                return;
            }
        }
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

/// S1：用户 kill（GUI 已有同类证据，这里在 headless 复现，保持四条链整齐）。
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

/// S2：OpenCode **自己**退出（TUI 内 Ctrl+C×2，spike 实测退出码 0；
/// **不**调用 kill_terminal）→ natural_exit。
///
/// Ctrl+C 可能需要不止一对（TUI 还在初始化时按键会被吞掉），所以这里像真实
/// 用户一样：等画面静默后按一对，若 25s 仍未终态再补一对。
#[test]
fn s2_opencode_can_exit_naturally() {
    let Some(harness) = prepare() else {
        return;
    };
    let (session_id, mut bytes) = harness.start();
    let mut answered = 0usize;
    harness.wait_quiet(&session_id, &mut bytes, &mut answered);
    eprintln!(
        "S2 首屏 {} bytes，TUI 标志 bracketed_paste = {} alternate_screen = {}",
        bytes.len(),
        count(&bytes, b"\x1b[?2004h"),
        count(&bytes, b"\x1b[?1049h")
    );

    let mut ctrl_c_pairs = 0;
    let finished = loop {
        ctrl_c_pairs += 1;
        harness
            .runtime
            .write(&session_id, b"\x03")
            .expect("写入第一次 Ctrl+C");
        std::thread::sleep(Duration::from_millis(600));
        harness
            .runtime
            .write(&session_id, b"\x03")
            .expect("写入第二次 Ctrl+C");
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut outcome = None;
        while outcome.is_none() {
            let record = harness
                .runtime
                .list_sessions(10)
                .expect("列出")
                .into_iter()
                .find(|record| record.hub_session_id == session_id)
                .expect("会话应存在");
            if record.status != SessionStatus::Running {
                outcome = Some(record);
                break;
            }
            // 观察窗到期必须**静默退出**内层循环，让外层按文档注释补发下一对；
            // 在这里 panic 会让「补一对」的重试永远不可达（2026-10-16 实测：
            // TUI 初始化变慢吞键时第一对超时直接炸测试）。
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if let Some(record) = outcome {
            break record;
        }
        assert!(ctrl_c_pairs < 3, "三对 Ctrl+C 后仍未自然退出");
        eprintln!("S2 第 {ctrl_c_pairs} 对 Ctrl+C 未生效（TUI 初始化吞键），补发一对");
    };

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
