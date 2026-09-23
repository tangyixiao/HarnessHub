//! Task 7D-A-ii：**真实** Codex + **真实** Claude **同时**跑，互不污染。
//!
//! 与 `terminal/concurrency_tests.rs` 的分工：那边用 fake PTY backend 做可重复的确定性矩阵
//! （每次提交都跑），这边用真实 binary 证明「两个真实 child 同时存在」在真机上也成立。
//!
//! 生产链路（本文件不改任何 Core 代码）：
//!
//! ```text
//! codex@local  ─┐
//!               ├─ 同一个 TerminalRuntime / PortablePtyBackend / Emitter
//! claude@local ─┘   → 两个真实 PTY + 两个真实 child 进程
//! ```
//!
//! cwd 是两个**不同**的专用目录（`D:\HarnessHub-E2E\{codex,claude}-concurrent`）。
//! Headless 只扩 test-only responder：DSR 由事件循环**分会话**应答，生产 PTY 层一个字节
//! 都不解析（ADR-0009）。本机缺少任一 binary 时明确跳过。
//!
//! ## 三个刻意的设计决定（都是被真机打出来的）
//!
//! 1. **输入绝不从主线程写 PTY**：`PortablePtyBackend::write` 是**持锁写**
//!    （`sessions` mutex 覆盖 `write_all` + `flush`），一旦某条会话的输入缓冲区满了、
//!    写入阻塞，整个 runtime 的 kill / resize / reap 都会被这把锁冻住。
//!    测试因此把 DSR 应答与 marker 输入交给独立线程，主线程永远不会卡在写系统调用上。
//! 2. **DSR 应答有上限**（每会话 3 次）：按「历史上出现过多少个 `ESC[6n`」无限重放会把
//!    Claude 的输入缓冲区灌满，正好触发第 1 条 —— 第一次真机运行就是这么挂死的。
//! 3. **进程观测独立于 PTY**：用 `tasklist` 查 PID，而不是只问 portable-pty 的 child
//!    状态（「我们以为它还活着」不等于「OS 里真的有这个进程」）。

#![cfg(windows)]

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, Once};
use std::thread;
use std::time::{Duration, Instant};

use harness_hub_lib::db::Database;
use harness_hub_lib::harness::adapter::HarnessAdapter;
use harness_hub_lib::harness::adapters::claude_code::ClaudeCodeAdapter;
use harness_hub_lib::harness::adapters::codex::CodexAdapter;
use harness_hub_lib::harness::inventory::{installation_id, reconcile_harnesses};
use harness_hub_lib::harness::probe::{HostProbe, SystemHostProbe};
use harness_hub_lib::harness::registry::HarnessRegistry;
use harness_hub_lib::pty::PortablePtyBackend;
use harness_hub_lib::runtime::local::LOCAL_TARGET_ID;
use harness_hub_lib::session::service::SessionService;
use harness_hub_lib::session::{SessionRecord, SessionStatus, TerminationReason};
use harness_hub_lib::terminal::{Emitter, PtyEvent, TerminalRuntime};

const DSR_REQUEST: &[u8] = b"\x1b[6n";
const DSR_REPLY: &[u8] = b"\x1b[1;1R";

/// 每个会话最多应答多少次 DSR。够解锁首屏即可；无限重放会灌满输入缓冲区（见文件头）。
const MAX_DSR_REPLIES_PER_SESSION: usize = 3;

/// 「TUI 真的响应了 DSR 应答」的字节下界。
///
/// 两条 Harness 都先发 `ESC[6n`（4 字节）然后**等应答**，所以「有字节」只说明 reader
/// 活着。超过 4 字节说明 TUI 处理了应答并继续画（首屏通常是几百字节）。
const PAINT_BYTES: usize = 32;

const CODEX_CWD: &str = r"D:\HarnessHub-E2E\codex-concurrent";
const CLAUDE_CWD: &str = r"D:\HarnessHub-E2E\claude-concurrent";

/// 各自独有的 marker：隔离必须用「只有我能产生的东西」证明，而不是「总 bytes 变了」。
const CODEX_MARKER: &[u8] = b"CODEX_ONLY_31415";
const CLAUDE_MARKER: &[u8] = b"CLAUDE_ONLY_27182";

/// 本文件每个用例都会起**两个真实 Harness**；串行执行，避免 6+ 个 node 进程互抢 CPU
/// 造成假超时（cargo 会并行跑同一个 test binary 里的用例）。
static SERIAL: Mutex<()> = Mutex::new(());

/// 卡死看门狗：没有它，真机挂死只会表现为「测试静默不返回」（第一次就踩到了）。
static PROGRESS: Mutex<Option<(String, Instant)>> = Mutex::new(None);
static WATCHDOG: Once = Once::new();
/// 超过这个时间没有任何阶段推进就主动 abort，并打印卡在哪个阶段。
const STALL_LIMIT: Duration = Duration::from_secs(120);

fn serialize() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn note(phase: &str) {
    eprintln!("[phase] {phase}");
    if let Ok(mut guard) = PROGRESS.lock() {
        *guard = Some((phase.to_string(), Instant::now()));
    }
}

fn start_watchdog() {
    WATCHDOG.call_once(|| {
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(5));
            let stale = PROGRESS.lock().ok().and_then(|guard| guard.clone());
            if let Some((phase, at)) = stale {
                if at.elapsed() > STALL_LIMIT {
                    eprintln!(
                        "[watchdog] 卡在阶段「{phase}」{:?}，主动 abort（否则会静默挂死）",
                        at.elapsed()
                    );
                    std::process::abort();
                }
            }
        });
    });
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    count(haystack, needle) > 0
}

/// 独立于 PTY 的进程观测：PID 在 OS 里到底还在不在。
fn pid_alive(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist 必须可执行");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

fn wait_for_pid_death(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !pid_alive(pid) {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    !pid_alive(pid)
}

/// 定向树杀（`/T`）：只杀自己记录到的 PID 的进程树，**绝不**安杀所有 node。
fn tree_kill(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .output();
}

/// 找一个 PID 的 `node.exe` 子进程。
///
/// 为什么用 `ParentProcessId` 而不是「按 cwd / 按名字扫」：父进程**死后**这个字段仍是
/// 记录值，所以「`.cmd` shim 已被产品 kill、但它的 node 子进程还在」这种情况依然能定点
/// 清理；同时只匹配自己记录过的父 PID，绝不会误伤别的 node（本机就有别人的 node）。
fn node_children_of(parent: u32) -> Vec<u32> {
    let script = format!(
        "(Get-CimInstance Win32_Process -Filter \"ParentProcessId={parent} and Name='node.exe'\").ProcessId"
    );
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output();

    match output {
        Ok(output) => String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .filter_map(|token| token.parse::<u32>().ok())
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// 分会话的字节 + 事件收集（一个 emitter 就够，事件自带 session_id）。
///
/// `seen` 是**累积**的：事件队列会被 [`Streams::take_events`] 取空，因此「见过哪些
/// session_id」必须单独记账，不能从队列里读。
#[derive(Default)]
struct Streams {
    bytes: Mutex<HashMap<String, Vec<u8>>>,
    events: Mutex<Vec<PtyEvent>>,
    seen: Mutex<BTreeSet<String>>,
}

impl Streams {
    fn emitter(self: &Arc<Self>) -> Emitter {
        let sink = Arc::clone(self);
        Arc::new(move |event| {
            let session_id = match &event {
                PtyEvent::Started { session_id, .. }
                | PtyEvent::Output { session_id, .. }
                | PtyEvent::Exited { session_id, .. }
                | PtyEvent::Error { session_id, .. } => session_id.clone(),
            };
            if let PtyEvent::Output { data, .. } = &event {
                sink.bytes
                    .lock()
                    .expect("bytes")
                    .entry(session_id.clone())
                    .or_default()
                    .extend_from_slice(data);
            }
            sink.seen.lock().expect("seen").insert(session_id);
            sink.events.lock().expect("events").push(event);
        })
    }

    /// 只要长度（热路径）：**不要**在这里 clone 整个累积缓冲区。
    fn len(&self, session_id: &str) -> usize {
        self.bytes
            .lock()
            .expect("bytes")
            .get(session_id)
            .map_or(0, Vec::len)
    }

    fn bytes(&self, session_id: &str) -> Vec<u8> {
        self.bytes
            .lock()
            .expect("bytes")
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    fn saw(&self, session_id: &str, marker: &[u8]) -> bool {
        contains(&self.bytes(session_id), marker)
    }

    /// 从 `from` 起扫描新出现的 DSR 请求；返回 `(已扫描到的字节数, 请求数)`。
    ///
    /// 回退 `DSR_REQUEST.len() - 1` 个字节重扫，避免请求被切在两个 chunk 之间漏掉。
    fn scan_dsr(&self, session_id: &str, from: usize) -> (usize, usize) {
        let guard = self.bytes.lock().expect("bytes");
        let data = guard.get(session_id).map_or(&[][..], Vec::as_slice);
        let start = from.saturating_sub(DSR_REQUEST.len() - 1);
        if start >= data.len() {
            return (data.len(), 0);
        }
        (data.len(), count(&data[start..], DSR_REQUEST))
    }

    fn session_ids(&self) -> BTreeSet<String> {
        self.seen.lock().expect("seen").clone()
    }

    fn take_events(&self) -> Vec<PtyEvent> {
        std::mem::take(&mut *self.events.lock().expect("events"))
    }
}

/// 每个会话的 DSR 扫描 / 应答计数。
#[derive(Default, Clone)]
struct DsrState {
    scanned: usize,
    requests: usize,
    answered: usize,
}

/// 输入通道：PTY 的写**只在这个线程里发生**（见文件头第 1 条）。
struct Input {
    sender: Sender<(String, Vec<u8>)>,
}

impl Input {
    fn new(runtime: Arc<TerminalRuntime>) -> Self {
        let (sender, receiver) = mpsc::channel::<(String, Vec<u8>)>();
        thread::spawn(move || {
            while let Ok((session_id, bytes)) = receiver.recv() {
                if runtime.write(&session_id, &bytes).is_err() {
                    break;
                }
            }
        });
        Self { sender }
    }

    fn send(&self, session_id: &str, bytes: Vec<u8>) {
        let _ = self.sender.send((session_id.to_string(), bytes));
    }
}

struct Concurrent {
    db: Arc<Mutex<Database>>,
    runtime: Arc<TerminalRuntime>,
    streams: Arc<Streams>,
    input: Input,
    dsr: Mutex<HashMap<String, DsrState>>,
    /// 起过的真实 child PID：`Drop` 时兜底清理，断言失败也不留孤儿进程。
    pids: Mutex<Vec<u32>>,
    codex_installation: String,
    claude_installation: String,
}

impl Drop for Concurrent {
    fn drop(&mut self) {
        let pids = self.pids.lock().expect("pids").clone();
        for pid in pids {
            // shim 若还活着：整棵树一次带走。
            if pid_alive(pid) {
                tree_kill(pid);
            }
            // 7D 实测：Windows 上 `.cmd` shim 被杀后，它启动的 node 子进程**仍会活着**
            // （产品当前只终止直接子进程）。测试不能因此留垃圾，用记录过的父 PID 定点清理。
            for child in node_children_of(pid) {
                if pid_alive(child) {
                    tree_kill(child);
                }
            }
        }
    }
}

/// 本机缺任一 binary（或 E2E 目录建不出来）时明确跳过，绝不假装通过。
fn prepare() -> Option<Concurrent> {
    start_watchdog();
    note("prepare");

    let probe: Arc<dyn HostProbe> = Arc::new(SystemHostProbe::new());
    let codex = CodexAdapter::new(Arc::clone(&probe));
    let claude = ClaudeCodeAdapter::new(Arc::clone(&probe));

    let codex_detect = codex.detect();
    let claude_detect = claude.detect();
    eprintln!(
        "codex : installed={} binary={:?} version={:?}",
        codex_detect.installed, codex_detect.binary_path, codex_detect.version
    );
    eprintln!(
        "claude: installed={} binary={:?} version={:?}",
        claude_detect.installed, claude_detect.binary_path, claude_detect.version
    );
    if !codex_detect.installed || !claude_detect.installed {
        eprintln!("跳过：本机没有同时装好 Codex 与 Claude，无法验证并发隔离");
        return None;
    }

    for cwd in [CODEX_CWD, CLAUDE_CWD] {
        if let Err(error) = std::fs::create_dir_all(cwd) {
            eprintln!("跳过：建不出 {cwd}（{error}）");
            return None;
        }
        if !Path::new(cwd).is_dir() {
            eprintln!("跳过：{cwd} 不是目录");
            return None;
        }
    }

    let mut registry = HarnessRegistry::new();
    registry.register(Box::new(CodexAdapter::new(Arc::clone(&probe))));
    registry.register(Box::new(ClaudeCodeAdapter::new(Arc::clone(&probe))));

    let db = Database::open_in_memory().expect("内存库");
    harness_hub_lib::runtime::local::ensure_local_target(db.connection()).expect("runtime target");
    reconcile_harnesses(
        db.connection(),
        &registry.summaries(LOCAL_TARGET_ID),
        LOCAL_TARGET_ID,
        "2026-09-24T00:00:00Z",
    )
    .expect("同步两个 Harness 的清单");
    let db = Arc::new(Mutex::new(db));

    let runtime = Arc::new(TerminalRuntime::new(
        Arc::clone(&db),
        Arc::new(registry),
        Arc::new(PortablePtyBackend::new()),
    ));
    let input = Input::new(Arc::clone(&runtime));

    Some(Concurrent {
        db,
        runtime,
        streams: Arc::new(Streams::default()),
        input,
        dsr: Mutex::new(HashMap::new()),
        pids: Mutex::new(Vec::new()),
        codex_installation: installation_id("codex", LOCAL_TARGET_ID),
        claude_installation: installation_id("claude", LOCAL_TARGET_ID),
    })
}

impl Concurrent {
    fn start(&self, installation: &str, cwd: &str, cols: u16, rows: u16) -> SessionRecord {
        let record = self
            .runtime
            .start(
                installation,
                Some(cwd),
                cols,
                rows,
                Some(self.streams.emitter()),
            )
            .expect("启动会话");
        if let Some(pid) = record.pid {
            self.pids.lock().expect("pids").push(pid as u32);
        }
        record
    }

    fn start_codex(&self) -> SessionRecord {
        note("start codex");
        self.start(&self.codex_installation, CODEX_CWD, 100, 30)
    }

    fn start_claude(&self) -> SessionRecord {
        note("start claude");
        self.start(&self.claude_installation, CLAUDE_CWD, 120, 32)
    }

    fn record(&self, session_id: &str) -> SessionRecord {
        self.runtime
            .list_sessions(10)
            .expect("列出会话")
            .into_iter()
            .find(|record| record.hub_session_id == session_id)
            .expect("会话必须存在")
    }

    fn running(&self) -> Vec<String> {
        self.runtime
            .list_sessions(10)
            .expect("列出会话")
            .into_iter()
            .filter(|record| record.status == SessionStatus::Running)
            .map(|record| record.hub_session_id)
            .collect()
    }

    /// 取事件，按会话**有上限**地应答 DSR。主线程只发不收（写在线程里）。
    fn pump(&self) {
        for event in self.streams.take_events() {
            if let PtyEvent::Output { session_id, .. } = event {
                let previous = {
                    let mut guard = self.dsr.lock().expect("dsr");
                    guard.entry(session_id.clone()).or_default().scanned
                };
                let (scanned, requests) = self.streams.scan_dsr(&session_id, previous);

                let mut guard = self.dsr.lock().expect("dsr");
                let state = guard.entry(session_id.clone()).or_default();
                state.scanned = scanned;
                state.requests = state.requests.max(requests);
                while state.answered < state.requests
                    && state.answered < MAX_DSR_REPLIES_PER_SESSION
                {
                    self.input.send(&session_id, DSR_REPLY.to_vec());
                    state.answered += 1;
                }
            }
        }
    }

    /// 泵事件直到 predicate 成立或超时；返回 predicate 是否成立。
    fn pump_until(&self, timeout: Duration, predicate: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump();
            if predicate() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    fn wait_terminal(&self, session_id: &str, seconds: u64) -> SessionRecord {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        loop {
            let record = self.record(session_id);
            if record.status != SessionStatus::Running {
                return record;
            }
            assert!(
                Instant::now() < deadline,
                "会话 {session_id} 在 {seconds}s 内没有进入终态"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn converge(&self, reason: TerminationReason) -> usize {
        let guard = self.db.lock().expect("db");
        SessionService::new(guard.connection())
            .reconcile_orphans(reason)
            .expect("收敛遗留 running")
    }

    /// 等两条会话都画出真实 TUI。
    fn wait_for_first_screens(&self, codex: &str, claude: &str) {
        note("wait for first screens");
        let painted = self.pump_until(Duration::from_secs(45), || {
            self.streams.len(codex) >= PAINT_BYTES && self.streams.len(claude) >= PAINT_BYTES
        });
        let (codex_len, claude_len) = (self.streams.len(codex), self.streams.len(claude));
        assert!(
            codex_len > 0 && claude_len > 0,
            "两条会话的 reader 都必须产出真实字节（codex={codex_len} claude={claude_len}）"
        );
        assert!(
            painted,
            "两条会话都必须在 45s 内画出真实 TUI（阈值 {PAINT_BYTES} 字节；\
             codex={codex_len} claude={claude_len}）"
        );
    }

    /// 结束一条会话，并把「产品 kill 之后还有哪些 node 子进程活着」如实记录。
    fn kill_and_settle(&self, session_id: &str, pid: u32, seconds: u64) -> SessionRecord {
        note(&format!("kill {session_id}"));
        self.runtime.kill(session_id).expect("kill 必须成功");
        let record = self.wait_terminal(session_id, seconds);
        assert!(
            wait_for_pid_death(pid, Duration::from_secs(10)),
            "shim 进程 {pid} 必须真的消失"
        );

        let leftovers: Vec<u32> = node_children_of(pid)
            .into_iter()
            .filter(|child| pid_alive(*child))
            .collect();
        if !leftovers.is_empty() {
            eprintln!(
                "[finding] 产品 kill 之后，shim {pid} 的 node 子进程仍存活：{leftovers:?}\
                 （当前 kill 只终止直接子进程；测试在 Drop 里定点清理）"
            );
        }
        record
    }
}

/// 正向：两个真实 child 同时存活 → 先 kill Claude（Codex 仍可用）→ 再 kill Codex。
#[test]
fn codex_and_claude_run_at_the_same_time_in_different_directories() {
    let _serial = serialize();
    let Some(both) = prepare() else {
        return;
    };

    let codex = both.start_codex();
    let claude = both.start_claude();

    let codex_id = codex.hub_session_id.clone();
    let claude_id = claude.hub_session_id.clone();
    let codex_pid = codex.pid.expect("codex 必须有真实 pid") as u32;
    let claude_pid = claude.pid.expect("claude 必须有真实 pid") as u32;

    // --- 同时存在：session / pid / installation / cwd 各自独立 ---
    assert_ne!(codex_id, claude_id, "不得共用 hub_session_id");
    assert_ne!(codex_pid, claude_pid, "两个 child 必须是不同进程");
    assert_eq!(codex.installation_id.as_deref(), Some("codex@local"));
    assert_eq!(claude.installation_id.as_deref(), Some("claude@local"));
    assert_ne!(codex.installation_id, claude.installation_id);
    assert_eq!(codex.cwd.as_deref(), Some(CODEX_CWD));
    assert_eq!(claude.cwd.as_deref(), Some(CLAUDE_CWD));
    assert_ne!(codex.cwd, claude.cwd);
    assert!(both.runtime.is_running(&codex_id).expect("查询 codex"));
    assert!(both.runtime.is_running(&claude_id).expect("查询 claude"));
    assert_eq!(both.running().len(), 2, "两条必须同时是 running");
    assert!(pid_alive(codex_pid), "codex PID 必须真实存在");
    assert!(pid_alive(claude_pid), "claude PID 必须真实存在");
    eprintln!("[1] 同时运行: codex pid={codex_pid} claude pid={claude_pid}");

    both.wait_for_first_screens(&codex_id, &claude_id);
    let dsr = both.dsr.lock().expect("dsr").clone();
    eprintln!(
        "[2] 首屏 bytes: codex={} claude={} （DSR 应答: codex={} claude={}）",
        both.streams.len(&codex_id),
        both.streams.len(&claude_id),
        dsr.get(&codex_id).map_or(0, |state| state.answered),
        dsr.get(&claude_id).map_or(0, |state| state.answered)
    );

    // --- marker 隔离 ---
    // 正向（自己看到自己的 marker）受被测 TUI 是否回显输入影响，如实记录；
    // 反向（对方的 marker 一个字都不许出现）是硬断言。
    note("marker isolation");
    both.input.send(&codex_id, CODEX_MARKER.to_vec());
    both.input.send(&claude_id, CLAUDE_MARKER.to_vec());
    let codex_echoed = both.pump_until(Duration::from_secs(8), || {
        both.streams.saw(&codex_id, CODEX_MARKER)
    });
    let claude_echoed = both.pump_until(Duration::from_secs(8), || {
        both.streams.saw(&claude_id, CLAUDE_MARKER)
    });
    eprintln!(
        "[3] marker 回显: codex={codex_echoed} claude={claude_echoed}\
         （不回显不等于隔离失败：真实 TUI 是否回显由被测程序决定，见 README）"
    );

    let codex_bytes = both.streams.bytes(&codex_id);
    let claude_bytes = both.streams.bytes(&claude_id);
    assert!(
        !contains(&codex_bytes, CLAUDE_MARKER),
        "Claude 的 marker 出现在了 Codex 的 stream 里"
    );
    assert!(
        !contains(&claude_bytes, CODEX_MARKER),
        "Codex 的 marker 出现在了 Claude 的 stream 里"
    );
    if codex_echoed {
        assert!(
            !contains(&claude_bytes, CODEX_MARKER),
            "Codex 的输入到达了 Claude 的 PTY"
        );
    }
    if claude_echoed {
        assert!(
            !contains(&codex_bytes, CLAUDE_MARKER),
            "Claude 的输入到达了 Codex 的 PTY"
        );
    }

    assert_eq!(
        both.streams.session_ids(),
        BTreeSet::from([codex_id.clone(), claude_id.clone()]),
        "emitter 不得出现第三条会话"
    );

    // --- resize 只动 codex ---
    note("resize codex");
    let claude_bytes_before = both.streams.len(&claude_id);
    let codex_bytes_before_resize = both.streams.len(&codex_id);
    both.runtime
        .resize(&codex_id, 132, 44)
        .expect("resize codex 必须成功");
    let codex_reflowed = both.pump_until(Duration::from_secs(10), || {
        both.streams.len(&codex_id) > codex_bytes_before_resize
    });
    assert!(
        both.runtime.is_running(&claude_id).expect("查询 claude"),
        "resize codex 不得把 claude 弄死"
    );
    assert!(both.runtime.is_running(&codex_id).expect("查询 codex"));
    assert!(
        pid_alive(claude_pid),
        "resize codex 之后 claude 进程必须还活着"
    );
    assert!(
        both.streams.len(&claude_id) >= claude_bytes_before,
        "claude 的输出不得因 codex 的 resize 而丢失"
    );
    eprintln!(
        "[4] resize codex → codex 重绘={codex_reflowed}，claude 仍 running 且进程存活（bytes {} → {}）",
        claude_bytes_before,
        both.streams.len(&claude_id)
    );

    // --- kill Claude → Codex 仍 running 且仍能 write / resize / 继续产出 ---
    let claude_row = both.kill_and_settle(&claude_id, claude_pid, 40);
    assert_eq!(claude_row.status, SessionStatus::Exited);
    assert_eq!(
        claude_row.termination_reason,
        Some(TerminationReason::UserKilled)
    );
    assert!(claude_row.exit_code.is_some(), "必须记录真实退出码");

    assert_eq!(
        both.record(&codex_id).status,
        SessionStatus::Running,
        "kill claude 不得影响 codex 的状态"
    );
    assert!(
        pid_alive(codex_pid),
        "kill claude 之后 codex 进程必须还活着"
    );
    assert_eq!(both.running(), vec![codex_id.clone()]);

    note("codex 仍可 write / resize / output");
    let codex_bytes_before = both.streams.len(&codex_id);
    both.input.send(&codex_id, b"\r".to_vec());
    both.runtime
        .resize(&codex_id, 140, 50)
        .expect("codex 仍必须可 resize");
    let codex_alive_output = both.pump_until(Duration::from_secs(10), || {
        both.streams.len(&codex_id) > codex_bytes_before
    });
    assert_eq!(
        both.record(&codex_id).status,
        SessionStatus::Running,
        "write/resize 之后 codex 仍必须 running"
    );
    assert!(
        !contains(&both.streams.bytes(&codex_id), CLAUDE_MARKER),
        "Claude 已经结束，它的 marker 不得出现在 Codex 的流里"
    );
    eprintln!(
        "[5] kill claude → codex 仍 running，继续产出={codex_alive_output}（bytes {codex_bytes_before} → {}）",
        both.streams.len(&codex_id)
    );

    // --- kill Codex → 全部终态 ---
    let codex_row = both.kill_and_settle(&codex_id, codex_pid, 40);
    assert_eq!(
        codex_row.termination_reason,
        Some(TerminationReason::UserKilled)
    );
    assert!(codex_row.exit_code.is_some());
    assert!(both.running().is_empty(), "两条都结束后不得留下 running");

    // --- DB lifecycle 独立 ---
    assert_ne!(
        claude_row.hub_session_id, codex_row.hub_session_id,
        "两条会话必须是两行"
    );
    let claude_after = both.record(&claude_id);
    assert_eq!(
        claude_after.ended_at, claude_row.ended_at,
        "codex 的终态不得改写 claude 的结束时间"
    );
    assert_eq!(claude_after.exit_code, claude_row.exit_code);
    eprintln!(
        "[6] 终态: claude exit={:?} codex exit={:?} running=0",
        claude_row.exit_code, codex_row.exit_code
    );
}

/// 反向：先 kill Codex → Claude 仍健康，且仍能被 resize。
#[test]
fn killing_codex_first_leaves_claude_healthy() {
    let _serial = serialize();
    let Some(both) = prepare() else {
        return;
    };

    let codex = both.start_codex();
    let claude = both.start_claude();
    let codex_id = codex.hub_session_id.clone();
    let claude_id = claude.hub_session_id.clone();
    let codex_pid = codex.pid.expect("codex pid") as u32;
    let claude_pid = claude.pid.expect("claude pid") as u32;

    both.wait_for_first_screens(&codex_id, &claude_id);

    let codex_row = both.kill_and_settle(&codex_id, codex_pid, 40);
    assert_eq!(codex_row.status, SessionStatus::Exited);
    assert_eq!(
        codex_row.termination_reason,
        Some(TerminationReason::UserKilled)
    );

    // Claude 不受影响
    assert_eq!(
        both.record(&claude_id).status,
        SessionStatus::Running,
        "kill codex 不得影响 claude"
    );
    assert!(pid_alive(claude_pid), "claude 进程必须还活着");
    assert_eq!(both.running(), vec![claude_id.clone()]);
    assert!(
        !contains(&both.streams.bytes(&claude_id), CODEX_MARKER),
        "Codex 已结束，它的 marker 不得出现在 Claude 的流里"
    );

    // Claude 仍能产出 / 仍能被 resize
    note("resize claude after codex died");
    let before = both.streams.len(&claude_id);
    both.runtime
        .resize(&claude_id, 111, 40)
        .expect("claude 仍必须可 resize");
    let reflowed = both.pump_until(Duration::from_secs(10), || {
        both.streams.len(&claude_id) > before
    });
    assert_eq!(both.record(&claude_id).status, SessionStatus::Running);
    assert!(both.streams.len(&claude_id) >= before);
    eprintln!(
        "[反向] kill codex → claude 仍 running（重绘={reflowed}，bytes {before} → {}，pid {claude_pid} 存活）",
        both.streams.len(&claude_id)
    );

    let claude_row = both.kill_and_settle(&claude_id, claude_pid, 40);
    assert_eq!(
        claude_row.termination_reason,
        Some(TerminationReason::UserKilled)
    );
    assert!(both.running().is_empty());
}

/// 强杀 Harness Hub 后重启：**两条**旧 running 都必须收敛成 unknown/lost。
#[test]
fn restart_converges_both_running_sessions_to_lost() {
    let _serial = serialize();
    let Some(both) = prepare() else {
        return;
    };

    let codex = both.start_codex();
    let claude = both.start_claude();
    let codex_id = codex.hub_session_id.clone();
    let claude_id = claude.hub_session_id.clone();
    let codex_pid = codex.pid.expect("codex pid") as u32;
    let claude_pid = claude.pid.expect("claude pid") as u32;

    both.wait_for_first_screens(&codex_id, &claude_id);
    assert_eq!(
        both.running().len(),
        2,
        "模拟强杀之前，库里必须同时有两条 running"
    );
    assert!(pid_alive(codex_pid) && pid_alive(claude_pid));

    // 「下次启动」走的就是这个生产函数（lib.rs 的启动收敛）。
    note("reconcile two running rows");
    let converged = both.converge(TerminationReason::Lost);
    assert_eq!(
        converged, 2,
        "两条遗留 running 都必须被收敛，而不是只处理第一行"
    );

    for session_id in [&codex_id, &claude_id] {
        let row = both.record(session_id);
        assert_eq!(row.status, SessionStatus::Unknown, "绝不恢复成 running");
        assert_eq!(row.termination_reason, Some(TerminationReason::Lost));
        assert!(row.ended_at.is_some());
        assert!(row.pid.is_some(), "PID 保留作为诊断线索");
    }
    assert!(both.running().is_empty(), "不得留下 ghost running");
    eprintln!("[ghost] 两条 running 都收敛为 unknown/lost");

    // 晚到的 kill 不得改写已经写下的终态（收敛之后才收到用户操作）。
    both.runtime.kill(&codex_id).expect("晚到的 kill 不应报错");
    both.runtime.kill(&claude_id).expect("晚到的 kill 不应报错");
    for session_id in [&codex_id, &claude_id] {
        let row = both.record(session_id);
        assert_eq!(
            row.status,
            SessionStatus::Unknown,
            "晚到的 kill 不得覆盖终态"
        );
        assert_eq!(row.termination_reason, Some(TerminationReason::Lost));
    }
    assert_eq!(
        both.converge(TerminationReason::Lost),
        0,
        "幂等：已经没有 running 可收敛"
    );

    assert!(wait_for_pid_death(codex_pid, Duration::from_secs(10)));
    assert!(wait_for_pid_death(claude_pid, Duration::from_secs(10)));
}
