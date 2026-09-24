//! 8A 确定性矩阵：输入调度的语义**不依赖真实进程**。
//!
//! 契约：`docs/specs/2026-09-24-task8a-runtime-input-path-design.md`。
//! 这里只驱动生产 `PtyManager`；fake 只替换 PTY 传输层。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::Error;
use crate::harness::launch::LaunchSpec;
use crate::pty::manager::fake::FakePtyBackend;
use crate::pty::PtyManager;

const CODEX: &str = "D:/npm-global/codex.cmd";
/// 第二条会话用的 program：fake 的阻塞闸门**按 program** 生效，
/// 所以「B 不受 A 影响」必须让 B 用另一个 program，否则连 B 的 writer 也会一起 park。
const CLAUDE: &str = "D:/npm-global/claude.cmd";

fn spec_for(program: &str) -> LaunchSpec {
    LaunchSpec {
        program: PathBuf::from(program),
        args: Vec::new(),
        cwd: None,
        env: Vec::new(),
        runtime_target_id: "local".to_string(),
    }
}

/// 容量可注入（spec §6.1）：小容量用例用 8 / 16 字节，不硬编码 64 KiB。
///
/// 返回 `Arc`：硬期限检查要把调用放进**另一个线程**（见 [`within_manager`]），闭包必须 `'static`。
fn manager(backend: Arc<FakePtyBackend>, capacity: usize) -> Arc<PtyManager> {
    Arc::new(PtyManager::with_input_capacity(
        backend,
        Arc::new(|_session, _seq, _bytes| {}),
        Arc::new(|_session, _code| {}),
        capacity,
    ))
}

fn spawn(manager: &PtyManager, session_id: &str) -> u32 {
    spawn_program(manager, session_id, CODEX)
}

fn spawn_program(manager: &PtyManager, session_id: &str, program: &str) -> u32 {
    let handle = manager
        .spawn(session_id, spec_for(program), 80, 24)
        .expect("spawn");
    manager.start_reading(session_id).expect("start_reading");
    handle.pid.expect("pid")
}

/// 硬期限：把「Runtime 被冻结」变成可断言的失败，而不是静默挂死。
fn within<T: Send + 'static>(
    timeout: Duration,
    label: &str,
    action: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(action());
    });
    receiver
        .recv_timeout(timeout)
        .unwrap_or_else(|_| panic!("{label} 在 {timeout:?} 内没有返回（Runtime 被冻结了）"))
}

/// 对 `PtyManager` 的一次调用施加硬期限。
///
/// 为什么必须另起线程：被冻结的调用**不会**因为超时自己解开，所以不能在同一线程里等
/// （那会变成挂死）；也**不能**用 `thread::scope`（scope 退出时会 join，同样挂死）。
/// 代价是闭包必须 `'static` —— 因此 manager 以 `Arc` 传入。
fn within_manager<T: Send + 'static>(
    timeout: Duration,
    label: &str,
    manager: &Arc<PtyManager>,
    action: impl FnOnce(&PtyManager) -> T + Send + 'static,
) -> T {
    let manager = Arc::clone(manager);
    within(timeout, label, move || action(&manager))
}

/// 写入是异步的：断言 backend 记录或 pending 归零之前必须等 worker，不能 sleep 猜。
fn wait_drained(manager: &PtyManager, session_id: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if manager.pending_bytes(session_id) == Some(0) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("输入队列在 {timeout:?} 内没有排空");
}

/// 本 Task 的**行为 RED**：单批超过容量时，即使队列是空的也必须整批拒绝。
/// （`PtyManager::new` 用生产默认容量 64 KiB，所以这条在旧实现上能编译、且会失败。）
#[test]
fn a_batch_larger_than_the_default_capacity_is_rejected() {
    let backend = Arc::new(FakePtyBackend::new());
    let manager = Arc::new(PtyManager::new(
        backend,
        Arc::new(|_session, _seq, _bytes| {}),
        Arc::new(|_session, _code| {}),
    ));
    spawn(&manager, "hub-a");

    let too_large = vec![b'x'; 64 * 1024 + 1];
    let rejected = manager
        .write("hub-a", &too_large)
        .expect_err("超上限必须被拒绝");

    match rejected {
        Error::InputBackpressure {
            pending_bytes,
            capacity_bytes,
            attempted_bytes,
            ..
        } => assert_eq!(
            (pending_bytes, capacity_bytes, attempted_bytes),
            (0, 64 * 1024, 64 * 1024 + 1)
        ),
        other => panic!("必须是 InputBackpressure，实际 {other:?}"),
    }
}

/// `pending_bytes` 必须包含 in-flight：worker 一 pop 就减账会让容量被绕过（spec §4.3）。
#[test]
fn pending_bytes_counts_the_in_flight_batch() {
    let backend = Arc::new(FakePtyBackend::new().with_blocking_write(CODEX));
    let manager = manager(Arc::clone(&backend), 16);
    spawn(&manager, "hub-a");

    manager.write("hub-a", &[b'a'; 8]).expect("首批接受");
    assert!(
        backend.wait_for_write_blocked("hub-a", Duration::from_secs(5)),
        "worker 必须真的进入阻塞写，否则这条测试没在测 in-flight 记账"
    );

    assert_eq!(
        manager.pending_bytes("hub-a"),
        Some(8),
        "in-flight 必须计入"
    );
    manager.write("hub-a", &[b'b'; 8]).expect("剩余容量刚好够");
    assert_eq!(manager.pending_bytes("hub-a"), Some(16));

    let rejected = manager.write("hub-a", b"x").expect_err("已满必须拒绝");
    match rejected {
        Error::InputBackpressure {
            pending_bytes,
            capacity_bytes,
            attempted_bytes,
            ..
        } => assert_eq!(
            (pending_bytes, capacity_bytes, attempted_bytes),
            (16, 16, 1)
        ),
        other => panic!("必须是 InputBackpressure，实际 {other:?}"),
    }

    backend.release_blocked_write();
    wait_drained(&manager, "hub-a", Duration::from_secs(5));
    manager.write("hub-a", &[b'c'; 16]).expect("排空后容量恢复");
    wait_drained(&manager, "hub-a", Duration::from_secs(5));
}

#[test]
fn a_batch_larger_than_an_injected_capacity_is_rejected_even_when_empty() {
    let backend = Arc::new(FakePtyBackend::new());
    let manager = manager(Arc::clone(&backend), 8);
    spawn(&manager, "hub-a");

    let rejected = manager
        .write("hub-a", &[b'x'; 9])
        .expect_err("超上限必须拒绝");
    match rejected {
        Error::InputBackpressure {
            pending_bytes,
            capacity_bytes,
            attempted_bytes,
            ..
        } => assert_eq!((pending_bytes, capacity_bytes, attempted_bytes), (0, 8, 9)),
        other => panic!("必须是 InputBackpressure，实际 {other:?}"),
    }
}

/// 被拒批次不得留下任何字节（禁止 partial enqueue）。
#[test]
fn rejection_never_partially_enqueues() {
    let backend = Arc::new(FakePtyBackend::new().with_blocking_write(CODEX));
    let manager = manager(Arc::clone(&backend), 8);
    spawn(&manager, "hub-a");

    manager.write("hub-a", &[b'a'; 8]).expect("首批接受");
    assert!(backend.wait_for_write_blocked("hub-a", Duration::from_secs(5)));
    assert!(manager.write("hub-a", &[b'z'; 4]).is_err());

    backend.release_blocked_write();
    wait_drained(&manager, "hub-a", Duration::from_secs(5));

    let written = backend.written.lock().expect("written").clone();
    assert_eq!(written.len(), 1, "被拒批次不得进入 worker：{written:?}");
    assert_eq!(written[0].1, vec![b'a'; 8]);
}

#[test]
fn batches_are_written_in_fifo_order() {
    let backend = Arc::new(FakePtyBackend::new());
    let manager = manager(Arc::clone(&backend), 64);
    spawn(&manager, "hub-a");

    manager.write("hub-a", b"first").expect("1");
    manager.write("hub-a", b"second").expect("2");
    manager.write("hub-a", b"third").expect("3");
    wait_drained(&manager, "hub-a", Duration::from_secs(5));

    let written: Vec<Vec<u8>> = backend
        .written
        .lock()
        .expect("written")
        .iter()
        .map(|(_, bytes)| bytes.clone())
        .collect();
    assert_eq!(
        written,
        vec![b"first".to_vec(), b"second".to_vec(), b"third".to_vec()]
    );
}

/// T1 + T10：A 的 writer 永久 park 时，整台 Runtime（含 B 与 A 自己的控制面）都不冻结。
#[test]
fn a_blocked_write_in_one_session_never_freezes_the_runtime() {
    let backend = Arc::new(FakePtyBackend::new().with_blocking_write(CODEX));
    let manager = manager(Arc::clone(&backend), 8);
    spawn(&manager, "hub-a");
    spawn_program(&manager, "hub-b", CLAUDE);

    manager.write("hub-a", &[b'a'; 8]).expect("首批接受");
    assert!(backend.wait_for_write_blocked("hub-a", Duration::from_secs(5)));

    // A 自己的输入通道被背压（不是挂死）
    assert!(matches!(
        within_manager(Duration::from_secs(2), "A 背压写入", &manager, |m| m
            .write("hub-a", b"x")),
        Err(Error::InputBackpressure { .. })
    ));

    // B 完全不受影响：入队 + 真的写进 backend + resize + kill
    assert!(
        within_manager(Duration::from_secs(2), "B write", &manager, |m| m
            .write("hub-b", b"hello"))
        .is_ok()
    );
    wait_drained(&manager, "hub-b", Duration::from_secs(5));
    assert!(
        within_manager(Duration::from_secs(2), "B resize", &manager, |m| m
            .resize("hub-b", 100, 30))
        .is_ok()
    );
    assert!(
        within_manager(Duration::from_secs(2), "B kill", &manager, |m| m
            .kill("hub-b"))
        .is_ok()
    );

    // A 自己的 resize / kill / try_wait 也必须能进去（INV-3）
    assert!(
        within_manager(Duration::from_secs(2), "A resize", &manager, |m| m
            .resize("hub-a", 120, 40))
        .is_ok()
    );
    assert!(
        within_manager(Duration::from_secs(2), "A kill", &manager, |m| m
            .kill("hub-a"))
        .is_ok()
    );
    assert!(
        within_manager(Duration::from_secs(2), "A try_wait", &manager, |m| m
            .try_wait("hub-a"))
        .is_ok()
    );

    // kill 成功之后输入侧关闭（spec §4.5）
    assert!(matches!(
        manager.write("hub-a", b"x"),
        Err(Error::InputClosed { .. })
    ));
}

/// T9：shutdown 绝不 join —— worker park 在阻塞写里时也必须在硬期限内返回（spec §4.7）。
///
/// 「没有 join」的直接证据就是这条硬期限：若任何路径 join 了那条正 park 在 OS 写里的
/// worker，`forget` 会一直不返回（并在 2 秒后把用例判失败）。
#[test]
fn shutdown_discards_queued_batches_and_never_joins() {
    let backend = Arc::new(FakePtyBackend::new().with_blocking_write(CODEX));
    let manager = manager(Arc::clone(&backend), 32);
    spawn(&manager, "hub-a");

    manager
        .write("hub-a", &[b'a'; 8])
        .expect("进入阻塞写的那一批");
    assert!(backend.wait_for_write_blocked("hub-a", Duration::from_secs(5)));
    manager.write("hub-a", &[b'b'; 8]).expect("排队等待的一批");

    within_manager(Duration::from_secs(2), "forget", &manager, |m| {
        m.forget("hub-a")
    })
    .expect("forget 必须立即返回");

    assert!(matches!(
        manager.write("hub-a", b"x"),
        Err(Error::InvalidInput(_) | Error::InputClosed { .. })
    ));
}

/// 同 T1 的补充：确认**输入通道**（不只是控制面）在 A park 时对 B 仍然可用。
#[test]
fn a_blocked_writer_does_not_freeze_a_second_session_input() {
    let backend = Arc::new(FakePtyBackend::new().with_blocking_write(CODEX));
    let manager = manager(Arc::clone(&backend), 8);
    spawn(&manager, "hub-a");
    spawn_program(&manager, "hub-b", CLAUDE);

    manager.write("hub-a", &[b'a'; 8]).expect("A 接受");
    assert!(backend.wait_for_write_blocked("hub-a", Duration::from_secs(5)));

    manager.write("hub-b", b"one").expect("B 写入");
    manager.write("hub-b", b"two").expect("B 再写入");
    wait_drained(&manager, "hub-b", Duration::from_secs(5));

    let written: Vec<Vec<u8>> = backend
        .written
        .lock()
        .expect("written")
        .iter()
        .filter(|(session, _)| session == "hub-b")
        .map(|(_, bytes)| bytes.clone())
        .collect();
    assert_eq!(written, vec![b"one".to_vec(), b"two".to_vec()]);
}
