//! Task 8B 真机 Gate（R1–R4）。直接驱动生产 `PtyManager` + `PortablePtyBackend`，
//! **不经过** Harness 适配器，也不改任何 Core 代码。
//!
//! 冻结语义（`docs/specs/2026-09-24-task8b-process-tree-ownership-design.md` §3）：
//! Session 拥有的是**进程树**，不是一个 PID。`kill` = `terminate_tree`。
//!
//! ## 四条刻意设计（都是被真机打出来的）
//!
//! 1. **进程观测独立于产品代码**：PID 存活用 `tasklist` 查，后代用 ToolHelp 快照自己走一遍，
//!    不调用 `pty::containment` 的产品函数 —— 否则「实现坏了」和「测试坏了」分不开。
//! 2. **test-only 终端模拟器**：conhost 因 `PSUEDOCONSOLE_INHERIT_CURSOR` 会发 `ESC[6n`
//!    并**等应答**，不应答时子进程连首屏都过不去（也拿不到后代）。应答按会话计数、
//!    每会话最多 [`MAX_DSR_REPLIES_PER_SESSION`] 次：按历史请求数无限重放会把子进程输入
//!    缓冲区灌满（7D 第一次真机运行正是这么挂死的）。
//! 3. **R4 是永久回归**：Codex A + Codex B + Claude C 同时跑 → kill A → 只有 A 的树死。
//!    A/B **同一个可执行文件**是关键：错误的 executable-scoped 清理（例如「杀掉所有 node」）
//!    也能让「零残留」看起来成立，只有这条能把它抓出来。
//! 4. **R2/R3 是支撑证据，R4 是不可跳过的 Gate**：R2/R3 缺 binary 时明确跳过；
//!    R4 缺 codex/claude 直接 panic（跳过 ≠ 通过）。

#![cfg(windows)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use harness_hub_lib::harness::launch::LaunchSpec;
use harness_hub_lib::pty::{
    OutputSink, PortablePtyBackend, PtyBackend, PtyManager, PtyProcessHandle, PtySpawnRequest,
};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};

const CODEX: &str = r"D:\npm-global\codex.cmd";
const CLAUDE: &str = r"D:\npm-global\claude.cmd";
const CODEX_CWD: &str = r"D:\HarnessHub-E2E\codex-concurrent";
const CLAUDE_CWD: &str = r"D:\HarnessHub-E2E\claude-concurrent";

const DSR_REQUEST: &[u8] = b"\x1b[6n";
const DSR_REPLY: &[u8] = b"\x1b[1;1R";
const MAX_DSR_REPLIES_PER_SESSION: usize = 3;

/// 每个用例都会起真实 Harness；串行执行，避免多个 node 互抢 CPU 造成假超时
/// （cargo 默认并行跑同一个 test binary 里的用例）。
static SERIAL: Mutex<()> = Mutex::new(());

fn serialize() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 输出汇聚：只看字节，不做任何解析（ADR-0009：解析属于终端模拟器）。
#[derive(Default)]
struct Streams {
    chunks: Mutex<Vec<(String, Vec<u8>)>>,
}

impl Streams {
    fn sink(self: &Arc<Self>) -> OutputSink {
        let streams = Arc::clone(self);
        Arc::new(move |session: &str, _seq: u64, bytes: &[u8]| {
            streams
                .lock_chunks()
                .push((session.to_string(), bytes.to_vec()));
        })
    }

    fn lock_chunks(&self) -> std::sync::MutexGuard<'_, Vec<(String, Vec<u8>)>> {
        self.chunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn take(&self) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut *self.lock_chunks())
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

/// 会话集合：manager + 输出汇聚 + 每会话已应答次数。
struct Harness {
    manager: Arc<PtyManager>,
    streams: Arc<Streams>,
    answered: HashMap<String, usize>,
}

impl Harness {
    fn new() -> Self {
        let streams = Arc::new(Streams::default());
        let manager = Arc::new(PtyManager::with_input_capacity(
            Arc::new(PortablePtyBackend::new()),
            streams.sink(),
            Arc::new(|_session, _code| {}),
            64 * 1024,
        ));
        Self {
            manager,
            streams,
            answered: HashMap::new(),
        }
    }

    fn spawn(&mut self, session: &str, program: &str, args: &[&str], cwd: Option<&str>) -> u32 {
        let spec = LaunchSpec {
            program: PathBuf::from(program),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            cwd: cwd.map(PathBuf::from),
            env: Vec::new(),
            runtime_target_id: "local".to_string(),
        };
        let handle = self.manager.spawn(session, spec, 80, 24).expect("spawn");
        self.manager.start_reading(session).expect("start_reading");
        handle.pid.expect("spawn 必须给出 pid")
    }

    /// test-only 终端模拟器：看到 `ESC[6n` 就回 `ESC[1;1R`，每会话有上限。
    ///
    /// `PtyManager::write` 在 8A 之后只是**入队**（不会阻塞主线程），所以这里可以直接调。
    fn pump(&mut self) {
        let chunks = self.streams.take();
        for (session, bytes) in chunks {
            let requested = count_occurrences(&bytes, DSR_REQUEST);
            let answered = self.answered.entry(session.clone()).or_default();
            while *answered < requested && *answered < MAX_DSR_REPLIES_PER_SESSION {
                self.manager.write(&session, DSR_REPLY).expect("应答 DSR");
                *answered += 1;
            }
        }
    }

    /// 等到 root 的后代节点数达到 `min_nodes`（含 root），期间持续应答 DSR。
    fn wait_tree(&mut self, root: u32, min_nodes: usize, timeout: Duration) -> Vec<u32> {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump();
            let mut nodes = vec![root];
            nodes.extend(descendants(root));
            if nodes.len() >= min_nodes || Instant::now() >= deadline {
                return nodes;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn kill(&mut self, session: &str) {
        self.manager.kill(session).expect("kill");
    }
}

/// 进程是否还活着 —— 问 OS，不问 portable-pty（「我们以为它还活着」不是证据）。
fn pid_alive(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

/// 独立的后代枚举（ToolHelp 快照 + 自己 BFS），**不复用产品实现**。
///
/// 父进程死掉之后 `th32ParentProcessID` 仍是记录值，所以「shim 已被 kill、后代的
/// ppid 指向已死的 shim」这种情况依然枚举得到 —— 这正是本 Task 要抓的形状。
fn descendants(root: u32) -> Vec<u32> {
    let mut all: Vec<(u32, u32)> = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
            return Vec::new();
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                all.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }

    let mut result = Vec::new();
    let mut queue = vec![root];
    while let Some(parent) = queue.pop() {
        for (pid, ppid) in &all {
            if *ppid == parent && *pid != root && !result.contains(pid) {
                result.push(*pid);
                queue.push(*pid);
            }
        }
    }
    result
}

fn wait_dead(pids: &[u32], timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if pids.iter().all(|pid| !pid_alive(*pid)) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn survivors(pids: &[u32]) -> Vec<u32> {
    pids.iter().copied().filter(|pid| pid_alive(*pid)).collect()
}

/// 收尾：残余进程一律清掉，别给后面的用例留垃圾。
fn cleanup(pids: &[u32]) {
    for pid in pids {
        if pid_alive(*pid) {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output();
        }
    }
}

// ---------------------------------------------------------------------------
// 判别性 Gate：直接驱动生产 transport，**不经过 reaper 的 forget**
// ---------------------------------------------------------------------------
//
// 上面 R1–R4 走 `PtyManager::kill`，而 `kill` 之后 reaper 会观测 root 退出、写终态、再
// `forget`（关 console + 释放 Job 句柄）。实测（变异验证）把 `terminate_tree` 退回
// direct-child kill 之后 R1–R4 **照样通过** —— 因为后代会死在 `forget` 那一步。
// 所以「kill 之后没有残留」这条**不足以**证明 `terminate_tree` 真的终结了整棵树。
//
// 下面两条把 `forget` 从等式中拿掉：只用生产 `PortablePtyBackend` 起会话，
// 然后调 `terminate_tree`，**不做任何 release / forget**。退化成 direct-child kill 时必失败。

/// 直接用 `PortablePtyBackend` 起会话（自带 test-only 终端模拟器）。
struct RawHarness {
    backend: Arc<PortablePtyBackend>,
    answered: Arc<Mutex<HashMap<String, usize>>>,
}

impl RawHarness {
    fn new() -> Self {
        Self {
            backend: Arc::new(PortablePtyBackend::new()),
            answered: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn spawn(&self, session: &str, program: &str, args: &[&str], cwd: Option<&str>) -> u32 {
        let handle: PtyProcessHandle = self
            .backend
            .spawn(PtySpawnRequest {
                session_id: session.to_string(),
                spec: LaunchSpec {
                    program: PathBuf::from(program),
                    args: args.iter().map(|arg| (*arg).to_string()).collect(),
                    cwd: cwd.map(PathBuf::from),
                    env: Vec::new(),
                    runtime_target_id: "local".to_string(),
                },
                cols: 80,
                rows: 24,
            })
            .expect("spawn");

        let mut reader = self.backend.take_reader(session).expect("reader");
        let backend = Arc::clone(&self.backend);
        let answered = Arc::clone(&self.answered);
        let id = session.to_string();
        std::thread::spawn(move || {
            let mut sink = [0u8; 8192];
            loop {
                match reader.read(&mut sink) {
                    Ok(0) => break,
                    Ok(read) => {
                        let requested = count_occurrences(&sink[..read], DSR_REQUEST);
                        let mut counts = answered.lock().unwrap_or_else(|p| p.into_inner());
                        let done = counts.entry(id.clone()).or_default();
                        while *done < requested && *done < MAX_DSR_REPLIES_PER_SESSION {
                            let _ = backend.write(&id, DSR_REPLY);
                            *done += 1;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        handle.pid.expect("spawn 必须给出 pid")
    }

    /// 等后代出现（无 reaper、无输出 sinks，纯 PTY）。
    fn wait_tree(&self, root: u32, min_nodes: usize, timeout: Duration) -> Vec<u32> {
        let deadline = Instant::now() + timeout;
        loop {
            let mut nodes = vec![root];
            nodes.extend(descendants(root));
            if nodes.len() >= min_nodes || Instant::now() >= deadline {
                return nodes;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// R1/raw：合成三层树 —— 只调 `terminate_tree`（**没有** forget / 没有关 console），全树必须消失。
///
/// 这是「Session 拥有进程树」最直接的判别性证据：退化实现（只杀直接子进程）在这里必失败
/// （实测：退回 direct-child kill 时本用例报 `仍在跑：[50500, 30040]`）。
#[test]
fn terminate_tree_kills_a_synthetic_three_level_tree_without_any_release() {
    let _serial = serialize();
    let harness = RawHarness::new();
    let root = harness.spawn(
        "hub-raw-tree",
        "cmd",
        &["/c", "cmd /c ping -n 300 127.0.0.1"],
        None,
    );

    let tree = harness.wait_tree(root, 3, Duration::from_secs(20));
    eprintln!("[R1/raw] tree = {tree:?}");
    assert!(tree.len() >= 3, "必须是三层树，实际 {tree:?}");

    harness
        .backend
        .terminate_tree("hub-raw-tree")
        .expect("terminate_tree");

    assert!(
        wait_dead(&tree, Duration::from_secs(15)),
        "terminate_tree 必须终结整棵树（这里没有任何 forget 兜底），仍在跑：{:?}",
        survivors(&tree)
    );
    cleanup(&tree);
}

/// R4/raw（永久回归，判别性最强）：Codex A + Codex B + Claude C → `terminate_tree(A)`
/// → 只有 A 的树死；**不经过** reaper 的 forget，所以「关 console + 释放 Job」这条路径
/// 不可能让它通过。
///
/// A/B 是**同一个可执行文件**：executable-scoped 的错误清理也能让「A 零残留」成立，
/// 只有 B/C 存活的断言能把它抓出来。环境缺失不算通过（缺 codex/claude 直接 panic）。
#[test]
fn terminate_tree_scopes_to_one_owned_tree_without_any_release() {
    let _serial = serialize();
    assert!(
        Path::new(CODEX).is_file() && Path::new(CLAUDE).is_file(),
        "本 Gate 需要真机 codex 与 claude（跳过不等于通过）：{CODEX} / {CLAUDE}"
    );

    let harness = RawHarness::new();
    let codex_cwd = if Path::new(CODEX_CWD).is_dir() {
        Some(CODEX_CWD)
    } else {
        None
    };
    let claude_cwd = if Path::new(CLAUDE_CWD).is_dir() {
        Some(CLAUDE_CWD)
    } else {
        None
    };

    let a = harness.spawn("hub-raw-a", CODEX, &[], codex_cwd);
    let b = harness.spawn("hub-raw-b", CODEX, &[], codex_cwd);
    let c = harness.spawn("hub-raw-c", CLAUDE, &[], claude_cwd);

    let tree_a = harness.wait_tree(a, 3, Duration::from_secs(30));
    let tree_b = harness.wait_tree(b, 3, Duration::from_secs(30));
    let tree_c = harness.wait_tree(c, 2, Duration::from_secs(30));
    eprintln!("[R4/raw] A={tree_a:?} B={tree_b:?} C={tree_c:?}");
    assert!(tree_a.len() >= 3 && tree_b.len() >= 3, "A/B 都必须是三层树");
    assert!(tree_c.len() >= 2, "C 至少两层");

    harness
        .backend
        .terminate_tree("hub-raw-a")
        .expect("terminate_tree A");

    assert!(
        wait_dead(&tree_a, Duration::from_secs(15)),
        "A 的全树必须死（没有 forget 兜底），仍在跑：{:?}",
        survivors(&tree_a)
    );
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(
        survivors(&tree_b).len(),
        tree_b.len(),
        "B 的整棵树必须存活（A/B 同名可执行文件），已死的：{:?}",
        tree_b
            .iter()
            .copied()
            .filter(|pid| !pid_alive(*pid))
            .collect::<Vec<u32>>()
    );
    assert_eq!(
        survivors(&tree_c).len(),
        tree_c.len(),
        "C 的整棵树必须存活，已死的：{:?}",
        tree_c
            .iter()
            .copied()
            .filter(|pid| !pid_alive(*pid))
            .collect::<Vec<u32>>()
    );

    harness.backend.terminate_tree("hub-raw-b").ok();
    harness.backend.terminate_tree("hub-raw-c").ok();
    std::thread::sleep(Duration::from_millis(500));
    cleanup(&tree_a);
    cleanup(&tree_b);
    cleanup(&tree_c);
}

/// R1：合成 parent → child → grandchild → kill → 三层 PID 全部消失。
#[test]
fn synthetic_three_level_tree_is_fully_terminated() {
    let _serial = serialize();
    let mut harness = Harness::new();
    let root = harness.spawn(
        "hub-tree",
        "cmd",
        &["/c", "cmd /c ping -n 300 127.0.0.1"],
        None,
    );

    let tree = harness.wait_tree(root, 3, Duration::from_secs(20));
    eprintln!("[R1] tree = {tree:?}");
    assert!(tree.len() >= 3, "必须是三层树，实际 {tree:?}");

    harness.kill("hub-tree");
    assert!(
        wait_dead(&tree, Duration::from_secs(15)),
        "kill 之后全树必须消失，仍在跑：{:?}",
        survivors(&tree)
    );
    cleanup(&tree);
}

/// R2/R3：真实 `codex.cmd` → `cmd.exe → node.exe → codex.exe`、`claude.cmd` → `cmd.exe → claude.exe`。
///
/// **支撑证据**：本机缺 binary 时明确跳过（不算 Gate 通过）。
#[test]
fn real_codex_and_claude_trees_are_fully_terminated() {
    let _serial = serialize();

    for (session, program, cwd, min_nodes) in [
        ("hub-codex", CODEX, CODEX_CWD, 3),
        ("hub-claude", CLAUDE, CLAUDE_CWD, 2),
    ] {
        if !Path::new(program).is_file() {
            eprintln!("[R2/R3] 跳过 {session}：本机没有 {program}");
            continue;
        }
        let mut harness = Harness::new();
        let cwd = if Path::new(cwd).is_dir() {
            Some(cwd)
        } else {
            None
        };
        let root = harness.spawn(session, program, &[], cwd);

        let tree = harness.wait_tree(root, min_nodes, Duration::from_secs(30));
        eprintln!("[R2/R3] {session} tree = {tree:?}");
        assert!(
            tree.len() >= min_nodes,
            "{session} 的树节点数不足（期望 ≥ {min_nodes}）：{tree:?}"
        );

        harness.kill(session);
        assert!(
            wait_dead(&tree, Duration::from_secs(15)),
            "{session} 全树必须消失，仍在跑：{:?}",
            survivors(&tree)
        );
        cleanup(&tree);
    }
}

/// R4（永久回归，最重要）：Codex A + Codex B + Claude C 同时跑 → kill A → 只有 A 的树死。
///
/// A 与 B **必须是同一个可执行文件**：executable-scoped 的错误清理也能让「零残留」成立，
/// 只有这条能把它抓出来。环境缺失**不算通过**（Gate）：缺 codex 或 claude 直接 panic。
#[test]
fn killing_one_session_tree_never_touches_another() {
    let _serial = serialize();
    assert!(
        Path::new(CODEX).is_file() && Path::new(CLAUDE).is_file(),
        "本 Gate 需要真机 codex 与 claude（跳过不等于通过）：{CODEX} / {CLAUDE}"
    );

    let mut harness = Harness::new();
    let codex_cwd = if Path::new(CODEX_CWD).is_dir() {
        Some(CODEX_CWD)
    } else {
        None
    };
    let claude_cwd = if Path::new(CLAUDE_CWD).is_dir() {
        Some(CLAUDE_CWD)
    } else {
        None
    };

    let a = harness.spawn("hub-a", CODEX, &[], codex_cwd);
    let b = harness.spawn("hub-b", CODEX, &[], codex_cwd);
    let c = harness.spawn("hub-c", CLAUDE, &[], claude_cwd);

    let tree_a = harness.wait_tree(a, 3, Duration::from_secs(30));
    let tree_b = harness.wait_tree(b, 3, Duration::from_secs(30));
    let tree_c = harness.wait_tree(c, 2, Duration::from_secs(30));
    eprintln!("[R4] A={tree_a:?} B={tree_b:?} C={tree_c:?}");
    assert!(tree_a.len() >= 3 && tree_b.len() >= 3, "A/B 都必须是三层树");
    assert!(tree_c.len() >= 2, "C 至少两层");

    harness.kill("hub-a");
    assert!(
        wait_dead(&tree_a, Duration::from_secs(15)),
        "A 的全树必须死，仍在跑：{:?}",
        survivors(&tree_a)
    );
    // 给 OS 一点时间把「误伤」暴露出来：B/C 里**任何一个**消失都是 session-scoped 失败。
    std::thread::sleep(Duration::from_millis(1000));
    let alive_b = survivors(&tree_b);
    let alive_c = survivors(&tree_c);
    assert_eq!(
        alive_b.len(),
        tree_b.len(),
        "B 的整棵树必须存活（A/B 同名可执行文件），已死的：{:?}",
        tree_b
            .iter()
            .copied()
            .filter(|pid| !pid_alive(*pid))
            .collect::<Vec<u32>>()
    );
    assert_eq!(
        alive_c.len(),
        tree_c.len(),
        "C 的整棵树必须存活，已死的：{:?}",
        tree_c
            .iter()
            .copied()
            .filter(|pid| !pid_alive(*pid))
            .collect::<Vec<u32>>()
    );

    harness.kill("hub-b");
    harness.kill("hub-c");
    std::thread::sleep(Duration::from_millis(500));
    cleanup(&tree_a);
    cleanup(&tree_b);
    cleanup(&tree_c);
}
