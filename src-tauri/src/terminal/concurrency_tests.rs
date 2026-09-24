//! 7D-A-i：Cross-Harness **并发隔离**矩阵（确定性，跑在 fake PTY backend 上）。
//!
//! 为什么需要这一层：真机 GUI 只能稳定驱动**一个** active session，而「Hub」成立的前提是
//! 两个 Harness **同时**跑却不互相污染。这类性质必须能作为自动回归每次提交都跑，
//! 因此主体证据放在这里；真实进程版本见 `tests/two_harness_concurrency.rs`。
//!
//! 被测对象是**生产的** `TerminalRuntime`：fake 只替换 PTY 传输层；会话状态机、emitter
//! 分发、kill 意图（`user_killed`）、终态落库、孤儿收敛全部走真实代码。
//!
//! fake 按 **LaunchSpec.program** 分流输出与 pid（见 `pty::manager::fake`）。
//! 为什么要按 program：`hub_session_id` 是 runtime 内部生成的 UUID，测试在 spawn 之前
//! 无法预知；program 是「这是哪条会话」在 spawn 前唯一可确定的线索。
//!
//! 这一步的第一个 RED 很有价值，值得留档：最初用 fake 的**共享**输出队列起两条会话时，
//! 谁先 `take_reader` 谁把两条会话的输出全拿走 —— `codex stream` 里出现了
//! `CLAUDE_ONLY_27182`。也就是说，一个不区分会话的传输层会让「隔离」测试说谎。
//! 修复点在夹具（生产 `PortablePtyBackend` 本来就按 session_id 存），
//! 但断言自始至终没变：**每条会话只能看到自己的东西**。

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::db::Database;
use crate::harness::adapters::claude_code::ClaudeCodeAdapter;
use crate::harness::adapters::codex::CodexAdapter;
use crate::harness::inventory::{installation_id, reconcile_harnesses};
use crate::harness::probe::HostProbe;
use crate::harness::registry::HarnessRegistry;
use crate::pty::backend::PtyBackend;
use crate::pty::manager::fake::FakePtyBackend;
use crate::runtime::local::{ensure_local_target, LOCAL_TARGET_ID};
use crate::session::{service::SessionService, SessionStatus, TerminationReason};
use crate::terminal::{Emitter, PtyEvent, TerminalRuntime};
use crate::test_support::{empty_db, FakeHostProbe, NOW};

/// 两个 Harness 在**不同 cwd** 上跑（Gate 要求 cwd 不同，且要能被证明）。
const CODEX_CWD: &str = "D:/HarnessHub-E2E/codex-concurrent";
const CLAUDE_CWD: &str = "D:/HarnessHub-E2E/claude-concurrent";

/// 假宿主上两个 shim 的路径；fake backend 就按这个 program 分流。
const CODEX_PROGRAM: &str = "D:/npm-global/codex.cmd";
const CLAUDE_PROGRAM: &str = "D:/npm-global/claude.cmd";

/// 两条会话各自的 pid：Gate 要求「pid 不同」是可断言的。
const CODEX_PID: u32 = 11_001;
const CLAUDE_PID: u32 = 22_002;

/// 各自独有的 marker：只比较「总 bytes 变没变」证明不了隔离。
const CODEX_MARKER: &[u8] = b"CODEX_ONLY_31415";
const CLAUDE_MARKER: &[u8] = b"CLAUDE_ONLY_27182";

fn marker_block(marker: &[u8]) -> Vec<u8> {
    let mut block = marker.to_vec();
    block.extend_from_slice(b"\r\n");
    block
}

fn installed_host() -> FakeHostProbe {
    FakeHostProbe::new()
        .with_binary("codex", CODEX_PROGRAM, "codex-cli 0.152.1")
        .with_binary("claude", CLAUDE_PROGRAM, "2.1.126 (Claude Code)")
        .with_dir("/home/dev/.claude")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// 事件收集器：一个会话一个，证明「谁收到了什么」而不是「总字节变了」。
#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<PtyEvent>>,
}

impl Recorder {
    fn emitter(self: &Arc<Self>) -> Emitter {
        let sink = Arc::clone(self);
        Arc::new(move |event| sink.events.lock().expect("events").push(event))
    }

    fn snapshot(&self) -> Vec<PtyEvent> {
        self.events.lock().expect("events").clone()
    }

    /// 该会话收到的全部原始输出字节（按到达顺序拼接）。
    fn bytes(&self, session_id: &str) -> Vec<u8> {
        self.snapshot()
            .iter()
            .filter_map(|event| match event {
                PtyEvent::Output {
                    session_id: seen,
                    data,
                    ..
                } if seen == session_id => Some(data.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// 这个收集器**见过**的所有 session_id（隔离断言用：不得出现别人的）。
    fn session_ids(&self) -> BTreeSet<String> {
        self.snapshot()
            .iter()
            .map(|event| match event {
                PtyEvent::Started { session_id, .. }
                | PtyEvent::Output { session_id, .. }
                | PtyEvent::Exited { session_id, .. }
                | PtyEvent::Error { session_id, .. } => session_id.clone(),
            })
            .collect()
    }

    fn saw_marker(&self, session_id: &str, marker: &[u8]) -> bool {
        contains(&self.bytes(session_id), marker)
    }
}

struct Fixture {
    db: Arc<Mutex<Database>>,
    runtime: TerminalRuntime,
    backend: Arc<FakePtyBackend>,
    codex: Arc<Recorder>,
    claude: Arc<Recorder>,
    codex_installation: String,
    claude_installation: String,
}

/// 两条会话都能产出的夹具：各自预置自己的 marker，pid 也不同。
fn concurrent_fixture() -> Fixture {
    Fixture::new(
        FakePtyBackend::new()
            .with_output_for_program(CODEX_PROGRAM, vec![marker_block(CODEX_MARKER)])
            .with_output_for_program(CLAUDE_PROGRAM, vec![marker_block(CLAUDE_MARKER)])
            .with_pid_for_program(CODEX_PROGRAM, CODEX_PID)
            .with_pid_for_program(CLAUDE_PROGRAM, CLAUDE_PID),
    )
}

impl Fixture {
    fn new(backend: FakePtyBackend) -> Self {
        let backend = Arc::new(backend);

        // 两个适配器共用同一个假宿主：Core 里没有「第一个 Harness」这种概念。
        let probe: Arc<dyn HostProbe> = Arc::new(installed_host());
        let mut registry = HarnessRegistry::new();
        registry.register(Box::new(CodexAdapter::new(Arc::clone(&probe))));
        registry.register(Box::new(ClaudeCodeAdapter::new(Arc::clone(&probe))));

        // 冷启动路径：空库 → 只调用生产代码（ensure_local_target + reconcile）填前置状态。
        let db = empty_db();
        ensure_local_target(db.connection()).expect("runtime target");
        reconcile_harnesses(
            db.connection(),
            &registry.summaries(LOCAL_TARGET_ID),
            LOCAL_TARGET_ID,
            NOW,
        )
        .expect("同步两个 Harness 的清单");
        let db = Arc::new(Mutex::new(db));

        let runtime = TerminalRuntime::new(
            Arc::clone(&db),
            Arc::new(registry),
            Arc::clone(&backend) as Arc<dyn PtyBackend>,
        );

        Self {
            db,
            runtime,
            backend,
            codex: Arc::new(Recorder::default()),
            claude: Arc::new(Recorder::default()),
            codex_installation: installation_id("codex", LOCAL_TARGET_ID),
            claude_installation: installation_id("claude", LOCAL_TARGET_ID),
        }
    }

    fn start(
        &self,
        installation: &str,
        cwd: &str,
        cols: u16,
        rows: u16,
        sink: &Arc<Recorder>,
    ) -> String {
        self.runtime
            .start(installation, Some(cwd), cols, rows, Some(sink.emitter()))
            .expect("启动会话")
            .hub_session_id
    }

    fn start_codex(&self) -> String {
        self.start(&self.codex_installation, CODEX_CWD, 100, 30, &self.codex)
    }

    fn start_claude(&self) -> String {
        self.start(&self.claude_installation, CLAUDE_CWD, 120, 32, &self.claude)
    }

    fn record(&self, session_id: &str) -> crate::session::SessionRecord {
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

    fn converge(&self, reason: TerminationReason) -> usize {
        let guard = self.db.lock().expect("db");
        SessionService::new(guard.connection())
            .reconcile_orphans(reason)
            .expect("收敛遗留 running")
    }

    fn wait_terminal(&self, session_id: &str) -> crate::session::SessionRecord {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let record = self.record(session_id);
            if record.status != SessionStatus::Running {
                return record;
            }
            assert!(
                Instant::now() < deadline,
                "会话 {session_id} 没有在预期时间内进入终态"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn writes_for(&self, session_id: &str) -> Vec<Vec<u8>> {
        self.backend
            .written
            .lock()
            .expect("written")
            .iter()
            .filter(|(session, _)| session == session_id)
            .map(|(_, bytes)| bytes.clone())
            .collect()
    }

    /// 写入是**异步**的（8A：每会话 writer worker + 有界队列）：
    /// 断言 backend 记录之前必须等它排空，不能 sleep 猜。
    ///
    /// 这同时**加强**了下面的负向断言：等到「claude 的批次已经被投递到某处」之后，
    /// 才有把握说 codex 那边什么都没收到。
    fn wait_for_writes(&self, session_id: &str, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.writes_for(session_id).len() >= expected {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("会话 {session_id} 在 5 秒内没有把 {expected} 次写入交给 backend");
    }

    fn resizes_for(&self, session_id: &str) -> Vec<(u16, u16)> {
        self.backend
            .resized
            .lock()
            .expect("resized")
            .iter()
            .filter(|(session, _, _)| session == session_id)
            .map(|(_, cols, rows)| (*cols, *rows))
            .collect()
    }

    fn spawn_request_for(&self, session_id: &str) -> crate::pty::backend::PtySpawnRequest {
        self.backend
            .spawned
            .lock()
            .expect("spawned")
            .iter()
            .find(|request| request.session_id == session_id)
            .cloned()
            .expect("每个会话都必须有 spawn 记录")
    }
}

/// 等某条会话的**自己的** stream 里出现 marker（reader 线程是异步的）。
fn wait_for_marker(session_id: &str, sink: &Arc<Recorder>, marker: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if sink.saw_marker(session_id, marker) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "会话 {session_id} 在 5 秒内没有收到自己的 marker {:?}（收到的字节：{:?}）",
        String::from_utf8_lossy(marker),
        String::from_utf8_lossy(&sink.bytes(session_id))
    );
}

/// 两个会话、两条安装、两个 cwd、两个 pid **同时**成立。
#[test]
fn both_harnesses_run_at_once_with_distinct_sessions_pids_and_rows() {
    let fixture = concurrent_fixture();

    let codex = fixture.start_codex();
    let claude = fixture.start_claude();

    assert_ne!(codex, claude, "两条会话不得共用 hub_session_id");

    let codex_row = fixture.record(&codex);
    let claude_row = fixture.record(&claude);

    assert_eq!(codex_row.status, SessionStatus::Running);
    assert_eq!(claude_row.status, SessionStatus::Running);
    assert_eq!(codex_row.harness_id, "codex");
    assert_eq!(claude_row.harness_id, "claude");
    assert_eq!(codex_row.installation_id.as_deref(), Some("codex@local"));
    assert_eq!(claude_row.installation_id.as_deref(), Some("claude@local"));
    assert_ne!(
        codex_row.installation_id, claude_row.installation_id,
        "installation 必须各自正确"
    );
    assert_eq!(codex_row.cwd.as_deref(), Some(CODEX_CWD));
    assert_eq!(claude_row.cwd.as_deref(), Some(CLAUDE_CWD));
    assert_ne!(codex_row.cwd, claude_row.cwd, "两条会话必须在不同 cwd 上");

    assert_eq!(codex_row.pid, Some(CODEX_PID as i32));
    assert_eq!(claude_row.pid, Some(CLAUDE_PID as i32));
    assert_ne!(codex_row.pid, claude_row.pid, "两条会话必须有不同的 pid");

    // 两个进程**同时**存活
    assert!(fixture.runtime.is_running(&codex).expect("查询 codex"));
    assert!(fixture.runtime.is_running(&claude).expect("查询 claude"));
    assert_eq!(fixture.running().len(), 2, "两条都必须同时是 running");

    // spawn 时就带上了各自的 cwd / 尺寸（不是事后猜的）
    let codex_spawn = fixture.spawn_request_for(&codex);
    assert_eq!(codex_spawn.spec.program, PathBuf::from(CODEX_PROGRAM));
    assert_eq!(codex_spawn.spec.cwd, Some(PathBuf::from(CODEX_CWD)));
    assert_eq!((codex_spawn.cols, codex_spawn.rows), (100, 30));

    let claude_spawn = fixture.spawn_request_for(&claude);
    assert_eq!(claude_spawn.spec.program, PathBuf::from(CLAUDE_PROGRAM));
    assert_eq!(claude_spawn.spec.cwd, Some(PathBuf::from(CLAUDE_CWD)));
    assert_eq!((claude_spawn.cols, claude_spawn.rows), (120, 32));
}

/// 输出隔离：每条会话的 stream **只**含自己的 marker，emitter 也只见自己的 session_id。
#[test]
fn each_session_only_ever_receives_its_own_output() {
    let fixture = concurrent_fixture();

    let codex = fixture.start_codex();
    let claude = fixture.start_claude();

    wait_for_marker(&codex, &fixture.codex, CODEX_MARKER);
    wait_for_marker(&claude, &fixture.claude, CLAUDE_MARKER);

    // 正向：自己的 marker 必须到
    assert!(fixture.codex.saw_marker(&codex, CODEX_MARKER));
    assert!(fixture.claude.saw_marker(&claude, CLAUDE_MARKER));

    // 反向：对方的 marker 一个字都不许到
    assert!(
        !fixture.codex.saw_marker(&codex, CLAUDE_MARKER),
        "codex stream 里出现了 Claude 的 marker：{:?}",
        String::from_utf8_lossy(&fixture.codex.bytes(&codex))
    );
    assert!(
        !fixture.claude.saw_marker(&claude, CODEX_MARKER),
        "claude stream 里出现了 Codex 的 marker：{:?}",
        String::from_utf8_lossy(&fixture.claude.bytes(&claude))
    );

    // emitter 分发本身也必须按会话隔离
    assert_eq!(
        fixture.codex.session_ids(),
        BTreeSet::from([codex.clone()]),
        "codex 的 emitter 只应见过 codex 的 session_id"
    );
    assert_eq!(
        fixture.claude.session_ids(),
        BTreeSet::from([claude.clone()]),
        "claude 的 emitter 只应见过 claude 的 session_id"
    );
}

/// 输入 / resize 隔离：fake 精确记录 `(session_id, cols, rows)`。
#[test]
fn input_and_resize_reach_only_the_addressed_session() {
    let fixture = concurrent_fixture();

    let codex = fixture.start_codex();
    let claude = fixture.start_claude();

    fixture
        .runtime
        .write(&claude, b"CLAUDE_INPUT")
        .expect("写入 claude");
    fixture
        .runtime
        .resize(&claude, 90, 20)
        .expect("resize claude");
    fixture.wait_for_writes(&claude, 1);

    assert_eq!(fixture.writes_for(&claude), vec![b"CLAUDE_INPUT".to_vec()]);
    assert!(
        fixture.writes_for(&codex).is_empty(),
        "写 claude 不得把字节投到 codex 的 PTY"
    );
    assert_eq!(fixture.resizes_for(&claude), vec![(90, 20)]);
    assert!(
        fixture.resizes_for(&codex).is_empty(),
        "resize claude 不得改 codex 的 PTY"
    );

    fixture
        .runtime
        .write(&codex, b"CODEX_INPUT")
        .expect("写入 codex");
    fixture
        .runtime
        .resize(&codex, 101, 31)
        .expect("resize codex");
    fixture.wait_for_writes(&codex, 1);

    assert_eq!(fixture.writes_for(&codex), vec![b"CODEX_INPUT".to_vec()]);
    assert_eq!(fixture.writes_for(&claude), vec![b"CLAUDE_INPUT".to_vec()]);
    assert_eq!(
        fixture.resizes_for(&claude),
        vec![(90, 20)],
        "codex 的 resize 不得混进 claude 的记录"
    );

    // 后端侧的原始记录：每条 resize 都带着自己的 session_id
    assert_eq!(
        fixture.backend.resized.lock().expect("resized").clone(),
        vec![(claude.clone(), 90, 20), (codex.clone(), 101, 31)]
    );
}

/// kill 顺序 A：先结束 Claude → Codex 仍 running，且仍能 write / resize / 继续产出。
#[test]
fn killing_claude_leaves_codex_running_and_still_usable() {
    let fixture = concurrent_fixture();

    let codex = fixture.start_codex();
    let claude = fixture.start_claude();
    wait_for_marker(&codex, &fixture.codex, CODEX_MARKER);
    wait_for_marker(&claude, &fixture.claude, CLAUDE_MARKER);

    fixture.runtime.kill(&claude).expect("kill claude");

    let finished = fixture.wait_terminal(&claude);
    assert_eq!(finished.status, SessionStatus::Exited);
    assert_eq!(
        finished.termination_reason,
        Some(TerminationReason::UserKilled)
    );
    assert_eq!(
        finished.exit_code,
        Some(137),
        "退出码来自 reaper 的真实观测"
    );

    // Codex 完全不受影响
    assert_eq!(fixture.record(&codex).status, SessionStatus::Running);
    assert!(fixture.runtime.is_running(&codex).expect("查询 codex"));
    assert_eq!(
        fixture.running(),
        vec![codex.clone()],
        "只应剩 codex 一条 running"
    );
    assert!(
        !fixture.codex.saw_marker(&codex, CLAUDE_MARKER),
        "结束 claude 不得把它的输出漏进 codex"
    );
    assert!(
        fixture
            .claude
            .session_ids()
            .iter()
            .all(|session| session == &claude),
        "claude 的 emitter 不得收到 codex 的事件"
    );

    // Codex 的 PTY 仍然可用：write / resize / 继续输出
    fixture
        .runtime
        .write(&codex, b"STILL_ALIVE\r")
        .expect("codex 仍可写入");
    fixture
        .runtime
        .resize(&codex, 133, 45)
        .expect("codex 仍可 resize");
    fixture
        .backend
        .push_output(CODEX_PROGRAM, b"CODEX_AFTER_CLAUDE_DIED\r\n".to_vec());
    wait_for_marker(&codex, &fixture.codex, b"CODEX_AFTER_CLAUDE_DIED");
    assert!(
        !fixture
            .claude
            .saw_marker(&claude, b"CODEX_AFTER_CLAUDE_DIED"),
        "claude 已经结束，绝不能再收到 codex 的后续输出"
    );
    assert_eq!(
        fixture.record(&codex).status,
        SessionStatus::Running,
        "kill claude 不得改变 codex 的状态"
    );

    // 收尾：kill codex → running = 0
    fixture.runtime.kill(&codex).expect("kill codex");
    let finished_codex = fixture.wait_terminal(&codex);
    assert_eq!(
        finished_codex.termination_reason,
        Some(TerminationReason::UserKilled)
    );
    assert!(fixture.running().is_empty());
}

/// kill 顺序 B（反向）：先结束 Codex → Claude 仍健康，且仍能继续产出。
#[test]
fn killing_codex_leaves_claude_running_and_still_usable() {
    let fixture = concurrent_fixture();

    let codex = fixture.start_codex();
    let claude = fixture.start_claude();
    wait_for_marker(&codex, &fixture.codex, CODEX_MARKER);
    wait_for_marker(&claude, &fixture.claude, CLAUDE_MARKER);

    fixture.runtime.kill(&codex).expect("kill codex");

    let finished = fixture.wait_terminal(&codex);
    assert_eq!(finished.status, SessionStatus::Exited);
    assert_eq!(
        finished.termination_reason,
        Some(TerminationReason::UserKilled)
    );
    assert_eq!(finished.exit_code, Some(137));

    assert_eq!(
        fixture.record(&claude).status,
        SessionStatus::Running,
        "kill codex 不得影响 claude"
    );
    assert_eq!(fixture.running(), vec![claude.clone()]);
    assert!(
        !fixture.claude.saw_marker(&claude, CODEX_MARKER),
        "结束 codex 不得把它的输出漏进 claude"
    );

    fixture
        .backend
        .push_output(CLAUDE_PROGRAM, b"CLAUDE_AFTER_CODEX_DIED\r\n".to_vec());
    wait_for_marker(&claude, &fixture.claude, b"CLAUDE_AFTER_CODEX_DIED");
    fixture
        .runtime
        .resize(&claude, 111, 40)
        .expect("claude 仍可 resize");
    assert_eq!(fixture.resizes_for(&claude), vec![(111, 40)]);
    assert!(fixture.resizes_for(&codex).is_empty());

    fixture.runtime.kill(&claude).expect("kill claude");
    let finished_claude = fixture.wait_terminal(&claude);
    assert_eq!(
        finished_claude.termination_reason,
        Some(TerminationReason::UserKilled)
    );
    assert!(fixture.running().is_empty());
}

/// DB lifecycle 独立：退出码、终态、原因、结束时间各自落库，互不覆盖。
#[test]
fn each_session_records_its_own_terminal_state() {
    let fixture = concurrent_fixture();

    let codex = fixture.start_codex();
    let claude = fixture.start_claude();
    wait_for_marker(&codex, &fixture.codex, CODEX_MARKER);
    wait_for_marker(&claude, &fixture.claude, CLAUDE_MARKER);

    // Claude 自然退出（0）
    fixture.backend.close_output(CLAUDE_PROGRAM);
    fixture.backend.exit_session(&claude, Some(0));
    let claude_row = fixture.wait_terminal(&claude);
    assert_eq!(claude_row.status, SessionStatus::Exited);
    assert_eq!(
        claude_row.termination_reason,
        Some(TerminationReason::NaturalExit)
    );
    assert_eq!(claude_row.exit_code, Some(0));
    let claude_ended_at = claude_row.ended_at.clone();
    assert!(claude_ended_at.is_some(), "终态必须写结束时间");

    // Codex 那一行仍未被碰过
    let codex_row = fixture.record(&codex);
    assert_eq!(codex_row.status, SessionStatus::Running);
    assert!(codex_row.ended_at.is_none());
    assert_eq!(codex_row.exit_code, None);
    assert_eq!(codex_row.termination_reason, None);

    // Codex 以非零码自然退出 → 自己那行 failed，且不覆盖 Claude 的 0
    fixture.backend.close_output(CODEX_PROGRAM);
    fixture.backend.exit_session(&codex, Some(3));
    let codex_row = fixture.wait_terminal(&codex);
    assert_eq!(codex_row.status, SessionStatus::Failed);
    assert_eq!(codex_row.exit_code, Some(3));
    assert_eq!(
        codex_row.termination_reason,
        Some(TerminationReason::NaturalExit)
    );

    let claude_after = fixture.record(&claude);
    assert_eq!(
        claude_after.exit_code,
        Some(0),
        "codex 的退出码不得覆盖 claude"
    );
    assert_eq!(claude_after.status, SessionStatus::Exited);
    assert_eq!(claude_after.ended_at, claude_ended_at, "结束时间不得被改写");
    assert!(fixture.running().is_empty());
}

/// 强杀后重启：**两条**旧 running 都要收敛，且不得混成一条。
#[test]
fn restart_converges_every_running_row_not_just_the_first() {
    let fixture = concurrent_fixture();

    let codex = fixture.start_codex();
    let claude = fixture.start_claude();
    assert_eq!(
        fixture.running().len(),
        2,
        "模拟强杀前：库里必须同时有两条 running"
    );

    // 应用强杀后重启走的就是这个生产函数（lib.rs 的启动收敛）。
    assert_eq!(
        fixture.converge(TerminationReason::Lost),
        2,
        "两条遗留 running 都必须被收敛，而不是只处理第一行"
    );

    for session_id in [&codex, &claude] {
        let row = fixture.record(session_id);
        assert_eq!(row.status, SessionStatus::Unknown, "绝不恢复成 running");
        assert_eq!(row.termination_reason, Some(TerminationReason::Lost));
        assert!(row.ended_at.is_some(), "收敛时必须写结束时间");
        assert!(row.pid.is_some(), "PID 保留作为诊断线索");
    }

    assert!(fixture.running().is_empty(), "不得留下 ghost running");
    assert_eq!(
        fixture.converge(TerminationReason::Lost),
        0,
        "幂等：已经没有 running 可收敛"
    );
    assert_eq!(
        fixture.runtime.list_sessions(10).expect("列出").len(),
        2,
        "收敛不得把两条会话合并或删除"
    );
}
