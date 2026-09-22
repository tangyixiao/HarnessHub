//! PTY 会话管理：**reader 与 reaper 是两条独立生命周期**。
//!
//! ```text
//! reader 线程：输出字节 + EOF        —— 只表达「输出流结束了」
//! reaper 线程：wait / try_wait       —— **唯一**的退出状态事实来源
//! ```
//!
//! 为什么必须分开（docs/adr/0010-reader-vs-reaper.md）：
//!
//! ```text
//! PTY EOF ≠ 已经拿到 exit status
//! ```
//!
//! 把 EOF 当成「进程结束了，猜个退出码」会在下面这种情形下说谎：
//! 子进程 fork 出后台进程、或 shell 提前关闭了 pty —— 输出流结束但进程还在跑。
//! 因此本模块**禁止**由 EOF 推导退出码。
//!
//! 仍然只是传输层：**不解析字节内容**，保证顺序、字节完整与退出上报。

use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::harness::launch::LaunchSpec;
use crate::pty::backend::{PtyBackend, PtyProcessHandle};

/// 原始输出回调：`(session_id, seq, bytes)`。
pub type OutputSink = Arc<dyn Fn(&str, u64, &[u8]) + Send + Sync>;

/// 进程结束回调：`(session_id, exit_code)`。**只由 reaper 线程调用。**
pub type ExitSink = Arc<dyn Fn(&str, Option<i32>) + Send + Sync>;

/// reaper 轮询间隔。
const REAP_INTERVAL: Duration = Duration::from_millis(50);

pub struct PtyManager {
    backend: Arc<dyn PtyBackend>,
    on_output: OutputSink,
    on_exit: ExitSink,
    /// spawn 时取好读端，等调用方把会话标记为 `running` 之后再启动读线程。
    ///
    /// 为什么必须分开：进程可能 spawn 成功后**立刻**退出。如果读线程在
    /// `mark_running` 之前就跑起来，EOF 会早于状态迁移到达，`finish()` 打在
    /// 还是 `created` 的会话上就会失败，会话永远卡在 `created`
    /// （真实竞态，已由 terminal 编排测试抓出）。
    /// 把「取读端」与「开始读」拆开，顺序就由调用方确定，不再依赖线程调度。
    pending_readers: Mutex<HashMap<String, Box<dyn Read + Send>>>,
}

impl PtyManager {
    pub fn new(backend: Arc<dyn PtyBackend>, on_output: OutputSink, on_exit: ExitSink) -> Self {
        Self {
            backend,
            on_output,
            on_exit,
            pending_readers: Mutex::new(HashMap::new()),
        }
    }

    /// spawn 并**取好读端**，但此时还没有读线程。
    ///
    /// 调用方必须在会话成功迁移到 `running` 之后调用 [`Self::start_reading`]。
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

        Ok(handle)
    }

    /// 启动 reader 与 reaper 线程。**必须在 `mark_running` 成功之后调用。**
    ///
    /// 两个线程职责严格分离：
    /// - reader：按序转发输出；读到 EOF 就结束，**不碰退出码**；
    /// - reaper：轮询 `try_wait`，只有拿到真实退出状态才调用 [`ExitSink`]。
    pub fn start_reading(&self, session_id: &str) -> Result<()> {
        let reader = self
            .pending_readers
            .lock()
            .map_err(|_| Error::StateLockPoisoned)?
            .remove(session_id)
            .ok_or_else(|| Error::InvalidInput(format!("会话没有待读取的 PTY：{session_id}")))?;

        // ---- reader：只负责输出与 EOF ----
        let on_output = Arc::clone(&self.on_output);
        let reader_session = session_id.to_string();
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
        });

        // ---- reaper：唯一的退出状态事实来源 ----
        let backend = Arc::clone(&self.backend);
        let on_exit = Arc::clone(&self.on_exit);
        let reaper_session = session_id.to_string();
        thread::spawn(move || loop {
            match backend.try_wait(&reaper_session) {
                Ok(Some(code)) => {
                    on_exit(&reaper_session, Some(code));
                    break;
                }
                // 仍在运行：继续等，**绝不猜测**
                Ok(None) => thread::sleep(REAP_INTERVAL),
                Err(_) => {
                    // 连退出状态都读不到：如实上报「拿不到」，由上层映射成 unknown/lost
                    on_exit(&reaper_session, None);
                    break;
                }
            }
        });

        Ok(())
    }

    pub fn write(&self, session_id: &str, bytes: &[u8]) -> Result<()> {
        self.backend.write(session_id, bytes)
    }

    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<()> {
        if cols == 0 || rows == 0 {
            return Err(Error::InvalidInput(
                "PTY 尺寸不能为 0：终端模拟器可能还没完成布局".to_string(),
            ));
        }
        self.backend.resize(session_id, cols, rows)
    }

    pub fn kill(&self, session_id: &str) -> Result<()> {
        self.backend.kill(session_id)
    }

    pub fn is_running(&self, session_id: &str) -> Result<bool> {
        self.backend.is_running(session_id)
    }

    pub fn try_wait(&self, session_id: &str) -> Result<Option<i32>> {
        self.backend.try_wait(session_id)
    }

    /// 丢弃会话句柄（终态写入之后调用）。
    pub fn forget(&self, session_id: &str) -> Result<()> {
        self.backend.forget(session_id)
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! 可编程的假后端：让 manager / 编排逻辑可以脱离真实进程测试。

    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    use crate::pty::backend::PtySpawnRequest;

    /// 每次 `read` 只吐出一块，用来精确验证「按序分块转发」与跨块字节完整性。
    struct BlockReader {
        blocks: VecDeque<Vec<u8>>,
        current: Vec<u8>,
        offset: usize,
    }

    impl Read for BlockReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }

            if self.current.is_empty() {
                match self.blocks.pop_front() {
                    Some(block) => {
                        self.current = block;
                        self.offset = 0;
                    }
                    None => return Ok(0),
                }
            }

            let remaining = &self.current[self.offset..];
            let read = remaining.len().min(buf.len());
            buf[..read].copy_from_slice(&remaining[..read]);
            self.offset += read;
            if self.offset >= self.current.len() {
                self.current.clear();
            }

            Ok(read)
        }
    }

    #[derive(Default)]
    pub struct FakePtyBackend {
        pub spawned: Mutex<Vec<PtySpawnRequest>>,
        pub written: Mutex<Vec<(String, Vec<u8>)>>,
        pub resized: Mutex<Vec<(String, u16, u16)>>,
        pub killed: Mutex<Vec<String>>,
        /// 读端返回的字节（spawn 时按顺序弹出）。
        outputs: Mutex<Vec<Vec<u8>>>,
        /// try_wait 的返回值；`None` 表示仍在运行。
        exit_codes: Mutex<HashMap<String, Option<i32>>>,
        /// 为真时 spawn 直接失败（模拟 binary 缺失、PTY 创建失败等）。
        fail_spawn: Mutex<bool>,
        /// 为真时所有会话都视为「已退出」，退出码取 `always_exit_code`。
        always_exited: Mutex<bool>,
        always_exit_code: Mutex<Option<i32>>,
        /// 被 kill 后进程的退出码（模拟真实被终止的进程）。
        killed_exit_code: i32,
        pub pid: Option<u32>,
    }

    impl FakePtyBackend {
        pub fn new() -> Self {
            Self {
                pid: Some(4242),
                killed_exit_code: 137,
                ..Self::default()
            }
        }

        /// 预先安排若干块输出。第一块读完即 EOF。
        pub fn with_output(self, blocks: Vec<Vec<u8>>) -> Self {
            *self.outputs.lock().expect("outputs") = blocks;
            self
        }

        /// 立即“已退出”，`try_wait` 返回给定码。
        pub fn with_exit_code(self, session_id: &str, code: Option<i32>) -> Self {
            self.exit_codes
                .lock()
                .expect("exit_codes")
                .insert(session_id.to_string(), code);
            self
        }

        /// 让 spawn 失败。
        pub fn failing_spawn(self) -> Self {
            *self.fail_spawn.lock().expect("fail_spawn") = true;
            self
        }

        /// 所有会话都视为已退出（测试里 session id 是 UUID，无法预先登记）。
        pub fn with_always_exited(self, code: Option<i32>) -> Self {
            *self.always_exited.lock().expect("always_exited") = true;
            *self.always_exit_code.lock().expect("always_exit_code") = code;
            self
        }
    }

    impl PtyBackend for FakePtyBackend {
        fn spawn(&self, request: PtySpawnRequest) -> Result<PtyProcessHandle> {
            if *self.fail_spawn.lock().expect("fail_spawn") {
                return Err(Error::InvalidInput("fake: spawn 失败".to_string()));
            }

            self.spawned.lock().expect("spawned").push(request.clone());
            Ok(PtyProcessHandle {
                session_id: request.session_id,
                pid: self.pid,
            })
        }

        fn take_reader(&self, _session_id: &str) -> Result<Box<dyn Read + Send>> {
            Ok(Box::new(BlockReader {
                blocks: self.outputs.lock().expect("outputs").drain(..).collect(),
                current: Vec::new(),
                offset: 0,
            }))
        }

        fn write(&self, session_id: &str, bytes: &[u8]) -> Result<()> {
            self.written
                .lock()
                .expect("written")
                .push((session_id.to_string(), bytes.to_vec()));
            Ok(())
        }

        fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<()> {
            self.resized
                .lock()
                .expect("resized")
                .push((session_id.to_string(), cols, rows));
            Ok(())
        }

        fn kill(&self, session_id: &str) -> Result<()> {
            self.killed
                .lock()
                .expect("killed")
                .push(session_id.to_string());

            // 真实语义：kill 之后进程**会**退出，reaper 随后就能读到退出状态。
            if !*self.always_exited.lock().expect("always_exited") {
                self.exit_codes
                    .lock()
                    .expect("exit_codes")
                    .insert(session_id.to_string(), Some(self.killed_exit_code));
            }
            Ok(())
        }

        fn try_wait(&self, session_id: &str) -> Result<Option<i32>> {
            if *self.always_exited.lock().expect("always_exited") {
                return Ok(*self.always_exit_code.lock().expect("always_exit_code"));
            }

            Ok(self
                .exit_codes
                .lock()
                .expect("exit_codes")
                .get(session_id)
                .copied()
                .unwrap_or(None))
        }

        fn is_running(&self, session_id: &str) -> Result<bool> {
            Ok(self.try_wait(session_id)?.is_none())
        }

        fn forget(&self, _session_id: &str) -> Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakePtyBackend;

    /// 测试用的分块收集器。
    type CapturedChunks = Arc<Mutex<Vec<(u64, Vec<u8>)>>>;
    /// 测试用的退出收集器。
    type CapturedExits = Arc<Mutex<Vec<(String, Option<i32>)>>>;
    use super::*;
    use crate::harness::launch::LaunchSpec;
    use std::path::PathBuf;
    use std::sync::Mutex;

    fn spec() -> LaunchSpec {
        LaunchSpec {
            program: PathBuf::from("codex"),
            args: vec!["--version".to_string()],
            cwd: None,
            env: Vec::new(),
            runtime_target_id: "local".to_string(),
        }
    }

    fn manager(backend: Arc<FakePtyBackend>) -> (PtyManager, CapturedChunks) {
        let chunks: CapturedChunks = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&chunks);
        let manager = PtyManager::new(
            backend,
            Arc::new(move |_session, seq, bytes| {
                sink.lock().expect("chunks").push((seq, bytes.to_vec()));
            }),
            Arc::new(|_session, _code| {}),
        );
        (manager, chunks)
    }

    #[test]
    fn spawn_forwards_the_structured_spec_untouched() {
        let backend = Arc::new(FakePtyBackend::new());
        let (manager, _chunks) = manager(Arc::clone(&backend));

        let handle = manager
            .spawn("hub-1", spec(), 120, 30)
            .expect("spawn 必须成功");
        manager.start_reading("hub-1").expect("开始读取");

        assert_eq!(handle.pid, Some(4242), "pid 要带回来用于诊断");
        let spawned = backend.spawned.lock().expect("spawned");
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].spec.program, PathBuf::from("codex"));
        assert_eq!(spawned[0].spec.args, vec!["--version".to_string()]);
        assert_eq!((spawned[0].cols, spawned[0].rows), (120, 30));
    }

    #[test]
    fn output_is_forwarded_in_order_and_byte_exact() {
        let backend = Arc::new(FakePtyBackend::new().with_output(vec![
            vec![0x1b, b'[', b'6', b'n'], // 含转义序列的原始字节
            vec![0xe4, 0xbd],             // 「你」的前两个字节
            vec![0xa0],                   // 第三个字节：多字节字符被切在两个 chunk 之间
        ]));
        let (manager, chunks) = manager(Arc::clone(&backend));

        manager.spawn("hub-1", spec(), 80, 24).expect("spawn");
        manager.start_reading("hub-1").expect("开始读取");

        // 读线程异步，等它跑完
        for _ in 0..50 {
            if chunks.lock().expect("chunks").len() >= 3 {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }

        let captured = chunks.lock().expect("chunks").clone();
        assert_eq!(captured.len(), 3, "三块必须分别转发");
        assert_eq!(captured[0].0, 0);
        assert_eq!(captured[1].0, 1);
        assert_eq!(captured[2].0, 2, "seq 必须单调递增且从 0 开始");

        // 字节必须原样保留：转义序列不被吞，跨块 UTF-8 不被损坏
        assert_eq!(captured[0].1, vec![0x1b, b'[', b'6', b'n']);
        let joined: Vec<u8> = captured.iter().flat_map(|(_, b)| b.clone()).collect();
        assert_eq!(
            String::from_utf8(joined).expect("跨块拼接后必须是合法 UTF-8"),
            "\u{1b}[6n你"
        );
    }

    #[test]
    fn exit_code_is_reported_after_eof() {
        let backend = Arc::new(
            FakePtyBackend::new()
                .with_output(vec![b"done".to_vec()])
                .with_exit_code("hub-1", Some(0)),
        );
        let exits: CapturedExits = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&exits);
        let manager = PtyManager::new(
            backend,
            Arc::new(|_session, _seq, _bytes| {}),
            Arc::new(move |session, code| {
                sink.lock()
                    .expect("exits")
                    .push((session.to_string(), code));
            }),
        );

        manager.spawn("hub-1", spec(), 80, 24).expect("spawn");
        manager.start_reading("hub-1").expect("开始读取");

        for _ in 0..50 {
            if !exits.lock().expect("exits").is_empty() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }

        assert_eq!(
            exits.lock().expect("exits").clone(),
            vec![("hub-1".to_string(), Some(0))]
        );
    }

    #[test]
    fn write_resize_and_kill_are_delegated_verbatim() {
        let backend = Arc::new(FakePtyBackend::new());
        let (manager, _chunks) = manager(Arc::clone(&backend));
        manager.spawn("hub-1", spec(), 80, 24).expect("spawn");
        manager.start_reading("hub-1").expect("开始读取");

        manager.write("hub-1", b"ls\r").expect("写入");
        manager.resize("hub-1", 100, 40).expect("调整尺寸");
        manager.kill("hub-1").expect("结束");

        assert_eq!(
            backend.written.lock().expect("written").clone(),
            vec![("hub-1".to_string(), b"ls\r".to_vec())]
        );
        assert_eq!(
            backend.resized.lock().expect("resized").clone(),
            vec![("hub-1".to_string(), 100, 40)]
        );
        assert_eq!(
            backend.killed.lock().expect("killed").clone(),
            vec!["hub-1".to_string()]
        );
    }

    #[test]
    fn zero_sized_resize_is_rejected_with_a_clear_error() {
        let backend = Arc::new(FakePtyBackend::new());
        let (manager, _chunks) = manager(Arc::clone(&backend));
        manager.spawn("hub-1", spec(), 80, 24).expect("spawn");
        manager.start_reading("hub-1").expect("开始读取");

        let error = manager.resize("hub-1", 0, 40).expect_err("必须拒绝");

        assert!(
            error.to_string().contains("尺寸"),
            "错误信息要说明原因：{error}"
        );
        assert!(
            backend.resized.lock().expect("resized").is_empty(),
            "被拒绝的 resize 不得透传到后端"
        );
    }
}
