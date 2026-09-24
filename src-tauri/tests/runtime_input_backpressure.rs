//! Task 8A 真机验收：**故意永不读 stdin 的 synthetic child**。
//!
//! 直接驱动生产 `PtyManager` + `PortablePtyBackend`（不经过 Codex/Claude，也不经过
//! `TerminalRuntime`），因为冻结点就在这一层。
//!
//! 合成 child 直接用 `ping -n 120 127.0.0.1`：
//! - ping **不读 stdin**，且会一直跑（120 秒），足以撑住整段断言；
//! - 不经过 `cmd /c`，因此**没有**孙进程，收尾用一次定向树杀就能清干净。
//!
//! 测试**自证**：若 child 其实会读 stdin，in-flight 批次会被结算、背压不会出现 → 用例失败。

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use harness_hub_lib::error::Error;
use harness_hub_lib::harness::launch::LaunchSpec;
use harness_hub_lib::pty::{PortablePtyBackend, PtyManager};

/// 容量必须**明显大于 ConPTY 的输入缓冲区**，否则 4 KiB/64 KiB 那种量级的批次会被
/// OS 一次性吞掉，`write_all` 根本不阻塞，这条测试就测不到「park 在 OS 写里」。
const CAPACITY: usize = 4 * 1024 * 1024;

fn maybe_reads_stdin_spec() -> LaunchSpec {
    LaunchSpec {
        program: PathBuf::from("ping"),
        args: vec!["-n".to_string(), "120".to_string(), "127.0.0.1".to_string()],
        cwd: None,
        env: Vec::new(),
        runtime_target_id: "local".to_string(),
    }
}

fn manager() -> Arc<PtyManager> {
    Arc::new(PtyManager::with_input_capacity(
        Arc::new(PortablePtyBackend::new()),
        Arc::new(|_session, _seq, _bytes| {}),
        Arc::new(|_session, _code| {}),
        CAPACITY,
    ))
}

/// 硬期限 + 实测耗时：把「Runtime 冻结」变成可断言的失败，并留下证据数字。
///
/// 必须另起线程：被冻结的调用不会因为超时自己解开（同一线程里等就变成挂死），
/// 而 `thread::scope` 退出时会 join —— 同样挂死。代价是闭包要 `Send + 'static`。
fn timed<T: Send + 'static>(
    timeout: Duration,
    label: &str,
    action: impl FnOnce() -> T + Send + 'static,
) -> (T, Duration) {
    let (sender, receiver) = std::sync::mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let _ = sender.send(action());
    });
    let value = receiver
        .recv_timeout(timeout)
        .unwrap_or_else(|_| panic!("{label} 在 {timeout:?} 内没有返回：Runtime 被冻结"));
    let elapsed = started.elapsed();
    eprintln!("[R2] {label} 返回耗时 = {elapsed:?}");
    (value, elapsed)
}

/// 对 `PtyManager` 的一次调用施加硬期限并记录耗时（闭包只借用传入的 manager）。
fn timed_manager<T: Send + 'static>(
    timeout: Duration,
    label: &str,
    manager: &Arc<PtyManager>,
    action: impl FnOnce(&PtyManager) -> T + Send + 'static,
) -> (T, Duration) {
    let manager = Arc::clone(manager);
    timed(timeout, label, move || action(&manager))
}

/// 打印一个唯一 marker 然后**自己退出**的原生 child。
///
/// 为什么不用 `cmd /c echo`：实测它在 ConPTY 下**不会退出**（`GetExitCodeProcess` 与
/// `tasklist` 都确认进程一直活着 15 秒以上），因此不适合做「自然退出」的验收。
/// 为什么用绝对路径：CreateProcess 不搜索 PATH，裸名字只对 System32 直属文件（如 ping）有效。
fn powershell_path() -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    PathBuf::from(root).join("System32\\WindowsPowerShell\\v1.0\\powershell.exe")
}

fn pid_alive(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

fn tree_kill(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .output();
}

/// R1/R2/R3：一条真实 child 永不读 stdin 时，整台 Runtime（B、以及 A 自己的控制面）都不冻结。
#[test]
fn a_real_child_that_never_reads_stdin_does_not_freeze_the_runtime() {
    let manager = manager();
    let mut pids = Vec::new();
    for session in ["hub-a", "hub-b"] {
        let handle = manager
            .spawn(session, maybe_reads_stdin_spec(), 80, 24)
            .expect("spawn");
        manager.start_reading(session).expect("start_reading");
        pids.push(handle.pid.expect("pid"));
    }

    // R1：塞满容量。首批 64 KiB 被接受 → worker 会 park 在真实 OS 写里。
    manager
        .write("hub-a", &vec![b'x'; CAPACITY])
        .expect("首批必须被接受");

    // 自证 child 不读 stdin：2 秒后 in-flight 仍未结算（被读掉就会变 0）。
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        manager.pending_bytes("hub-a"),
        Some(CAPACITY),
        "child 若在读 stdin，in-flight 批次会被结算 —— 这条断言就是在证明它不读"
    );

    let (rejected, _) = timed_manager(Duration::from_secs(2), "A 背压写入", &manager, |m| {
        m.write("hub-a", b"more")
    });
    match rejected.expect_err("队列必须满") {
        Error::InputBackpressure {
            pending_bytes,
            capacity_bytes,
            ..
        } => assert_eq!((pending_bytes, capacity_bytes), (CAPACITY, CAPACITY)),
        other => panic!("必须是 InputBackpressure，实际 {other:?}"),
    }

    // R2：A 的 writer 真 park 在 OS write 里时，B 与 A 的控制面都要在硬期限内返回。
    let (b_write, _) = timed_manager(Duration::from_secs(2), "B write", &manager, |m| {
        m.write("hub-b", b"hello")
    });
    assert!(b_write.is_ok());
    let (b_resize, _) = timed_manager(Duration::from_secs(2), "B resize", &manager, |m| {
        m.resize("hub-b", 100, 30)
    });
    assert!(b_resize.is_ok());
    let (b_kill, _) = timed_manager(Duration::from_secs(2), "B kill", &manager, |m| {
        m.kill("hub-b")
    });
    assert!(b_kill.is_ok());
    let (a_resize, _) = timed_manager(Duration::from_secs(2), "A resize", &manager, |m| {
        m.resize("hub-a", 120, 40)
    });
    assert!(a_resize.is_ok());
    let (a_kill, _) = timed_manager(Duration::from_secs(2), "A kill", &manager, |m| {
        m.kill("hub-a")
    });
    assert!(a_kill.is_ok());
    let (a_wait, _) = timed_manager(Duration::from_secs(2), "A try_wait", &manager, |m| {
        m.try_wait("hub-a")
    });
    assert!(a_wait.is_ok());

    // R3：kill 之后输入侧关闭
    assert!(matches!(
        manager.write("hub-a", b"x"),
        Err(Error::InputClosed { .. })
    ));

    // 清理：定向树杀（8A 用 taskkill /T；8B 会换成 ProcessHandle::terminate_tree）
    for pid in &pids {
        tree_kill(*pid);
    }
    std::thread::sleep(Duration::from_millis(500));
    for pid in &pids {
        assert!(!pid_alive(*pid), "synthetic child {pid} 必须被清理掉");
    }
}

/// E2 的回归锁：子进程**自己退出**（不 kill），reaper 走完「真实退出 → 终态 → 有界等待
/// reader → 释放」之后，退出前最后写出的字节不得被截断。
///
/// 没有 `READER_DRAIN_GRACE` 时这条会在某些时序下丢尾部字节 —— 这正是那个有界等待存在的理由。
///
/// 合成 child 用绝对路径的 Windows PowerShell（CreateProcess 不搜 PATH）：
/// 它会执行 `Write-Output <marker>` 然后自己退出。但它和 Claude 一样，**先发 `ESC[6n`
/// 并等待应答**（实测：只收到 `ESC[6n` 就永远不动），所以这里必须做 test-only DSR responder，
/// 否则 child 根本走不到 echo（ADR-0009：应答属于终端侧，不属于 PTY 层）。
#[test]
fn the_last_output_is_not_truncated_when_the_reaper_releases_the_session() {
    use std::sync::atomic::{AtomicBool, Ordering};

    const MARKER: &str = "HH_TAIL_MARKER_271828";
    const DSR_REQUEST: &[u8] = b"\x1b[6n";
    const DSR_REPLY: &[u8] = b"\x1b[1;1R";

    let chunks: Arc<std::sync::Mutex<Vec<u8>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let answered = Arc::new(AtomicBool::new(false));
    // 输出回调里要能回写 DSR 应答，但 manager 还没构造完 —— 用 OnceLock 回填，避免自引用。
    let manager_slot: Arc<std::sync::OnceLock<Arc<PtyManager>>> =
        Arc::new(std::sync::OnceLock::new());

    let sink = Arc::clone(&chunks);
    let answered_for_sink = Arc::clone(&answered);
    let slot_for_sink = Arc::clone(&manager_slot);
    let manager = Arc::new(PtyManager::with_input_capacity(
        Arc::new(PortablePtyBackend::new()),
        Arc::new(move |session, _seq, bytes| {
            let requested = {
                let mut collected = sink.lock().expect("chunks");
                collected.extend_from_slice(bytes);
                collected
                    .windows(DSR_REQUEST.len())
                    .any(|window| window == DSR_REQUEST)
            };
            if requested && !answered_for_sink.swap(true, Ordering::SeqCst) {
                if let Some(manager) = slot_for_sink.get() {
                    let _ = manager.write(session, DSR_REPLY);
                }
            }
        }),
        Arc::new(|_session, _code| {}),
        CAPACITY,
    ));
    // `PtyManager` 没有实现 Debug，所以不能用 `expect`（它要求 T: Debug）。
    assert!(
        manager_slot.set(Arc::clone(&manager)).is_ok(),
        "回填 manager 只能发生一次"
    );

    let handle = manager
        .spawn(
            "hub-tail",
            LaunchSpec {
                program: powershell_path(),
                args: vec![
                    "-NoProfile".to_string(),
                    "-NonInteractive".to_string(),
                    "-Command".to_string(),
                    format!("Write-Output {MARKER}"),
                ],
                cwd: None,
                env: Vec::new(),
                runtime_target_id: "local".to_string(),
            },
            80,
            24,
        )
        .expect("spawn");
    manager.start_reading("hub-tail").expect("start_reading");

    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if manager.pending_bytes("hub-tail").is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    assert_eq!(
        manager.pending_bytes("hub-tail"),
        None,
        "reaper 必须回收 handle（真实退出路径）"
    );

    let text = String::from_utf8_lossy(&chunks.lock().expect("chunks")).into_owned();
    assert!(
        text.contains(MARKER),
        "退出前最后写出的字节被截断了：{text:?}"
    );
    let pid = handle.pid.expect("pid");
    assert!(!pid_alive(pid), "自己退出的 child 也必须真的结束");
}
