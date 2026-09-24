# Task 8A — Runtime Input Path Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `write_terminal()` 不再阻塞在子进程 stdin 上，并让一条会话的阻塞写**无法**冻结整台 Runtime（包括它自己的 kill/resize/reap）。

**Architecture:** `PtyBackend` 保持**阻塞的纯传输 primitive**，但只有每会话专属的 writer worker 调用它；跨会话的输入调度（有界队列 + `pending_bytes` 记账 + 背压/失败态）放在新的 `pty::session::SessionHandle`，由 `PtyManager` 持有。`PortablePtyBackend` 内部改成「全局 map 锁只查 `Arc<LivePty>`，per-session 锁（writer/master/child）各自独立」，于是阻塞写只 park 在该会话的 writer 锁上。

**Tech Stack:** Rust 2021（`std::sync::{Mutex, Condvar}`、`std::thread`、`VecDeque`）、`portable-pty 0.9`、`thiserror 2`；测试用 `cargo test`（单元 + `#[cfg(windows)]` 真机集成）；前端不改。

**Spec:** `docs/specs/2026-09-24-task8a-runtime-input-path-design.md`（每个决定都从该 spec 推出；执行前先读它）

## Global Constraints

- **INV-1**：任何可能阻塞的 OS I/O 不得发生在跨会话锁（`sessions` map）持有期间。
- **INV-2**：`write_terminal` 不得阻塞在子进程 stdin 上（只做 O(1) 入队或立即报错）。
- **INV-3**：`kill` / `try_wait` / `resize` / `take_reader` 不得依赖 writer 的锁。
- **INV-4**：全局 `sessions` 锁只用于「查找 → clone `Arc` / 插入 / 删除」；**禁止** `global lock → per-session lock → OS call` 嵌套。
- `DEFAULT_INPUT_CAPACITY_BYTES = 64 * 1024`（生产默认）；容量**必须可注入**（`PtyManager::with_input_capacity`），小容量用例用 8 / 16 字节，不硬编码 64 KiB。
- `pending_bytes` = 队列中未 pop 的字节 **+ 正在 `backend.write` 里的那一批**；只有 `backend.write` **返回之后**才结算。
- 准入必须**整批接受或整批拒绝**；禁止 partial enqueue；单批 `> capacity` 时即使队列空也必须拒绝。
- `try_enqueue → Ok` 只表示「Harness Hub 已接受本次输入」，**不**表示 OS 已写入 PTY。
- worker 失败：标记输入侧失败 + 丢弃未发送队列；**不写 Session 终态**（终态只由 reaper 决定）、**不发 `PtyEvent`**。
- `kill` 成功 → 关闭输入侧；`kill` 失败 → 不动输入侧。
- `shutdown_input()` **绝不 join**；任何 Drop 路径都禁止 join（drop `JoinHandle` = detach）。
- **无 Harness 特判**；不新增跨 IPC DTO（错误仍序列化为字符串）；UI 行为不变。
- 真机 PTY 测试一律 `#![cfg(windows)]`，本机缺前置条件时明确跳过，绝不假装通过。
- 提交信息用 `test:` / `feat:` / `fix:` / `chore:` / `docs:` / `refactor:` 前缀，一次提交只做一件事。
- 每个 Task 结束都要跑该 Task 指定的测试命令；全部任务结束后跑 `pnpm verify`。

## File Structure

| 文件 | 责任 | 动作 |
| --- | --- | --- |
| `src-tauri/src/error.rs` | 三个新错误变体（背压诊断字段齐全） | Modify |
| `src-tauri/src/pty/session.rs` | `InputState` / `SessionHandle`：有界队列、`pending_bytes` 记账、worker 生命周期 | Create |
| `src-tauri/src/pty/mod.rs` | 暴露 `session` 模块与 `#[cfg(test)] mod input_tests;` | Modify |
| `src-tauri/src/pty/manager.rs` | `PtyManager` 持 `sessions: Arc<Mutex<HashMap<String, Arc<SessionHandle>>>>`；`write` 入队；`kill`/`forget`/reaper 关闭输入侧；fake 增加阻塞写/失败写 | Modify |
| `src-tauri/src/pty/portable_pty_backend.rs` | 每会话独立加锁（writer/master/child），全局 map 锁只查 `Arc` | Modify |
| `src-tauri/src/pty/input_tests.rs` | 确定性矩阵（T1–T10），不碰真实进程 | Create |
| `src-tauri/tests/runtime_input_backpressure.rs` | 真机 synthetic child 验收（R1–R3） | Create |
| `tests/e2e/README.md` | 8A 真机证据 | Modify |

---

### Task 1: 错误变体（背压诊断字段齐全）

**Files:**
- Modify: `src-tauri/src/error.rs`（`enum Error` 内 `InvalidInput` 之后）
- Test: `src-tauri/src/error.rs`（文件末尾新增 `#[cfg(test)] mod tests`）

**Interfaces:**
- Consumes: 无
- Produces: `Error::InputBackpressure { session_id: String, pending_bytes: usize, capacity_bytes: usize, attempted_bytes: usize }`、`Error::InputClosed { session_id: String }`、`Error::InputWorkerFailed { session_id: String, detail: String }`

- [ ] **Step 1: 写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// 背压错误必须自带四个诊断字段：用户贴 5 MiB 时要能一眼区分
    /// 「队列本来就满」与「这一批自己超上限」（spec §4.9）。
    #[test]
    fn backpressure_error_carries_full_diagnostics() {
        let error = Error::InputBackpressure {
            session_id: "hub-1".to_string(),
            pending_bytes: 65_536,
            capacity_bytes: 65_536,
            attempted_bytes: 12,
        };
        let message = error.to_string();

        assert!(message.contains("hub-1"), "{message}");
        assert!(message.contains("65536"), "{message}");
        assert!(message.contains("12"), "{message}");
        // 跨 IPC 仍是字符串（IpcResult 形状不变）。
        assert_eq!(
            serde_json::to_value(&error).expect("序列化"),
            serde_json::json!(message)
        );
    }

    #[test]
    fn closed_and_worker_failure_are_distinguishable_variants() {
        let closed = Error::InputClosed {
            session_id: "hub-1".to_string(),
        };
        let failed = Error::InputWorkerFailed {
            session_id: "hub-1".to_string(),
            detail: "写入 PTY 失败".to_string(),
        };

        assert!(closed.to_string().contains("已关闭"));
        assert!(failed.to_string().contains("写入 PTY 失败"));
        assert!(matches!(closed, Error::InputClosed { .. }));
        assert!(matches!(failed, Error::InputWorkerFailed { .. }));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib error::tests`
Expected: 编译错误 `no variant named InputBackpressure`

- [ ] **Step 3: 最小实现**

在 `error.rs` 的 `InvalidInput(String)` 之后加入：

```rust
    /// 输入未被接受：会话的待写输入（含正在写的那一批）已达上限。
    #[error(
        "输入被丢弃：会话 {session_id} 的待写输入 {pending_bytes} 字节已达上限 {capacity_bytes}（本次 {attempted_bytes} 字节整批拒绝）"
    )]
    InputBackpressure {
        session_id: String,
        pending_bytes: usize,
        capacity_bytes: usize,
        attempted_bytes: usize,
    },

    /// 输入侧已关闭（会话结束 / forget / kill 成功之后）。
    #[error("输入不可用：会话 {session_id} 的输入侧已关闭")]
    InputClosed { session_id: String },

    /// writer worker 遇到真实的 PTY 写入错误（终态仍只由 reaper 决定）。
    #[error("输入不可用：会话 {session_id} 的写入线程失败：{detail}")]
    InputWorkerFailed { session_id: String, detail: String },
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib error::tests`
Expected: `test result: ok. 2 passed`

- [ ] **Step 5: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/src/error.rs
git commit -m "feat(pty): add input backpressure/closed/worker-failed errors with diagnostics"
```

---

### Task 2: 有界输入队列 + writer worker（T1–T5、T9、T10）

**Files:**
- Create: `src-tauri/src/pty/session.rs`
- Modify: `src-tauri/src/pty/mod.rs`
- Modify: `src-tauri/src/pty/manager.rs`
- Test: `src-tauri/src/pty/input_tests.rs`（Create）

**Interfaces:**
- Consumes: `Error::{InputBackpressure, InputClosed}`（Task 1）
- Produces:
  - `pty::session::DEFAULT_INPUT_CAPACITY_BYTES: usize`
  - `pty::session::SessionHandle::{spawn(&str, Arc<dyn PtyBackend>, usize) -> Self, try_enqueue(&self, &[u8]) -> Result<()>, shutdown_input(&self), pending_bytes(&self) -> usize}`
  - `pty::session::InputState`（`pub(crate)`；可观测契约就是三个错误变体，**不**额外暴露 `closed`/`failure` 谓词 —— 没有第二个谓词就没有「只查了一半」的坑）
  - `PtyManager::{new (改为委托), with_input_capacity(Arc<dyn PtyBackend>, OutputSink, ExitSink, usize) -> Self, write (入队), pending_bytes(&self, &str) -> Option<usize>, forget, kill (kill 成功关闭输入侧)}`
  - fake：`with_blocking_write(&str) -> Self`、`wait_for_write_blocked(&self, &str, Duration) -> bool`、`release_blocked_write(&self)`

- [ ] **Step 1: 写失败测试**

创建 `src-tauri/src/pty/input_tests.rs`（本 Task 写全，共 6 条；**Step 2 必须先看到第一条 RED**）：

```rust
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

fn spec() -> LaunchSpec {
    LaunchSpec {
        program: PathBuf::from(CODEX),
        args: Vec::new(),
        cwd: None,
        env: Vec::new(),
        runtime_target_id: "local".to_string(),
    }
}

/// 容量可注入（spec §6.1）：小容量用例用 8 / 16 字节，不硬编码 64 KiB。
fn manager(backend: Arc<FakePtyBackend>, capacity: usize) -> PtyManager {
    PtyManager::with_input_capacity(
        backend,
        Arc::new(|_session, _seq, _bytes| {}),
        Arc::new(|_session, _code| {}),
        capacity,
    )
}

/// 带可观测 sink 的 manager：用来证明输入路径**不发事件、不写终态**（spec §4.4）。
fn manager_with_sinks(
    backend: Arc<FakePtyBackend>,
    capacity: usize,
    outputs: Arc<std::sync::Mutex<Vec<String>>>,
    exits: Arc<std::sync::Mutex<Vec<String>>>,
) -> PtyManager {
    PtyManager::with_input_capacity(
        backend,
        Arc::new(move |session, _seq, _bytes| {
            outputs.lock().expect("outputs").push(session.to_string());
        }),
        Arc::new(move |session, _code| {
            exits.lock().expect("exits").push(session.to_string());
        }),
        capacity,
    )
}

fn spawn(manager: &PtyManager, session_id: &str) -> u32 {
    let handle = manager.spawn(session_id, spec(), 80, 24).expect("spawn");
    manager.start_reading(session_id).expect("start_reading");
    handle.pid.expect("pid")
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

/// 硬期限：把「Runtime 被冻结」变成可断言的失败，而不是静默挂死。
fn within<T>(timeout: Duration, label: &str, action: impl FnOnce() -> T) -> T {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(action());
    });
    receiver
        .recv_timeout(timeout)
        .unwrap_or_else(|_| panic!("{label} 在 {timeout:?} 内没有返回（Runtime 被冻结了）"))
}

/// 本 Task 的**行为 RED**：单批超过容量时，即使队列是空的也必须整批拒绝。
/// （`PtyManager::new` 用生产默认容量 64 KiB，所以这条在旧实现上能编译、且会失败。）
#[test]
fn a_batch_larger_than_the_default_capacity_is_rejected() {
    let backend = Arc::new(FakePtyBackend::new());
    let manager = PtyManager::new(
        backend,
        Arc::new(|_session, _seq, _bytes| {}),
        Arc::new(|_session, _code| {}),
    );
    spawn(&manager, "hub-a");

    let too_large = vec![b'x'; 64 * 1024 + 1];
    let rejected = manager.write("hub-a", &too_large).expect_err("超上限必须被拒绝");

    match rejected {
        Error::InputBackpressure {
            pending_bytes,
            capacity_bytes,
            attempted_bytes,
            ..
        } => assert_eq!((pending_bytes, capacity_bytes, attempted_bytes), (0, 64 * 1024, 64 * 1024 + 1)),
        other => panic!("必须是 InputBackpressure，实际 {other:?}"),
    }
}

#[test]
fn a_batch_larger_than_an_injected_capacity_is_rejected_even_when_empty() {
    let backend = Arc::new(FakePtyBackend::new());
    let manager = manager(Arc::clone(&backend), 8);
    spawn(&manager, "hub-a");

    let rejected = manager.write("hub-a", &[b'x'; 9]).expect_err("超上限必须拒绝");
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

    assert_eq!(manager.pending_bytes("hub-a"), Some(8), "in-flight 必须计入");
    manager.write("hub-a", &[b'b'; 8]).expect("剩余容量刚好够");
    assert_eq!(manager.pending_bytes("hub-a"), Some(16));

    let rejected = manager.write("hub-a", b"x").expect_err("已满必须拒绝");
    match rejected {
        Error::InputBackpressure {
            pending_bytes,
            capacity_bytes,
            attempted_bytes,
            ..
        } => assert_eq!((pending_bytes, capacity_bytes, attempted_bytes), (16, 16, 1)),
        other => panic!("必须是 InputBackpressure，实际 {other:?}"),
    }

    backend.release_blocked_write();
    wait_drained(&manager, "hub-a", Duration::from_secs(5));
    manager.write("hub-a", &[b'c'; 16]).expect("排空后容量恢复");
    wait_drained(&manager, "hub-a", Duration::from_secs(5));
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
    spawn(&manager, "hub-b");

    manager.write("hub-a", &[b'a'; 8]).expect("首批接受");
    assert!(backend.wait_for_write_blocked("hub-a", Duration::from_secs(5)));

    // A 自己的输入通道被背压（不是挂死）
    assert!(matches!(
        within(Duration::from_secs(2), "A 背压写入", || manager.write("hub-a", b"x")),
        Err(Error::InputBackpressure { .. })
    ));

    // B 完全不受影响
    assert!(within(Duration::from_secs(2), "B write", || manager.write("hub-b", b"hello")).is_ok());
    assert!(within(Duration::from_secs(2), "B resize", || manager.resize("hub-b", 100, 30)).is_ok());
    assert!(within(Duration::from_secs(2), "B kill", || manager.kill("hub-b")).is_ok());
    wait_drained(&manager, "hub-b", Duration::from_secs(5));

    // A 自己的 resize / kill / try_wait 也必须能进去（INV-3）
    assert!(within(Duration::from_secs(2), "A resize", || manager.resize("hub-a", 120, 40)).is_ok());
    assert!(within(Duration::from_secs(2), "A kill", || manager.kill("hub-a")).is_ok());
    assert!(within(Duration::from_secs(2), "A try_wait", || manager.try_wait("hub-a")).is_ok());

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

    manager.write("hub-a", &[b'a'; 8]).expect("进入阻塞写的那一批");
    assert!(backend.wait_for_write_blocked("hub-a", Duration::from_secs(5)));
    manager.write("hub-a", &[b'b'; 8]).expect("排队等待的一批");

    within(Duration::from_secs(2), "forget", || manager.forget("hub-a")).expect("forget 必须立即返回");

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
    spawn(&manager, "hub-b");

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
```

- [ ] **Step 2: 跑测试确认失败（行为 RED，不是只有编译错误）**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::input_tests`
Expected: 编译错误 `no function with_input_capacity` 会先挡路 —— 因此本步先临时把
`with_input_capacity` 之外的用例注释掉，只留
`a_batch_larger_than_the_default_capacity_is_rejected`，跑出**行为失败**：

```text
assertion failed: 超上限必须被拒绝
（旧 write 直接委托 backend.write，永远返回 Ok）
```

看到这条失败后，把其余用例恢复（它们随后必须一并变绿）。**不要**跳过这一步：这是本 Task 的
「先看它失败」证据。

- [ ] **Step 3: 实现 `pty/session.rs`**

（本 Task 只做队列 + 记账 + 关闭；**失败态留给 Task 3**，否则 Task 3 的测试会一写就绿。）

```rust
//! 每会话输入调度：**有界**队列 + 专属 writer worker。
//!
//! ```text
//! TerminalRuntime::write → SessionHandle::try_enqueue   （O(1)，永不阻塞）
//!                              ↓ 有界队列（byte 记账）
//!                    writer worker → PtyBackend::write   （阻塞 primitive，只在这里调用）
//! ```
//!
//! 硬约定：
//! 1. `pending_bytes` **包含正在写的那一批**，只有 `backend.write` 返回后才结算；
//! 2. `shutdown_input` **绝不 join**（drop `JoinHandle` = detach）—— worker 可能正 park 在
//!    阻塞的 OS 写里，join 会把本 Task 要修的问题从后门放回来。

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

use crate::error::{Error, Result};
use crate::pty::backend::PtyBackend;

/// 生产默认容量（测试用 `PtyManager::with_input_capacity` 注入小容量）。
pub(crate) const DEFAULT_INPUT_CAPACITY_BYTES: usize = 64 * 1024;

struct InputQueue {
    batches: VecDeque<Vec<u8>>,
    /// 队列中未 pop 的字节 + 正在 `backend.write` 里的那一批。
    pending_bytes: usize,
    closed: bool,
}

pub(crate) struct InputState {
    session_id: String,
    capacity_bytes: usize,
    queue: Mutex<InputQueue>,
    ready: Condvar,
}

impl InputState {
    fn new(session_id: &str, capacity_bytes: usize) -> Self {
        Self {
            session_id: session_id.to_string(),
            capacity_bytes,
            queue: Mutex::new(InputQueue {
                batches: VecDeque::new(),
                pending_bytes: 0,
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, InputQueue> {
        self.queue
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn pending_bytes(&self) -> usize {
        self.lock().pending_bytes
    }

    /// 整批接受或整批拒绝；**绝不** partial enqueue。
    pub(crate) fn try_enqueue(&self, bytes: &[u8]) -> Result<()> {
        let mut queue = self.lock();

        if queue.closed {
            return Err(Error::InputClosed {
                session_id: self.session_id.clone(),
            });
        }

        let attempted_bytes = bytes.len();
        if queue.pending_bytes + attempted_bytes > self.capacity_bytes {
            return Err(Error::InputBackpressure {
                session_id: self.session_id.clone(),
                pending_bytes: queue.pending_bytes,
                capacity_bytes: self.capacity_bytes,
                attempted_bytes,
            });
        }

        queue.batches.push_back(bytes.to_vec());
        queue.pending_bytes += attempted_bytes;
        drop(queue);
        self.ready.notify_all();
        Ok(())
    }

    /// worker 取下一批；`None` = 输入侧已关闭，可以退出线程。
    fn take_batch(&self) -> Option<Vec<u8>> {
        let mut queue = self.lock();
        loop {
            if let Some(batch) = queue.batches.pop_front() {
                return Some(batch);
            }
            if queue.closed {
                return None;
            }
            queue = self
                .ready
                .wait(queue)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// 只有 `backend.write` **返回之后**才结算（spec §4.3）。
    fn finish_batch(&self, len: usize) {
        let mut queue = self.lock();
        queue.pending_bytes = queue.pending_bytes.saturating_sub(len);
    }

    /// 关闭输入侧：标记 closed、丢弃未发送输入、修正 `pending_bytes`、唤醒 worker。**不 join。**
    fn shutdown(&self) {
        let mut queue = self.lock();
        queue.closed = true;
        queue.batches.clear();
        queue.pending_bytes = 0;
        drop(queue);
        self.ready.notify_all();
    }
}

/// 会话的输入侧句柄。worker 只捕获 `Arc<InputState>` + backend + session_id，
/// **不**捕获 `SessionHandle`（避免与 `JoinHandle` 自引用，spec §4.2）。
pub(crate) struct SessionHandle {
    input: Arc<InputState>,
    /// drop 它 = detach。**禁止任何路径 join**（含 Drop）。
    _worker: JoinHandle<()>,
}

impl SessionHandle {
    pub(crate) fn spawn(
        session_id: &str,
        backend: Arc<dyn PtyBackend>,
        capacity_bytes: usize,
    ) -> Self {
        let input = Arc::new(InputState::new(session_id, capacity_bytes));
        let worker_input = Arc::clone(&input);
        let worker_session = session_id.to_string();

        let _worker = thread::spawn(move || {
            while let Some(batch) = worker_input.take_batch() {
                // Task 3 会把这里换成「失败 → 标记输入侧失败并丢弃队列」。
                if backend.write(&worker_session, &batch).is_err() {
                    break;
                }
                worker_input.finish_batch(batch.len());
            }
        });

        Self { input, _worker }
    }

    pub(crate) fn try_enqueue(&self, bytes: &[u8]) -> Result<()> {
        self.input.try_enqueue(bytes)
    }

    pub(crate) fn shutdown_input(&self) {
        self.input.shutdown();
    }

    pub(crate) fn pending_bytes(&self) -> usize {
        self.input.pending_bytes()
    }
}
```

- [ ] **Step 4: 接线 `PtyManager`**

`src-tauri/src/pty/mod.rs`：

```rust
pub mod backend;
pub mod manager;
pub mod portable_pty_backend;
pub mod session;

#[cfg(test)]
mod input_tests;
```

`src-tauri/src/pty/manager.rs`：

```rust
use crate::pty::session::{SessionHandle, DEFAULT_INPUT_CAPACITY_BYTES};

pub struct PtyManager {
    backend: Arc<dyn PtyBackend>,
    on_output: OutputSink,
    on_exit: ExitSink,
    input_capacity_bytes: usize,
    pending_readers: Mutex<HashMap<String, Box<dyn Read + Send>>>,
    /// 每会话输入侧（有界队列 + writer worker）。`Arc` 是为了让 reaper 也能释放它。
    sessions: Arc<Mutex<HashMap<String, Arc<SessionHandle>>>>,
}

impl PtyManager {
    pub fn new(backend: Arc<dyn PtyBackend>, on_output: OutputSink, on_exit: ExitSink) -> Self {
        Self::with_input_capacity(backend, on_output, on_exit, DEFAULT_INPUT_CAPACITY_BYTES)
    }

    /// 容量可注入（spec §6.1）。
    pub fn with_input_capacity(
        backend: Arc<dyn PtyBackend>,
        on_output: OutputSink,
        on_exit: ExitSink,
        input_capacity_bytes: usize,
    ) -> Self {
        Self {
            backend,
            on_output,
            on_exit,
            input_capacity_bytes,
            pending_readers: Mutex::new(HashMap::new()),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn session_handle(&self, session_id: &str) -> Result<Arc<SessionHandle>> {
        self.sessions
            .lock()
            .map_err(|_| Error::StateLockPoisoned)?
            .get(session_id)
            .cloned()
            .ok_or_else(|| Error::InvalidInput(format!("未知会话：{session_id}")))
    }

    pub fn spawn(
        &self,
        session_id: &str,
        spec: LaunchSpec,
        cols: u16,
        rows: u16,
    ) -> Result<PtyProcessHandle> {
        let handle = self.backend.spawn(crate::pty::backend::PtySpawnRequest {
            session_id: session_id.to_string(),
            spec,
            cols,
            rows,
        })?;

        let reader = self.backend.take_reader(session_id)?;
        self.pending_readers
            .lock()
            .map_err(|_| Error::StateLockPoisoned)?
            .insert(session_id.to_string(), reader);

        // spawn 失败不得留下 handle（spec §7 L1）。
        self.sessions
            .lock()
            .map_err(|_| Error::StateLockPoisoned)?
            .insert(
                session_id.to_string(),
                Arc::new(SessionHandle::spawn(
                    session_id,
                    Arc::clone(&self.backend),
                    self.input_capacity_bytes,
                )),
            );

        Ok(handle)
    }

    /// **非阻塞**：只入队（INV-2）。`Ok` 只表示「已接受本次输入」。
    pub fn write(&self, session_id: &str, bytes: &[u8]) -> Result<()> {
        self.session_handle(session_id)?.try_enqueue(bytes)
    }

    pub fn pending_bytes(&self, session_id: &str) -> Option<usize> {
        self.sessions
            .lock()
            .ok()?
            .get(session_id)
            .map(SessionHandle::pending_bytes)
    }

    /// kill 成功才关闭输入侧（spec §4.5）。
    pub fn kill(&self, session_id: &str) -> Result<()> {
        self.backend.kill(session_id)?;
        if let Ok(handle) = self.session_handle(session_id) {
            handle.shutdown_input();
        }
        Ok(())
    }

    /// 释放资源：关闭输入侧 + 丢 backend 句柄。reaper 在 Task 6 接上。
    pub fn forget(&self, session_id: &str) -> Result<()> {
        let handle = self
            .sessions
            .lock()
            .map_err(|_| Error::StateLockPoisoned)?
            .remove(session_id);
        if let Some(handle) = handle {
            handle.shutdown_input();
        }
        self.backend.forget(session_id)
    }
}
```

`resize` / `try_wait` / `is_running` / `start_reading` 本 Task **不动**。

- [ ] **Step 5: fake 加阻塞写闸门（test-only）**

在 `#[cfg(test)] mod fake` 内加入：

```rust
    /// 「子进程永远不读 stdin」的可编程替身：让某个 program 的 `write` park 在 Condvar 上。
    #[derive(Default)]
    struct WriteGate {
        entered: Mutex<std::collections::HashSet<String>>,
        entered_changed: Condvar,
        released: Mutex<bool>,
        release_changed: Condvar,
    }

    impl WriteGate {
        fn block(&self, session_id: &str) {
            {
                let mut entered = self.entered.lock().expect("entered");
                entered.insert(session_id.to_string());
                self.entered_changed.notify_all();
            }
            let mut released = self.released.lock().expect("released");
            while !*released {
                released = self
                    .release_changed
                    .wait(released)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        }

        fn wait_entered(&self, session_id: &str, timeout: Duration) -> bool {
            let deadline = std::time::Instant::now() + timeout;
            let mut entered = self.entered.lock().expect("entered");
            loop {
                if entered.contains(session_id) {
                    return true;
                }
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return false;
                }
                let (guard, _) = self
                    .entered_changed
                    .wait_timeout(entered, remaining)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                entered = guard;
            }
        }

        fn release(&self) {
            *self.released.lock().expect("released") = true;
            self.release_changed.notify_all();
        }
    }
```

`FakePtyBackend` 加字段：

```rust
        /// 哪些 program 的 write 会 park（模拟子进程不读 stdin）。
        blocking_writes: Mutex<std::collections::HashSet<String>>,
        gate: WriteGate,
```

加方法：

```rust
        /// 该 program 的 `write` 会**永久** park，直到 `release_blocked_write()`。
        pub fn with_blocking_write(self, program: &str) -> Self {
            self.blocking_writes
                .lock()
                .expect("blocking_writes")
                .insert(program.to_string());
            self
        }

        pub fn wait_for_write_blocked(&self, session_id: &str, timeout: Duration) -> bool {
            self.gate.wait_entered(session_id, timeout)
        }

        pub fn release_blocked_write(&self) {
            self.gate.release();
        }
```

`impl PtyBackend for FakePtyBackend::write` 末尾（原有 `written.push` 之后）加：

```rust
            if let Some(program) = self.program_of(session_id) {
                if self
                    .blocking_writes
                    .lock()
                    .expect("blocking_writes")
                    .contains(&program)
                {
                    self.gate.block(session_id);
                }
            }
            Ok(())
```

- [ ] **Step 6: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::`
Expected: `pty::input_tests` 8 条全绿；`pty::manager::tests` 里断言 `backend.written` 的用例会因异步化失败

- [ ] **Step 7: 适配既有 `pty::manager::tests` 的异步时序（合法修改，不是放宽）**

在 `pty::manager::tests` 加 helper：

```rust
    /// 写入现在是异步的：断言 backend 记录前必须等 worker 排空（不能 sleep 猜）。
    fn wait_for_writes(backend: &Arc<FakePtyBackend>, expected: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if backend.written.lock().expect("written").len() >= expected {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("backend 在 5 秒内没有收到 {expected} 次写入");
    }
```

`write_resize_and_kill_are_delegated_verbatim`：`manager.write("hub-1", b"ls\r")` 之后插
`wait_for_writes(&backend, 1);` 再断言 `written`。`resized` / `killed` 是同步的，不用等。

- [ ] **Step 8: 跑全部 pty 测试 + clippy**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty:: && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
Expected: 全绿、clippy 无警告

- [ ] **Step 9: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/src/pty/session.rs src-tauri/src/pty/mod.rs src-tauri/src/pty/manager.rs src-tauri/src/pty/input_tests.rs
git commit -m "feat(pty): schedule per-session input through a bounded queue and writer worker"
```

---

### Task 3: 异步 writer 失败语义（T6）

**Files:**
- Modify: `src-tauri/src/pty/session.rs`（加 `failure` 态）
- Modify: `src-tauri/src/pty/manager.rs`（fake 加 `with_failing_write`）
- Test: `src-tauri/src/pty/input_tests.rs`

**Interfaces:**
- Consumes: Task 2 的 `SessionHandle` / `manager_with_sinks` helper
- Produces: `InputState` 的失败态（`failure: Option<String>`、`fail(&Error)`）、`Error::InputWorkerFailed` 的返回路径、fake 的 `with_failing_write(program, message) -> Self`

- [ ] **Step 1: 写失败测试**

```rust
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

    manager.write("hub-a", b"first").expect("入队成功（此刻还无法知道会失败）");
    manager.write("hub-a", b"queued-but-never-sent").expect("第二批发进队列");

    // 等 worker 真的进入失败态（后续写入必须明确报 InputWorkerFailed）
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut failure = None;
    while Instant::now() < deadline {
        if let Err(error @ Error::InputWorkerFailed { .. }) = manager.write("hub-a", b"x") {
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
    assert!(within(Duration::from_secs(2), "resize", || manager.resize("hub-a", 100, 30)).is_ok());
    assert!(within(Duration::from_secs(2), "try_wait", || manager.try_wait("hub-a")).is_ok());
    assert!(within(Duration::from_secs(2), "kill", || manager.kill("hub-a")).is_ok());

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
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::input_tests::worker_failure`
Expected: 先因 `no method named with_failing_write` 无法编译；补上 fake 方法后（Step 3 前半）
必须看到**行为失败**：第二次写入返回 `Ok`（自己还在队列里/被接受）而不是 `InputWorkerFailed`，
且 `pending_bytes` 仍非 0

- [ ] **Step 3: 最小实现**

fake（`pty/manager.rs`）：加字段 `failing_writes: Mutex<HashMap<String, String>>` + 方法

```rust
        pub fn with_failing_write(self, program: &str, message: &str) -> Self {
            self.failing_writes
                .lock()
                .expect("failing_writes")
                .insert(program.to_string(), message.to_string());
            self
        }
```

并在 fake 的 `write` 里、`written.push` 之后、阻塞闸门**之前**：

```rust
            if let Some(message) = self
                .program_of(session_id)
                .and_then(|program| self.failing_writes.lock().expect("failing_writes").get(&program).cloned())
            {
                return Err(Error::InvalidInput(format!("fake: 写入 PTY 失败：{message}")));
            }
```

`pty/session.rs`：

```rust
struct InputQueue {
    batches: VecDeque<Vec<u8>>,
    pending_bytes: usize,
    closed: bool,
    /// worker 遇到的真实 `backend.write` 错误（spec §4.4）。
    failure: Option<String>,
}
```

`InputState::new` 里加 `failure: None`。

`try_enqueue` 里，在 closed 检查**之后**、容量检查**之前**插入（**优先级：closed → failure →
capacity**，见 spec §4.4.1）：

```rust
        if let Some(detail) = queue.failure.clone() {
            return Err(Error::InputWorkerFailed {
                session_id: self.session_id.clone(),
                detail,
            });
        }
```

`take_batch` 的退出条件加上 `|| queue.failure.is_some()`；新增：

```rust
    /// worker 失败：标记 + 丢弃未发送输入（此时无 in-flight，pending 归零）。**不写终态。**
    fn fail(&self, error: &Error) {
        let mut queue = self.lock();
        queue.failure = Some(error.to_string());
        queue.batches.clear();
        queue.pending_bytes = 0;
        drop(queue);
        self.ready.notify_all();
    }
```

worker 循环改为：

```rust
            while let Some(batch) = worker_input.take_batch() {
                match backend.write(&worker_session, &batch) {
                    Ok(()) => worker_input.finish_batch(batch.len()),
                    Err(error) => {
                        worker_input.fail(&error);
                        break;
                    }
                }
            }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::`
Expected: `test result: ok`（含 `the_reaper_releases_the_session_after_writing_the_terminal_state` 与
`reader_eof_does_not_release_the_session`）

- [ ] **Step 5: 锁死错误优先级（T11）**

```rust
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
        matches!(manager.write("hub-a", b"after-kill"), Err(Error::InputClosed { .. })),
        "closed 优先于 failure：显式关闭是对调用方最直接的当前事实"
    );
}
```

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::input_tests::error_priority`
Expected: PASS（若报 `InputWorkerFailed`，说明实现里的检查顺序反了 —— 按 spec §4.4.1 调成
closed 在前，**不要**改测试）

- [ ] **Step 6: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/src/pty/session.rs src-tauri/src/pty/manager.rs src-tauri/src/pty/input_tests.rs
git commit -m "feat(pty): async writer failure marks the input side unavailable, without touching state"
```

---

### Task 4: `kill` 成功关闭输入侧 / `forget` 后拒绝入队（T7/T8）

**Files:**
- Modify: `src-tauri/src/pty/manager.rs`（fake 加 `fail_next_kill`）
- Test: `src-tauri/src/pty/input_tests.rs`

**Interfaces:**
- Consumes: Task 2 的 `PtyManager::{kill, forget}`（`kill` 成功关闭输入侧已在 Task 2 接线）
- Produces: fake 的 `fail_next_kill(&self, session_id: &str)` + 字段 `failing_kills: Mutex<std::collections::HashSet<String>>`

- [ ] **Step 1: 写失败测试**

```rust
/// T7：kill 成功 → 输入侧关闭；kill 失败 → **不**擅自关闭（spec §4.5）。
#[test]
fn input_is_closed_after_a_successful_kill_but_not_after_a_failed_one() {
    // 成功路径
    let backend = Arc::new(FakePtyBackend::new());
    let manager = manager(Arc::clone(&backend), 64);
    spawn(&manager, "hub-a");
    manager.kill("hub-a").expect("kill 成功");
    assert!(
        matches!(manager.write("hub-a", b"x"), Err(Error::InputClosed { .. })),
        "kill 成功之后必须拒绝新输入"
    );

    // 失败路径
    let backend = Arc::new(FakePtyBackend::new());
    let manager = manager(Arc::clone(&backend), 64);
    spawn(&manager, "hub-b");
    backend.fail_next_kill("hub-b");
    assert!(manager.kill("hub-b").is_err(), "backend kill 失败必须如实返回");
    assert!(
        manager.write("hub-b", b"still-ok").is_ok(),
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
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::input_tests`
Expected: 编译错误 `no method named fail_next_kill`

- [ ] **Step 3: 实现 fake 的 kill 失败开关**

字段：`failing_kills: Mutex<std::collections::HashSet<String>>`；方法：

```rust
        /// 让下一次 kill 直接失败（模拟 backend 层面的终止失败）。
        pub fn fail_next_kill(&self, session_id: &str) {
            self.failing_kills
                .lock()
                .expect("failing_kills")
                .insert(session_id.to_string());
        }
```

fake 的 `kill` 开头：

```rust
            if self
                .failing_kills
                .lock()
                .expect("failing_kills")
                .remove(session_id)
            {
                return Err(Error::InvalidInput("fake: 结束进程失败".to_string()));
            }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::`
Expected: `test result: ok`

- [ ] **Step 5: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/src/pty/manager.rs src-tauri/src/pty/input_tests.rs
git commit -m "test(pty): pin kill/forget interaction with the input side"
```

---

### Task 5: `PortablePtyBackend` 每会话独立加锁（INV-1/INV-4）

**Files:**
- Modify: `src-tauri/src/pty/portable_pty_backend.rs`

**Interfaces:**
- Consumes: 无
- Produces: 同名 `PtyBackend` 实现，内部为 `Mutex<HashMap<String, Arc<LivePty>>>` + `LivePty { writer, master, child }` 三个独立 `Mutex`（**trait 不变**）

- [ ] **Step 1: 先跑基线**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test real_codex_terminal -- --nocapture`
Expected: PASS（或明确打印「本机没装」跳过）

- [ ] **Step 2: 重构（纯结构改动，无行为变化）**

```rust
struct LivePty {
    /// 只有写入会 park 在这把锁上。
    writer: Mutex<Box<dyn Write + Send>>,
    /// resize / try_clone_reader。
    master: Mutex<Box<dyn MasterPty + Send>>,
    /// kill / try_wait / is_running —— **不**依赖 writer 的锁（INV-3）。
    child: Mutex<Box<dyn Child + Send + Sync>>,
}

#[derive(Default)]
pub struct PortablePtyBackend {
    /// 只用于「查找 → clone Arc / 插入 / 删除」；**绝不**在持有期间做 OS 调用（INV-1/INV-4）。
    sessions: Mutex<HashMap<String, Arc<LivePty>>>,
}

impl PortablePtyBackend {
    /// 统一访问模式：全局锁只活在这几行里。
    fn live(&self, session_id: &str) -> Result<Arc<LivePty>> {
        lock(&self.sessions)?
            .get(session_id)
            .cloned()
            .ok_or_else(|| Error::InvalidInput(format!("未知会话：{session_id}")))
    }
}
```

方法改写。`spawn` 的前半段**逐字保留**（从 `let pty_system = native_pty_system();` 到
`drop(pair.slave);`，含 openpty / CommandBuilder / spawn_command / 丢掉 slave），只把
「组装 `LivePty` 并插入」的后半段换掉：

```rust
    fn spawn(&self, request: PtySpawnRequest) -> Result<PtyProcessHandle> {
        // ── 以下到 drop(pair.slave) 为止逐字保留现状（openpty / builder / spawn_command）──
        // ── 以下为替换后的后半段 ──
        let pid = child.process_id();
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| Error::InvalidInput(format!("获取 PTY 写端失败：{error}")))?;
        lock(&self.sessions)?.insert(
            request.session_id.clone(),
            Arc::new(LivePty {
                writer: Mutex::new(writer),
                master: Mutex::new(pair.master),
                child: Mutex::new(child),
            }),
        );
        Ok(PtyProcessHandle {
            session_id: request.session_id,
            pid,
        })
    }

    fn take_reader(&self, session_id: &str) -> Result<Box<dyn Read + Send>> {
        let live = self.live(session_id)?;
        let master = lock(&live.master)?;
        master
            .try_clone_reader()
            .map_err(|error| Error::InvalidInput(format!("获取 PTY 读端失败：{error}")))
    }

    fn write(&self, session_id: &str, bytes: &[u8]) -> Result<()> {
        let live = self.live(session_id)?;
        let mut writer = lock(&live.writer)?;
        writer
            .write_all(bytes)
            .map_err(|error| Error::InvalidInput(format!("写入 PTY 失败：{error}")))?;
        writer
            .flush()
            .map_err(|error| Error::InvalidInput(format!("刷新 PTY 失败：{error}")))
    }

    fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<()> {
        let live = self.live(session_id)?;
        let master = lock(&live.master)?;
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| Error::InvalidInput(format!("调整 PTY 尺寸失败：{error}")))
    }

    fn kill(&self, session_id: &str) -> Result<()> {
        // 语义不变：**不移除**会话（reaper 还要靠 try_wait 读真实退出状态）。
        let live = self.live(session_id)?;
        let mut child = lock(&live.child)?;
        child
            .kill()
            .map_err(|error| Error::InvalidInput(format!("结束进程失败：{error}")))
    }

    fn try_wait(&self, session_id: &str) -> Result<Option<i32>> {
        // 未知会话保持现状语义：Ok(None)（不得改成报错）。
        let Ok(live) = self.live(session_id) else {
            return Ok(None);
        };
        let mut child = lock(&live.child)?;
        let status = child
            .try_wait()
            .map_err(|error| Error::InvalidInput(format!("查询进程状态失败：{error}")))?;
        Ok(status.map(|status| status.exit_code() as i32))
    }

    fn is_running(&self, session_id: &str) -> Result<bool> {
        // 未知会话保持现状语义：Ok(false)。
        let Ok(live) = self.live(session_id) else {
            return Ok(false);
        };
        let mut child = lock(&live.child)?;
        let status = child
            .try_wait()
            .map_err(|error| Error::InvalidInput(format!("查询进程状态失败：{error}")))?;
        Ok(status.is_none())
    }

    fn forget(&self, session_id: &str) -> Result<()> {
        lock(&self.sessions)?.remove(session_id);
        Ok(())
    }
```

- [ ] **Step 3: 跑单测 + clippy**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty:: && cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`
Expected: `test result: ok`、clippy 无警告

- [ ] **Step 4: 真机回归（结构改动的关键验证）**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test two_harness_concurrency -- --nocapture --test-threads=1`
Expected: `3 passed`（7D 并发用例在新锁结构下仍全绿）

- [ ] **Step 5: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/src/pty/portable_pty_backend.rs
git commit -m "refactor(pty): hold per-session locks instead of the global map lock during IO"
```

---

### Task 6: reaper 释放会话资源（spec §4.8）

**Files:**
- Modify: `src-tauri/src/pty/manager.rs`（`start_reading` 的 reaper 闭包 + fake 的 `forgotten` 记录）
- Test: `src-tauri/src/pty/input_tests.rs`

**Interfaces:**
- Consumes: `PtyManager::forget`、`SessionHandle::shutdown_input`（Task 2）
- Produces: reaper 在 `on_exit`（终态）之后走**固定顺序** `shutdown_input() → 从 manager map 移除 → backend.forget`，并在丢弃 PTY 前**有界**等待 reader 结束（`READER_DRAIN_GRACE = 250ms`，`Arc<AtomicBool>` 由 reader 线程置位）；fake 字段 `forgotten: Mutex<Vec<String>>`

- [ ] **Step 1: 写失败测试**

```rust
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
        PtyManager::with_input_capacity(
            Arc::clone(&backend),
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
                exited_for_sink.lock().expect("exited").push(session.to_string());
            }),
            64,
        )
    };
    spawn(&manager, "hub-a");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if manager.pending_bytes("hub-a").is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(manager.pending_bytes("hub-a"), None, "reaper 必须释放 handle");
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

    // 等 reader 把预置字节读完并 EOF（fake 的 try_wait 仍返回 None = 进程还在跑）
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
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::input_tests::the_reaper`
Expected: 编译错误 `no field forgotten`（fake 还没记录 forget）→ 补上字段后必须看到行为失败：
`pending_bytes` 仍是 `Some(..)`（reaper 还没释放）

- [ ] **Step 3: 实现**

fake：加 `pub forgotten: Mutex<Vec<String>>`，并在 `forget` 中 push：

```rust
        fn forget(&self, session_id: &str) -> Result<()> {
            self.forgotten
                .lock()
                .expect("forgotten")
                .push(session_id.to_string());
            Ok(())
        }
```

reaper 改为（**顺序固定**：终态 → 有界等待 reader → shutdown_input → 移除 handle → backend.forget）：

```rust
// 文件头新增：
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// reader 结束后的有界等待：让已经读到的最后字节交付完，避免「退出前最后几十字节被截断」
/// （spec §4.8 E2）。**有界**，所以不是 join，也绝不会让 reaper 卡住。
const READER_DRAIN_GRACE: Duration = Duration::from_millis(250);
```

```rust
        // ---- reader：只负责输出与 EOF；**绝不**触发资源回收（spec E1）----
        let on_output = Arc::clone(&self.on_output);
        let reader_session = session_id.to_string();
        let reader_finished = Arc::new(AtomicBool::new(false));
        let reader_done = Arc::clone(&reader_finished);
        thread::spawn(move || {
            let mut seq: u64 = 0;
            let mut reader = reader;
            let mut chunk = [0u8; 8192];

            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break, // EOF：仅表示输出流结束
                    Ok(read) => {
                        // 原样转发：这里绝不解析、绝不改写字节。
                        on_output(&reader_session, seq, &chunk[..read]);
                        seq += 1;
                    }
                    Err(_) => break,
                }
            }
            reader_done.store(true, Ordering::SeqCst);
        });

        // ---- reaper：唯一的退出状态事实来源 + 唯一的资源回收点 ----
        let backend = Arc::clone(&self.backend);
        let sessions = Arc::clone(&self.sessions);
        let on_exit = Arc::clone(&self.on_exit);
        let reaper_session = session_id.to_string();
        thread::spawn(move || loop {
            let exit_code = match backend.try_wait(&reaper_session) {
                Ok(Some(code)) => Some(code),
                // 仍在运行：继续等，**绝不猜测**
                Ok(None) => {
                    thread::sleep(REAP_INTERVAL);
                    continue;
                }
                // 连退出状态都读不到：如实上报「拿不到」，由上层映射成 unknown/lost
                Err(_) => None,
            };

            // 1) 终态：reaper 是唯一事实来源（forget 不决定 terminal state）。
            on_exit(&reaper_session, exit_code);

            // 2) 有界等待 reader 把已读到的字节交付完（E2；最多 READER_DRAIN_GRACE，必然会返回）。
            let deadline = Instant::now() + READER_DRAIN_GRACE;
            while !reader_finished.load(Ordering::SeqCst) && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }

            // 3) 资源回收，顺序固定（spec §4.8）：
            //    先 shutdown_input（让并发 write 拿到 InputClosed 而不是「未知会话」），
            //    再从 manager map 移除，最后丢 backend 句柄（drop LivePty = 关 PTY master）。
            let handle = sessions
                .lock()
                .ok()
                .and_then(|map| map.get(&reaper_session).cloned());
            if let Some(handle) = handle {
                handle.shutdown_input();
            }
            if let Ok(mut map) = sessions.lock() {
                map.remove(&reaper_session);
            }
            let _ = backend.forget(&reaper_session);
            break;
        });

        Ok(())
    }
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::`
Expected: `test result: ok`

- [ ] **Step 5: 真机回归（终态仍然正确）**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test claude_lifecycle -- --nocapture --test-threads=1`
Expected: `4 passed`（S1 user_killed / S2 natural_exit / S3 host_shutdown / S4 lost 不受影响）

- [ ] **Step 6: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/src/pty/manager.rs src-tauri/src/pty/input_tests.rs
git commit -m "fix(pty): release the session handle and PTY when the reaper sees the exit"
```

---

### Task 7: 真机 synthetic non-reader 验收（R1–R3）

**Files:**
- Create: `src-tauri/tests/runtime_input_backpressure.rs`

**Interfaces:**
- Consumes: `PtyManager::{with_input_capacity, spawn, start_reading, write, resize, kill, try_wait, pending_bytes}`、`PortablePtyBackend`
- Produces: 8A 的真机证据（R1–R4：硬期限 + 实测耗时 + 尾部不截断；`tests/e2e/README.md` 引用其输出）

- [ ] **Step 1: 写测试**

```rust
//! Task 8A 真机验收：**故意永不读 stdin 的 synthetic child**。
//!
//! 直接驱动生产 `PtyManager` + `PortablePtyBackend`（不经过 Codex/Claude，也不经过
//! TerminalRuntime），因为冻结点就在这一层。
//!
//! 合成 child：`cmd /c "ping -n 120 127.0.0.1 >nul"` —— cmd 与 ping 都不读 stdin。
//! 测试**自证**：若 child 其实会读 stdin，in-flight 批次会被结算、背压不会出现 → 用例失败。

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use harness_hub_lib::error::Error;
use harness_hub_lib::harness::launch::LaunchSpec;
use harness_hub_lib::pty::{PortablePtyBackend, PtyManager};

const CAPACITY: usize = 64 * 1024;

fn synthetic_spec() -> LaunchSpec {
    LaunchSpec {
        program: PathBuf::from("cmd"),
        args: vec!["/c".to_string(), "ping -n 120 127.0.0.1 >nul".to_string()],
        cwd: None,
        env: Vec::new(),
        runtime_target_id: "local".to_string(),
    }
}

fn manager() -> PtyManager {
    PtyManager::with_input_capacity(
        Arc::new(PortablePtyBackend::new()),
        Arc::new(|_session, _seq, _bytes| {}),
        Arc::new(|_session, _code| {}),
        CAPACITY,
    )
}

/// 硬期限 + 实测耗时：把「Runtime 冻结」变成可断言的失败，并留下证据数字。
fn timed<T>(timeout: Duration, label: &str, action: impl FnOnce() -> T) -> (T, Duration) {
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

fn pid_alive(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

#[test]
fn a_real_child_that_never_reads_stdin_does_not_freeze_the_runtime() {
    let manager = manager();
    let mut pids = Vec::new();
    for session in ["hub-a", "hub-b"] {
        let handle = manager.spawn(session, synthetic_spec(), 80, 24).expect("spawn");
        manager.start_reading(session).expect("start_reading");
        pids.push(handle.pid.expect("pid") as u32);
    }

    // R1：塞满容量。首批 64 KiB 被接受 → worker 会 park 在真实 OS 写里。
    manager.write("hub-a", &vec![b'x'; CAPACITY]).expect("首批必须被接受");

    // 自证 child 不读 stdin：2 秒后 in-flight 仍未结算（被读掉就会变 0）。
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        manager.pending_bytes("hub-a"),
        Some(CAPACITY),
        "child 若在读 stdin，in-flight 批次会被结算 —— 这条断言就是在证明它不读"
    );

    let (rejected, _) = timed(Duration::from_secs(2), "A 背压写入", || manager.write("hub-a", b"more"));
    match rejected.expect_err("队列必须满") {
        Error::InputBackpressure {
            pending_bytes,
            capacity_bytes,
            ..
        } => assert_eq!((pending_bytes, capacity_bytes), (CAPACITY, CAPACITY)),
        other => panic!("必须是 InputBackpressure，实际 {other:?}"),
    }

    // R2：A 的 writer 真 park 在 OS write 里时，B 与 A 的控制面都要在硬期限内返回。
    let (b_write, _) = timed(Duration::from_secs(2), "B write", || manager.write("hub-b", b"hello"));
    assert!(b_write.is_ok());
    let (b_resize, _) = timed(Duration::from_secs(2), "B resize", || manager.resize("hub-b", 100, 30));
    assert!(b_resize.is_ok());
    let (b_kill, _) = timed(Duration::from_secs(2), "B kill", || manager.kill("hub-b"));
    assert!(b_kill.is_ok());
    let (a_resize, _) = timed(Duration::from_secs(2), "A resize", || manager.resize("hub-a", 120, 40));
    assert!(a_resize.is_ok());
    let (a_kill, _) = timed(Duration::from_secs(2), "A kill", || manager.kill("hub-a"));
    assert!(a_kill.is_ok());
    let (a_wait, _) = timed(Duration::from_secs(2), "A try_wait", || manager.try_wait("hub-a"));
    assert!(a_wait.is_ok());

    // R3：kill 之后输入侧关闭
    assert!(matches!(manager.write("hub-a", b"x"), Err(Error::InputClosed { .. })));

    // 清理：定向树杀（8A 用 taskkill /T；8B 会换成 ProcessHandle::terminate_tree）
    for pid in &pids {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
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
#[test]
fn the_last_output_is_not_truncated_when_the_reaper_releases_the_session() {
    const MARKER: &str = "HH_TAIL_MARKER_271828";

    let chunks: Arc<std::sync::Mutex<Vec<u8>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&chunks);
    let manager = PtyManager::with_input_capacity(
        Arc::new(PortablePtyBackend::new()),
        Arc::new(move |_session, _seq, bytes| {
            sink.lock().expect("chunks").extend_from_slice(bytes);
        }),
        Arc::new(|_session, _code| {}),
        CAPACITY,
    );

    let handle = manager
        .spawn(
            "hub-tail",
            LaunchSpec {
                program: PathBuf::from("cmd"),
                args: vec!["/c".to_string(), format!("echo {MARKER}")],
                cwd: None,
                env: Vec::new(),
                runtime_target_id: "local".to_string(),
            },
            80,
            24,
        )
        .expect("spawn");
    manager.start_reading("hub-tail").expect("start_reading");

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if manager.pending_bytes("hub-tail").is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
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
    assert!(
        !pid_alive(handle.pid.expect("pid") as u32),
        "自己退出的 child 也必须真的结束"
    );
}
```

- [ ] **Step 2: 跑测试（含自证 + 截断回归）**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test runtime_input_backpressure -- --nocapture`
Expected: 两条都 PASS，且日志里 `pending_bytes` 2 秒后仍是 65536。若这条断言失败，说明
synthetic child 会读 stdin → 换备选 `pwsh -NoProfile -NonInteractive -Command "Start-Sleep -Seconds 120"` 重跑，
直到自证成立（**不要**放宽断言）。

- [ ] **Step 3: 再跑一次确认稳定（真机用例不能一次就信）**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test runtime_input_backpressure -- --nocapture`
Expected: 连续两次 PASS，并记录六条实测耗时

- [ ] **Step 4: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/tests/runtime_input_backpressure.rs
git commit -m "test(pty): a real child that never reads stdin can no longer freeze the runtime"
```

---

### Task 8: 7D 回归 + 全量 Gate + 证据归档

**Files:**
- Modify: `tests/e2e/README.md`（新增「Task 8A」小节）

**Interfaces:**
- Consumes: 全部前序 Task
- Produces: 8A 验收记录（真实输出）

- [ ] **Step 1: 7D 回归（新队列路径 + 新锁结构）**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib terminal::concurrency_tests && cargo test --manifest-path src-tauri/Cargo.toml --test two_harness_concurrency -- --nocapture --test-threads=1`
Expected: `7 passed` + `3 passed`

- [ ] **Step 2: 全量 Gate**

Run: `pnpm verify`（另跑 `pnpm python:test`）
Expected: 退出码 0；前端 124 用例；Rust 单测 = 260 + 新增

- [ ] **Step 3: Core diff 审计（无 Harness 特判、无新 DTO）**

```bash
cd D:/HarnessHub
git diff --stat origin/main..HEAD
grep -rn "codex\|claude" src-tauri/src/pty/ src-tauri/src/error.rs   # 只应出现在测试数据/注释
grep -rn "InputBackpressure\|InputClosed\|InputWorkerFailed" src-tauri/src
```
Expected: 改动只落在 `pty/*`、`error.rs`、测试与文档；`pty/` 无 Harness 分支

- [ ] **Step 4: 写证据到 `tests/e2e/README.md`**

新增小节，粘入 Task 7 Step 2/3 的两次 PASS 输出与六条实测耗时，并写清：

```text
8A 已证明：A 的 writer 永久 park 时，整台 Runtime、B 会话、以及 A 自己的 kill/resize/reap 都不冻结。
8A 未证明（留给 8B）：回收被 OS syscall 卡死的 writer 线程本身。
```

- [ ] **Step 5: 提交**

```bash
cd D:/HarnessHub
git add tests/e2e/README.md
git commit -m "docs(e2e): record the Task 8A input-path acceptance and its measured latencies"
```

---

## 完成标准

```text
✓ spec §8 的每一条 Gate 都有测试对应（T1–T12 + R1–R4 + 7D 回归）
✓ pnpm verify 退出码 0；7D 全部回归绿
✓ INV-1..INV-4 在 portable_pty_backend.rs 里逐条可读
✓ `pending_bytes` 含 in-flight 由 T2 与 R1 两处锁死（单元 + 真机）
✓ 无 join、无 Harness 特判、无新 IPC DTO、UI 无改动
✓ 证据落在 tests/e2e/README.md（真实输出，不是结论）
```
