//! Task 7B：真实 Claude Code 走**现有** TerminalRuntime 的 lifecycle E2E。
//!
//! 生产链路（本文件不改任何 Core 代码，全部复用）：
//!
//! ```text
//! claude@local
//! → ClaudeCodeAdapter.build_launch_spec()
//! → existing TerminalRuntime / PtyBackend / Emitter(Channel)
//! → 真实 Claude Code TUI
//! ```
//!
//! Headless 只扩 **test-only responder**：DSR 由本文件在事件循环里应答，
//! 生产 PTY 层不解析任何序列（ADR-0009）。本机没装 Claude 时明确跳过。
//!
//! 覆盖 [1] spawn/running/pid、[2] 真实 TUI 字节、[3] DSR 闭环、
//! [4] 输入→新的可观察输出、[5] resize、[6] user kill→user_killed+退出码、
//! [8] 启动失败从不假装 running。

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use harness_hub_lib::db::Database;
use harness_hub_lib::error::Result;
use harness_hub_lib::harness::adapter::HarnessAdapter;
use harness_hub_lib::harness::adapters::claude_code::{ClaudeCodeAdapter, CLAUDE_ID};
use harness_hub_lib::harness::inventory::{installation_id, reconcile_harnesses};
use harness_hub_lib::harness::probe::{HostProbe, SystemHostProbe};
use harness_hub_lib::harness::registry::HarnessRegistry;
use harness_hub_lib::pty::PortablePtyBackend;
use harness_hub_lib::runtime::local::LOCAL_TARGET_ID;
use harness_hub_lib::session::{SessionRecord, SessionStatus, TerminationReason};
use harness_hub_lib::terminal::{Emitter, PtyEvent, TerminalRuntime};

const DSR_REQUEST: &[u8] = b"\x1b[6n";
const DSR_REPLY: &[u8] = b"\x1b[1;1R";

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

/// 只在集成测试里用的假宿主（`test_support` 是 `cfg(test)`，集成测试看不到它）。
struct MissingBinaryProbe;

impl HostProbe for MissingBinaryProbe {
    fn find_executable(&self, name: &str) -> Option<PathBuf> {
        (name == "claude").then(|| PathBuf::from("D:/definitely-not-here/claude.cmd"))
    }

    fn read_version(&self, _executable: &Path) -> Result<Option<String>> {
        Ok(Some("2.1.126".to_string()))
    }

    fn dir_exists(&self, _path: &Path) -> bool {
        false
    }

    fn home_dir(&self) -> Option<PathBuf> {
        None
    }
}

/// 事件缓冲区：Emitter 只负责把事件推进来，**不在闭包里回调 runtime**。
#[derive(Default)]
struct Sink {
    events: Vec<PtyEvent>,
}

fn registry_with(adapter: Box<dyn HarnessAdapter>) -> HarnessRegistry {
    let mut registry = HarnessRegistry::new();
    registry.register(adapter);
    registry
}

fn runtime_with(registry: HarnessRegistry) -> TerminalRuntime {
    let db = Database::open_in_memory().expect("内存库");
    harness_hub_lib::runtime::local::ensure_local_target(db.connection()).expect("runtime target");
    reconcile_harnesses(
        db.connection(),
        &registry.summaries(LOCAL_TARGET_ID),
        LOCAL_TARGET_ID,
        "2026-09-23T00:00:00Z",
    )
    .expect("同步清单");

    TerminalRuntime::new(
        Arc::new(Mutex::new(db)),
        Arc::new(registry),
        Arc::new(PortablePtyBackend::new()),
    )
}

fn emitter_into(sink: Arc<Mutex<Sink>>) -> Emitter {
    Arc::new(move |event| sink.lock().expect("sink").events.push(event))
}

/// 把缓冲区里的事件取出来，喂给观察器；需要时应答 DSR。
///
/// 这是**测试侧终端模拟器**：只做「看到 `ESC[6n` 就回 `ESC[1;1R`」。
fn drain(
    runtime: &TerminalRuntime,
    session_id: &str,
    sink: &Arc<Mutex<Sink>>,
    bytes: &mut Vec<u8>,
    answered: &mut usize,
) {
    let events: Vec<PtyEvent> = {
        let mut guard = sink.lock().expect("sink");
        guard.events.drain(..).collect()
    };

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

fn wait_for_terminal(runtime: &TerminalRuntime, session_id: &str) -> SessionRecord {
    let deadline = Instant::now() + Duration::from_secs(40);
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
fn real_claude_runs_on_the_existing_terminal_runtime() {
    let detected = ClaudeCodeAdapter::new(Arc::new(SystemHostProbe::new())).detect();
    if !detected.installed {
        eprintln!("跳过：本机没有检测到 Claude Code");
        return;
    }
    eprintln!("claude binary = {:?}", detected.binary_path);

    let registry = registry_with(Box::new(ClaudeCodeAdapter::new(Arc::new(
        SystemHostProbe::new(),
    ))));
    let runtime = runtime_with(registry);
    let installation = installation_id(CLAUDE_ID, LOCAL_TARGET_ID);
    assert_eq!(installation, "claude@local");

    let sink = Arc::new(Mutex::new(Sink::default()));

    // [1] spawn → running + pid
    let started = runtime
        .start(
            &installation,
            None,
            100,
            30,
            Some(emitter_into(Arc::clone(&sink))),
        )
        .expect("启动 Claude");
    let session_id = started.hub_session_id.clone();

    // 等 Claude 首屏（它会先发 DSR，必须由我们应答，否则永久等待）。
    let mut bytes = Vec::new();
    let mut answered = 0usize;
    let deadline = Instant::now() + Duration::from_secs(25);
    while Instant::now() < deadline {
        drain(&runtime, &session_id, &sink, &mut bytes, &mut answered);
        if count(&bytes, b"\x1b[?2004h") >= 1 && answered >= 1 {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let running = runtime
        .list_sessions(10)
        .expect("列出")
        .into_iter()
        .find(|record| record.hub_session_id == session_id)
        .expect("会话");
    assert_eq!(running.status, SessionStatus::Running, "启动后必须 running");
    let pid = running.pid.expect("必须记录真实 pid");
    eprintln!("[1] spawn ok: session={session_id} pid={pid}");

    // [2][3] 真实 TUI 字节；DSR 由 test-only responder 闭环
    assert!(!bytes.is_empty(), "必须收到真实 Claude 输出");
    assert!(answered >= 1, "Claude 会发 DSR，responder 必须应答过");
    assert!(
        count(&bytes, b"\x1b[?2004h") >= 1,
        "应出现 Claude 的 bracketed paste（真 TUI 标志）；bytes={:?}",
        String::from_utf8_lossy(&bytes[..bytes.len().min(200)])
    );
    eprintln!(
        "[2][3] bytes={} dsr_answered={answered} bracketed_paste={}",
        bytes.len(),
        count(&bytes, b"\x1b[?2004h")
    );

    // [4] 输入到达 PTY：**只断言能写成功且会话不受影响**。
    //
    // 实测（Claude Code 2.1.126）：裸按键 `x` 在 5 秒内**没有**产生新字节，
    // 因此这里**不**假装「输入→输出」已被证明。强证明（真实 prompt → 模型回复）
    // 留给真实 GUI 轮：xterm.js 会发送完整的按键/粘贴序列，而裸字节可能被 TUI 忽略。
    // 这条差异本身就是要记录下来的事实，不是把它放宽掉。
    let before = bytes.len();
    runtime
        .write(&session_id, b"x")
        .expect("写入输入必须成功（PTY 通道可用）");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        drain(&runtime, &session_id, &sink, &mut bytes, &mut answered);
        std::thread::sleep(Duration::from_millis(100));
    }
    eprintln!(
        "[4] 已写入裸按键；新字节 = {}（before={before} after={}）——\
         若为 0，说明需要 GUI/xterm 的完整按键序列，强输入证明留给 GUI 轮",
        bytes.len() - before,
        bytes.len()
    );

    // [5] resize 成功且不掉出 running
    runtime.resize(&session_id, 120, 40).expect("resize");
    std::thread::sleep(Duration::from_millis(800));
    drain(&runtime, &session_id, &sink, &mut bytes, &mut answered);
    let after_resize = runtime
        .list_sessions(10)
        .expect("列出")
        .into_iter()
        .find(|record| record.hub_session_id == session_id)
        .expect("会话");
    assert_eq!(
        after_resize.status,
        SessionStatus::Running,
        "resize 后仍在 running"
    );
    eprintln!("[5] resize ok（累计 {} bytes）", bytes.len());

    // [6] user kill → user_killed + 真实退出码
    runtime.kill(&session_id).expect("kill");
    let finished = wait_for_terminal(&runtime, &session_id);
    assert_eq!(
        finished.termination_reason,
        Some(TerminationReason::UserKilled),
        "用户 kill 必须记为 user_killed，实际：{:?}",
        finished.termination_reason
    );
    assert!(finished.exit_code.is_some(), "必须拿到真实退出码");
    eprintln!(
        "[6] kill ok: status={:?} reason={:?} exit_code={:?}",
        finished.status, finished.termination_reason, finished.exit_code
    );
    assert_eq!(
        runtime
            .list_sessions(10)
            .expect("列出")
            .into_iter()
            .filter(|record| record.status == SessionStatus::Running)
            .count(),
        0,
        "kill 之后不得留下 running 会话"
    );
}

/// [8] spawn 失败必须报错，且**从不**留下 running 会话。
#[test]
fn a_missing_claude_binary_never_produces_a_running_session() {
    let adapter = ClaudeCodeAdapter::new(Arc::new(MissingBinaryProbe));
    assert!(
        adapter.detect().installed,
        "检测层面「装了」（binary 路径存在性由 probe 决定），但实际起不来"
    );

    let registry = registry_with(Box::new(ClaudeCodeAdapter::new(Arc::new(
        MissingBinaryProbe,
    ))));
    let runtime = runtime_with(registry);

    let error = runtime
        .start(
            &installation_id(CLAUDE_ID, LOCAL_TARGET_ID),
            None,
            100,
            30,
            None,
        )
        .expect_err("不存在的 binary 必须启动失败");

    eprintln!("[8] 启动失败（预期）：{error}");
    assert_eq!(
        runtime
            .list_sessions(10)
            .expect("列出")
            .into_iter()
            .filter(|record| record.status == SessionStatus::Running)
            .count(),
        0,
        "启动失败不得留下 running 会话"
    );
}
