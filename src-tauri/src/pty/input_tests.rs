//! 8A 确定性矩阵：输入调度的语义**不依赖真实进程**。
//!
//! 契约：`docs/specs/2026-09-24-task8a-runtime-input-path-design.md`。
//! 这里只驱动生产 `PtyManager`；fake 只替换 PTY 传输层。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::Error;
use crate::harness::launch::LaunchSpec;
use crate::pty::backend::PtyBackend;
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

/// 带可观测 sink 的 manager：用来证明输入路径**不发事件、不写终态**（spec §4.4）。
fn manager_with_sinks(
    backend: Arc<FakePtyBackend>,
    capacity: usize,
    outputs: Arc<std::sync::Mutex<Vec<String>>>,
    exits: Arc<std::sync::Mutex<Vec<String>>>,
) -> Arc<PtyManager> {
    Arc::new(PtyManager::with_input_capacity(
        backend,
        Arc::new(move |session, _seq, _bytes| {
            outputs.lock().expect("outputs").push(session.to_string());
        }),
        Arc::new(move |session, _code| {
            exits.lock().expect("exits").push(session.to_string());
        }),
        capacity,
    ))
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

/// T6：worker 遇到真实写入错误 → 输入侧失败 + 丢弃未发送 + 后续明确报错，
/// 但 **不发事件、不写终态**，且 kill/resize/reap 仍然工作（spec §4.4）。
#[test]
fn worker_failure_closes_input_and_keeps_control_paths_alive() {
    let backend = Arc::new(FakePtyBackend::new().with_failing_write(CODEX, "PTY 已关闭"));
    let outputs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let exits = Arc::new(std::sync::Mutex::new(Vec::new()));
    let manager = manager_with_sinks(
        Arc::clone(&backend),
        64,
        Arc::clone(&outputs),
        Arc::clone(&exits),
    );
    spawn(&manager, "hub-a");

    manager
        .write("hub-a", b"first")
        .expect("入队成功（此刻还无法知道会失败）");
    manager
        .write("hub-a", b"queued-but-never-sent")
        .expect("第二批发进队列");

    // 等 worker 真的进入失败态（后续写入必须明确报 InputWorkerFailed）
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut failure = None;
    while Instant::now() < deadline {
        if let Err(error @ Error::InputWorkerFailed { .. }) = manager.write("hub-a", b"probe") {
            failure = Some(error);
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let failure = failure.expect("worker 失败后写入必须明确报 InputWorkerFailed");
    assert!(failure.to_string().contains("PTY 已关闭"), "{failure}");

    // 未发送的排队字节被丢弃，pending 归零
    assert_eq!(manager.pending_bytes("hub-a"), Some(0));

    // 控制面仍然活着（INV-3）
    assert!(
        within_manager(Duration::from_secs(2), "resize", &manager, |m| m
            .resize("hub-a", 100, 30))
        .is_ok()
    );
    assert!(
        within_manager(Duration::from_secs(2), "try_wait", &manager, |m| m
            .try_wait("hub-a"))
        .is_ok()
    );
    assert!(
        within_manager(Duration::from_secs(2), "kill", &manager, |m| m
            .kill("hub-a"))
        .is_ok()
    );

    // 输入路径绝不写终态、绝不发事件（终态只由 reaper 决定）
    assert!(
        exits.lock().expect("exits").is_empty(),
        "输入失败不得触发退出回调：{:?}",
        exits.lock().expect("exits")
    );
    assert!(
        outputs.lock().expect("outputs").is_empty(),
        "输入失败不得产生输出事件：{:?}",
        outputs.lock().expect("outputs")
    );
}

/// T11：优先级固定为 closed → failure → capacity，不吃锁竞争（spec §4.4.1）。
///
/// 唯一可观测的竞争：worker 先失败，随后 kill 成功关闭输入侧（两个标志同时为真）。
#[test]
fn error_priority_is_closed_over_worker_failure() {
    let backend = Arc::new(FakePtyBackend::new().with_failing_write(CODEX, "PTY 已关闭"));
    let manager = manager(Arc::clone(&backend), 64);
    spawn(&manager, "hub-a");

    manager.write("hub-a", b"first").expect("入队");

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_failure = false;
    while Instant::now() < deadline {
        if matches!(
            manager.write("hub-a", b"probe"),
            Err(Error::InputWorkerFailed { .. })
        ) {
            saw_failure = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(saw_failure, "先要看到 InputWorkerFailed");

    // 显式关闭输入侧（kill 成功路径）→ 之后必须报 InputClosed，而不是继续报 worker 失败
    manager.kill("hub-a").expect("kill 成功");
    assert!(
        matches!(
            manager.write("hub-a", b"after-kill"),
            Err(Error::InputClosed { .. })
        ),
        "closed 优先于 failure：显式关闭是对调用方最直接的当前事实"
    );
}

/// T7：kill 成功 → 输入侧关闭；kill 失败 → **不**擅自关闭（spec §4.5）。
#[test]
fn input_is_closed_after_a_successful_kill_but_not_after_a_failed_one() {
    // 成功路径
    let ok_backend = Arc::new(FakePtyBackend::new());
    let ok_manager = manager(Arc::clone(&ok_backend), 64);
    spawn(&ok_manager, "hub-a");
    ok_manager.kill("hub-a").expect("kill 成功");
    assert!(
        matches!(
            ok_manager.write("hub-a", b"x"),
            Err(Error::InputClosed { .. })
        ),
        "kill 成功之后必须拒绝新输入"
    );

    // 失败路径：backend 拒绝终止时，绝不能把「没能结束进程」伪装成「进程已结束」
    let broken_backend = Arc::new(FakePtyBackend::new());
    let broken_manager = manager(Arc::clone(&broken_backend), 64);
    spawn(&broken_manager, "hub-b");
    broken_backend.fail_next_kill("hub-b");
    assert!(
        broken_manager.kill("hub-b").is_err(),
        "backend kill 失败必须如实返回"
    );
    assert!(
        broken_manager.write("hub-b", b"still-ok").is_ok(),
        "kill 失败不得擅自关闭输入侧"
    );
}

/// T8：forget 之后再入队必须被拒（不得静默入队到已释放的会话）。
#[test]
fn enqueue_after_forget_is_rejected() {
    let backend = Arc::new(FakePtyBackend::new());
    let manager = manager(Arc::clone(&backend), 64);
    spawn(&manager, "hub-a");

    manager.forget("hub-a").expect("forget");
    assert!(
        matches!(
            manager.write("hub-a", b"x"),
            Err(Error::InvalidInput(_) | Error::InputClosed { .. })
        ),
        "forget 之后不得成功入队"
    );
    assert_eq!(manager.pending_bytes("hub-a"), None, "handle 必须已被移除");
}

/// 会话退出后必须释放输入侧与 backend 句柄：否则每会话泄漏一个 worker 线程
/// （现状 `forget` 全仓没有任何生产调用点），同时**顺序**必须是「先写终态、再回收资源」。
#[test]
fn the_reaper_releases_the_session_after_writing_the_terminal_state() {
    let backend = Arc::new(FakePtyBackend::new().with_always_exited(Some(0)));
    let exited = Arc::new(std::sync::Mutex::new(Vec::new()));
    let order_violated = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let manager = {
        let backend_for_sink = Arc::clone(&backend);
        let exited_for_sink = Arc::clone(&exited);
        let violated = Arc::clone(&order_violated);
        let backend_handle: Arc<dyn PtyBackend> = backend.clone();
        Arc::new(PtyManager::with_input_capacity(
            backend_handle,
            Arc::new(|_session, _seq, _bytes| {}),
            Arc::new(move |session, _code| {
                // 终态写入的这一刻，backend 里**不允许**已经被 forget
                if !backend_for_sink
                    .forgotten
                    .lock()
                    .expect("forgotten")
                    .is_empty()
                {
                    violated.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                exited_for_sink
                    .lock()
                    .expect("exited")
                    .push(session.to_string());
            }),
            64,
        ))
    };
    spawn(&manager, "hub-a");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if manager.pending_bytes("hub-a").is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(
        manager.pending_bytes("hub-a"),
        None,
        "reaper 必须释放 handle"
    );
    assert!(
        backend
            .forgotten
            .lock()
            .expect("forgotten")
            .contains(&"hub-a".to_string()),
        "reaper 必须调用 backend.forget"
    );
    assert_eq!(
        exited.lock().expect("exited").as_slice(),
        &["hub-a".to_string()],
        "终态回调必须恰好发生一次"
    );
    assert!(
        !order_violated.load(std::sync::atomic::Ordering::SeqCst),
        "顺序必须是「先写终态、再回收资源」"
    );
}

/// E1：reader EOF **不得**触发 forget（EOF ≠ 进程退出，ADR-0010 / spec §4.8）。
#[test]
fn reader_eof_does_not_release_the_session() {
    let backend = Arc::new(FakePtyBackend::new().with_output(vec![b"bye".to_vec()]));
    let manager = manager(Arc::clone(&backend), 64);
    spawn(&manager, "hub-a");

    // 负向断言的落定窗口：reader 读完预置字节（fake 随即 EOF）只需毫秒级，
    // 200ms 足够让「错误地把 EOF 当退出」的实现暴露出来。
    std::thread::sleep(Duration::from_millis(200));

    assert!(
        backend.forgotten.lock().expect("forgotten").is_empty(),
        "reader EOF 绝不能触发资源回收"
    );
    assert_eq!(
        manager.pending_bytes("hub-a"),
        Some(0),
        "handle 必须还在（进程仍然存在）"
    );
    assert!(
        manager.is_running("hub-a").expect("查询"),
        "EOF 只表示输出流结束，进程仍应被视为运行中"
    );
}
