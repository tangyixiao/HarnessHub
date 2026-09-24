//! `portable-pty` 实现：**raw transport，不碰任何转义序列**。
//!
//! Windows 实测结论（见 `tests/windows_pty_spike.rs`）：
//! - `.cmd` shim 可以被直接 spawn，**不需要**包命令解释器；
//! - PTY 宿主必须由终端模拟器应答 DSR，否则交互式 CLI 会永久等待 ——
//!   但那个应答**不属于本文件**（见 `pty::backend` 的边界说明）。
//!
//! ## 锁粒度（Task 8A 的不变量，见 ADR-0009 与 8A 设计文档）
//!
//! ```text
//! INV-1  任何可能阻塞的 OS I/O 不得发生在跨会话锁持有期间
//! INV-3  kill / try_wait / resize / take_reader 不得依赖 writer 的锁
//! INV-4  全局 sessions 锁只用于「查找 → clone Arc / 插入 / 删除」
//! ```
//!
//! 为什么必须这样：`write_all` 会在子进程不读 stdin 时**永久阻塞**。以前它是在持有
//! `sessions` 全局锁时调用的，于是一条会话的阻塞写会把 `resize` / `kill` / `try_wait`
//! （reaper）全部冻结 —— 连「结束会话」这个唯一的恢复手段都进不去（7D 真机实测：
//! 15+ 分钟静默挂死，测试进程 CPU ≈ 0）。现在阻塞写只 park 在**该会话自己的** writer 锁上。
//!
//! 统一访问模式（本文件所有方法都必须长这样）：
//!
//! ```text
//! let live = { sessions.lock()?.get(id).cloned() };  // 全局锁到此必须已释放
//! live.writer.lock() / live.master.lock() / live.child.lock()
//! ```
//!
//! **禁止** `global map lock → session-local mutex → OS call` 这种嵌套。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use crate::error::{Error, Result};
use crate::pty::backend::{PtyBackend, PtyProcessHandle, PtySpawnRequest};

/// 一条会话的三个互不依赖的句柄，各自一把锁。
///
/// 拆成三把锁是刻意的：阻塞写只能占住 `writer`，`kill` / `try_wait` 走 `child`，
/// `resize` 走 `master` —— 三者互不等待（INV-3）。
struct LivePty {
    /// 只有写入会 park 在这把锁上。
    writer: Mutex<Box<dyn Write + Send>>,
    /// resize / try_clone_reader。
    master: Mutex<Box<dyn MasterPty + Send>>,
    /// kill / try_wait / is_running。
    child: Mutex<Box<dyn Child + Send + Sync>>,
}

/// 真实 PTY 后端。**只有这个文件依赖 `portable-pty`**，其余代码只认 [`PtyBackend`]。
#[derive(Default)]
pub struct PortablePtyBackend {
    /// 只用于「查找 → clone `Arc` / 插入 / 删除」；**绝不**在持有期间做 OS 调用。
    sessions: Mutex<HashMap<String, Arc<LivePty>>>,
}

impl PortablePtyBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// 统一访问模式：全局锁只活在这几行里，返回 `Arc` 之后调用方才去碰 per-session 锁。
    fn live(&self, session_id: &str) -> Result<Arc<LivePty>> {
        lock(&self.sessions)?
            .get(session_id)
            .cloned()
            .ok_or_else(|| Error::InvalidInput(format!("未知会话：{session_id}")))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>> {
    mutex.lock().map_err(|_| Error::StateLockPoisoned)
}

impl PtyBackend for PortablePtyBackend {
    fn spawn(&self, request: PtySpawnRequest) -> Result<PtyProcessHandle> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: request.rows,
                cols: request.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| Error::InvalidInput(format!("打开 PTY 失败：{error}")))?;

        // LaunchSpec 是结构化的：program / args / cwd / env 分别传入，
        // 这里**不做**任何字符串拼接，也不理解 Harness 语义。
        let mut builder = CommandBuilder::new(&request.spec.program);
        builder.args(&request.spec.args);
        if let Some(cwd) = &request.spec.cwd {
            builder.cwd(cwd);
        }
        for (key, value) in &request.spec.env {
            builder.env(key, value);
        }

        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|error| Error::InvalidInput(format!("启动进程失败：{error}")))?;
        // 父进程必须丢掉 slave，否则子进程永远拿不到 EOF。
        drop(pair.slave);

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

        // 这一行可能在子进程不读 stdin 时阻塞很久 —— 但只 park 本会话的 writer 锁，
        // 不再冻结任何别的会话或本会话的 kill/resize/reap（INV-1/INV-3）。
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
        let live = self.live(session_id)?;
        let mut child = lock(&live.child)?;

        // **不移除**会话：reaper 还要靠 try_wait 读到真实退出状态。
        // 移除会让 try_wait 永远返回 None，终态就永远写不下去。
        //
        // 也**不等** writer 的锁：子进程被卡住的写不能阻挡「结束会话」（INV-3）。
        child
            .kill()
            .map_err(|error| Error::InvalidInput(format!("结束进程失败：{error}")))
    }

    fn try_wait(&self, session_id: &str) -> Result<Option<i32>> {
        // 未知会话保持原有语义（`Ok(None)`），不改成报错：reaper 依赖这条。
        let Ok(live) = self.live(session_id) else {
            return Ok(None);
        };
        let mut child = lock(&live.child)?;

        let status = child
            .try_wait()
            .map_err(|error| Error::InvalidInput(format!("查询进程状态失败：{error}")))?;

        Ok(status.map(|status| status.exit_code() as i32))
    }

    /// 显式丢弃会话（终态写入之后由上层调用，释放 PTY 句柄）。
    fn forget(&self, session_id: &str) -> Result<()> {
        // 全局锁只做 remove；返回的 `Arc<LivePty>` 在这里被 drop，
        // 于是 master 关闭、writer 释放 —— park 在写里的 worker 有机会拿到错误并退出。
        lock(&self.sessions)?.remove(session_id);
        Ok(())
    }

    fn is_running(&self, session_id: &str) -> Result<bool> {
        // 未知会话保持原有语义（`Ok(false)`）。
        let Ok(live) = self.live(session_id) else {
            return Ok(false);
        };
        let mut child = lock(&live.child)?;

        let status = child
            .try_wait()
            .map_err(|error| Error::InvalidInput(format!("查询进程状态失败：{error}")))?;

        Ok(status.is_none())
    }
}
