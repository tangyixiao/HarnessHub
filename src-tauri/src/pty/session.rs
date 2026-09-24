//! 每会话输入调度：**有界**队列 + 专属 writer worker。
//!
//! ```text
//! TerminalRuntime::write → SessionHandle::try_enqueue   （O(1)，永不阻塞）
//!                              ↓ 有界队列（byte 记账）
//!                    writer worker → PtyBackend::write   （阻塞 primitive，只在这里调用）
//! ```
//!
//! 硬约定：
//! 1. `pending_bytes` **包含正在写的那一批**，只有 `backend.write` 返回后才结算 ——
//!    否则 worker 一 pop 就减账，容量可以被「pop 后重填」绕过（spec §4.3）；
//! 2. `shutdown_input` **绝不 join**（drop `JoinHandle` = detach）—— worker 可能正 park 在
//!    阻塞的 OS 写里，join 会把本 Task 要修的问题从后门放回来（spec §4.7）。

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
    ///
    /// **不在 pop 时减 `pending_bytes`**：那一批仍在 `backend.write` 里，属于 in-flight。
    /// 减账只发生在 [`Self::finish_batch`]（写入返回之后），否则容量可以被「pop 后重填」绕过。
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
                match backend.write(&worker_session, &batch) {
                    Ok(()) => worker_input.finish_batch(batch.len()),
                    // Task 3 会把这里换成「标记输入侧失败 + 丢弃未发送队列」。
                    Err(_) => break,
                }
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
