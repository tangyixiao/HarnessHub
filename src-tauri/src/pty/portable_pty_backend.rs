//! `portable-pty` 实现：**raw transport，不碰任何转义序列**。
//!
//! Windows 实测结论（见 `tests/windows_pty_spike.rs`）：
//! - `.cmd` shim 可以被直接 spawn，**不需要**包命令解释器；
//! - PTY 宿主必须由终端模拟器应答 DSR，否则交互式 CLI 会永久等待 ——
//!   但那个应答**不属于本文件**（见 `pty::backend` 的边界说明）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::Mutex;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use crate::error::{Error, Result};
use crate::pty::backend::{PtyBackend, PtyProcessHandle, PtySpawnRequest};

struct LivePty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
}

/// 真实 PTY 后端。**只有这个文件依赖 `portable-pty`**，其余代码只认 [`PtyBackend`]。
#[derive(Default)]
pub struct PortablePtyBackend {
    sessions: Mutex<HashMap<String, LivePty>>,
}

impl PortablePtyBackend {
    pub fn new() -> Self {
        Self::default()
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
            LivePty {
                master: pair.master,
                writer,
                child,
            },
        );

        Ok(PtyProcessHandle {
            session_id: request.session_id,
            pid,
        })
    }

    fn take_reader(&self, session_id: &str) -> Result<Box<dyn Read + Send>> {
        let sessions = lock(&self.sessions)?;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| Error::InvalidInput(format!("未知会话：{session_id}")))?;

        session
            .master
            .try_clone_reader()
            .map_err(|error| Error::InvalidInput(format!("获取 PTY 读端失败：{error}")))
    }

    fn write(&self, session_id: &str, bytes: &[u8]) -> Result<()> {
        let mut sessions = lock(&self.sessions)?;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| Error::InvalidInput(format!("未知会话：{session_id}")))?;

        session
            .writer
            .write_all(bytes)
            .map_err(|error| Error::InvalidInput(format!("写入 PTY 失败：{error}")))?;
        session
            .writer
            .flush()
            .map_err(|error| Error::InvalidInput(format!("刷新 PTY 失败：{error}")))?;
        Ok(())
    }

    fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<()> {
        let sessions = lock(&self.sessions)?;
        let session = sessions
            .get(session_id)
            .ok_or_else(|| Error::InvalidInput(format!("未知会话：{session_id}")))?;

        session
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| Error::InvalidInput(format!("调整 PTY 尺寸失败：{error}")))
    }

    fn kill(&self, session_id: &str) -> Result<()> {
        let mut sessions = lock(&self.sessions)?;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| Error::InvalidInput(format!("未知会话：{session_id}")))?;

        // **不移除**会话：reaper 还要靠 try_wait 读到真实退出状态。
        // 移除会让 try_wait 永远返回 None，终态就永远写不下去。
        session
            .child
            .kill()
            .map_err(|error| Error::InvalidInput(format!("结束进程失败：{error}")))
    }

    fn try_wait(&self, session_id: &str) -> Result<Option<i32>> {
        let mut sessions = lock(&self.sessions)?;
        let Some(session) = sessions.get_mut(session_id) else {
            return Ok(None);
        };

        let status = session
            .child
            .try_wait()
            .map_err(|error| Error::InvalidInput(format!("查询进程状态失败：{error}")))?;

        Ok(status.map(|status| status.exit_code() as i32))
    }

    /// 显式丢弃会话（终态写入之后由上层调用，释放 PTY 句柄）。
    fn forget(&self, session_id: &str) -> Result<()> {
        lock(&self.sessions)?.remove(session_id);
        Ok(())
    }

    fn is_running(&self, session_id: &str) -> Result<bool> {
        let mut sessions = lock(&self.sessions)?;
        let Some(session) = sessions.get_mut(session_id) else {
            return Ok(false);
        };

        let status = session
            .child
            .try_wait()
            .map_err(|error| Error::InvalidInput(format!("查询进程状态失败：{error}")))?;

        Ok(status.is_none())
    }
}
