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
    //!
    //! **按 `LaunchSpec.program` 分流输出与 pid**（7D 并发隔离矩阵需要同时存在两条会话）。
    //! 为什么不是按 `session_id`：`hub_session_id` 由 runtime 内部生成，测试在 spawn 之前
    //! 无法预知它；program 是「这是哪条会话」在 spawn 前唯一可确定的线索。
    //! 生产侧 `PortablePtyBackend` 本来就是按 `session_id` 存的，这里只是让 fake 也能表达
    //! 「哪条会话该收到哪些字节」—— 否则隔离断言测不出真话（7D-A 的 RED 就是这么来的）。

    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Condvar, Mutex};

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

    /// 可**实时追加**的输出流。
    ///
    /// 为什么需要它：并发矩阵要证明「另一条会话被 kill 之后，这一条**仍能继续产出**」，
    /// 而一次性预置的块在 spawn 时就被读完了。`LiveStream` 允许测试在任意时刻
    /// `push_output` / `close_output`，而 reader 仍是**生产代码**在按序转发。
    struct LiveStream {
        state: Mutex<LiveState>,
        ready: Condvar,
    }

    #[derive(Default)]
    struct LiveState {
        queue: VecDeque<Vec<u8>>,
        closed: bool,
    }

    impl LiveStream {
        fn new(blocks: Vec<Vec<u8>>) -> Arc<Self> {
            let stream = Arc::new(Self {
                state: Mutex::new(LiveState::default()),
                ready: Condvar::new(),
            });
            for block in blocks {
                stream.push(block);
            }
            stream
        }

        fn push(&self, block: Vec<u8>) {
            let mut state = self.state.lock().expect("live stream");
            state.queue.push_back(block);
            self.ready.notify_all();
        }

        /// 关闭流（read 返回 0 = EOF）。**EOF 只表示输出结束，不表示进程退出。**
        fn close(&self) {
            let mut state = self.state.lock().expect("live stream");
            state.closed = true;
            self.ready.notify_all();
        }

        fn reader(self: &Arc<Self>) -> Box<dyn Read + Send> {
            Box::new(LiveReader {
                stream: Arc::clone(self),
            })
        }

        fn poisoned() -> std::io::Error {
            std::io::Error::other("fake live stream 锁中毒")
        }
    }

    struct LiveReader {
        stream: Arc<LiveStream>,
    }

    impl Read for LiveReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }

            let mut state = self
                .stream
                .state
                .lock()
                .map_err(|_| LiveStream::poisoned())?;
            loop {
                if let Some(block) = state.queue.pop_front() {
                    let read = block.len().min(buf.len());
                    buf[..read].copy_from_slice(&block[..read]);
                    if read < block.len() {
                        state.queue.push_front(block[read..].to_vec());
                    }
                    return Ok(read);
                }
                if state.closed {
                    return Ok(0);
                }
                state = self
                    .stream
                    .ready
                    .wait(state)
                    .map_err(|_| LiveStream::poisoned())?;
            }
        }
    }

    #[derive(Default)]
    pub struct FakePtyBackend {
        pub spawned: Mutex<Vec<PtySpawnRequest>>,
        pub written: Mutex<Vec<(String, Vec<u8>)>>,
        pub resized: Mutex<Vec<(String, u16, u16)>>,
        pub killed: Mutex<Vec<String>>,
        /// 读端返回的字节（spawn 时按顺序弹出）。**共享队列**：只适合单会话测试。
        outputs: Mutex<Vec<Vec<u8>>>,
        /// 按 program 分流的实时输出流（可并发、可运行期追加）。
        live_outputs: Mutex<HashMap<String, Arc<LiveStream>>>,
        /// 按 program 指定的 pid（默认 [`Self::pid`]）：并发矩阵要求两条会话 pid 不同。
        pids_by_program: Mutex<HashMap<String, u32>>,
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
        ///
        /// **单会话专用**：多条会话同时读会抢同一个队列（见模块文档）。
        pub fn with_output(self, blocks: Vec<Vec<u8>>) -> Self {
            *self.outputs.lock().expect("outputs") = blocks;
            self
        }

        /// 为某个 program 预置输出，且流**保持打开**（会话不会因 EOF 结束）。
        pub fn with_output_for_program(self, program: &str, blocks: Vec<Vec<u8>>) -> Self {
            self.live_outputs
                .lock()
                .expect("live_outputs")
                .insert(program.to_string(), LiveStream::new(blocks));
            self
        }

        /// 指定某个 program 的 pid，让并发矩阵能断言「两条会话 pid 不同」。
        pub fn with_pid_for_program(self, program: &str, pid: u32) -> Self {
            self.pids_by_program
                .lock()
                .expect("pids_by_program")
                .insert(program.to_string(), pid);
            self
        }

        /// 运行期追加输出：已启动的 reader 会按序读到它。
        pub fn push_output(&self, program: &str, block: Vec<u8>) {
            self.live_stream(program).push(block);
        }

        /// 关闭某个 program 的输出流（EOF）。
        pub fn close_output(&self, program: &str) {
            self.live_stream(program).close();
        }

        fn live_stream(&self, program: &str) -> Arc<LiveStream> {
            let mut streams = self.live_outputs.lock().expect("live_outputs");
            Arc::clone(
                streams
                    .entry(program.to_string())
                    .or_insert_with(|| LiveStream::new(Vec::new())),
            )
        }

        fn program_of(&self, session_id: &str) -> Option<String> {
            self.spawned
                .lock()
                .expect("spawned")
                .iter()
                .find(|request| request.session_id == session_id)
                .map(|request| request.spec.program.to_string_lossy().into_owned())
        }

        /// 立即“已退出”，`try_wait` 返回给定码。
        pub fn with_exit_code(self, session_id: &str, code: Option<i32>) -> Self {
            self.exit_codes
                .lock()
                .expect("exit_codes")
                .insert(session_id.to_string(), code);
            self
        }

        /// 运行期让某个会话“自己退出”（`try_wait` 从此返回给定码）。
        ///
        /// 与 [`Self::kill`] 的区别：这条路径**没有**用户 kill 意图，
        /// 因此会被记成 `natural_exit` —— 并发矩阵要能分别制造两种终态。
        pub fn exit_session(&self, session_id: &str, code: Option<i32>) {
            self.exit_codes
                .lock()
                .expect("exit_codes")
                .insert(session_id.to_string(), code);
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

            let program = request.spec.program.to_string_lossy().into_owned();
            let pid = self
                .pids_by_program
                .lock()
                .expect("pids_by_program")
                .get(&program)
                .copied()
                .or(self.pid);

            self.spawned.lock().expect("spawned").push(request.clone());
            Ok(PtyProcessHandle {
                session_id: request.session_id,
                pid,
            })
        }

        fn take_reader(&self, session_id: &str) -> Result<Box<dyn Read + Send>> {
            // 先看有没有针对这个 program 的实时流；没有才回退到共享队列。
            if let Some(program) = self.program_of(session_id) {
                let live = self
                    .live_outputs
                    .lock()
                    .expect("live_outputs")
                    .get(&program)
                    .cloned();
                if let Some(stream) = live {
                    return Ok(stream.reader());
                }
            }

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

    /// **约束 3：大输出不得破坏顺序 / seq / 跨块 UTF-8，也不得有界缓存历史。**
    ///
    /// PTY 输出不只是聊天：`cat huge.log`、`npm install`、`cargo build` 都可能
    /// 瞬间产生几 MB。传输层必须：固定大小分块、逐块转发、**不保留历史**
    /// （允许背压，但绝不无限吃内存、也绝不静默丢字节）。
    #[test]
    fn large_output_streams_in_order_without_keeping_history() {
        const BLOCK_SIZE: usize = 8 * 1024;
        /// 3 字节字符重复这么多次 ≈ 4 MB，且每个块边界都会切开某个字符
        /// （8192 % 3 != 0），因此必然命中「跨块 UTF-8」场景。
        const REPEAT: usize = 1_400_000;

        let payload: Vec<u8> = "你".repeat(REPEAT).into_bytes();
        let blocks: Vec<Vec<u8>> = payload.chunks(BLOCK_SIZE).map(<[u8]>::to_vec).collect();
        let expected_blocks = blocks.len();
        assert!(expected_blocks > 500, "载荷应足够大：{expected_blocks} 块");
        assert!(
            payload.len() % 3 != 0 || BLOCK_SIZE % 3 != 0,
            "测试前提：块边界应当切开多字节字符"
        );

        let backend = Arc::new(FakePtyBackend::new().with_output(blocks));
        let (manager, chunks) = manager(Arc::clone(&backend));
        manager.spawn("hub-1", spec(), 80, 24).expect("spawn");
        manager.start_reading("hub-1").expect("开始读取");

        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while chunks.lock().expect("chunks").len() < expected_blocks {
            assert!(
                std::time::Instant::now() < deadline,
                "大输出在 20 秒内没有读完，实际 {} / {expected_blocks} 块",
                chunks.lock().expect("chunks").len()
            );
            thread::sleep(Duration::from_millis(5));
        }

        let captured = chunks.lock().expect("chunks").clone();

        // 1) 不得静默丢块；seq 严格有序
        assert_eq!(
            captured.len(),
            expected_blocks,
            "块数必须与写入一致，不得静默丢字节"
        );
        for (index, (seq, _)) in captured.iter().enumerate() {
            assert_eq!(*seq, index as u64, "seq 必须严格有序（顺序不得错乱）");
        }

        // 2) 逐字节一致：顺序、内容、跨块边界都没有被破坏
        let joined: Vec<u8> = captured
            .iter()
            .flat_map(|(_, block)| block.iter().copied())
            .collect();
        assert_eq!(joined.len(), payload.len(), "总字节数必须一致");
        assert_eq!(joined, payload, "重新拼装后必须逐字节完全相同");

        // 3) 跨块的多字节字符仍然可解码（终端模拟器的有状态 decoder 才有这个前提）
        let text = String::from_utf8(joined).expect("跨块多字节字符必须仍然合法");
        assert_eq!(text.chars().count(), REPEAT);
        assert!(text.chars().all(|character| character == '你'));
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
