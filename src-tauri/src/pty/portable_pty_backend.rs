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

#[cfg(windows)]
use crate::pty::containment::{descendants_of, terminate_pid, Containment, SWEEP_ROUNDS_LIMIT};

/// 一条会话**拥有的进程**：控制句柄 + （Windows）进程树所有权。
///
/// 拆成独立结构是刻意的：`LivePty` 里 `master` 是**传输**，这里是**进程**。两者生命周期
/// 必须能分别处理（spec §1.3）：关传输让 park 的写返回，release containment 让后代不偷活。
struct LiveProcess {
    /// kill / try_wait / is_running。
    child: Mutex<Box<dyn Child + Send + Sync>>,
    /// Windows：Job Object。**单点所有权**（不 Clone / 不 Duplicate / 不给别的线程），
    /// 和 master 一样是 `Option`：`forget` 必须能**显式取出并关闭唯一句柄** ——
    /// parked writer 持有 `Arc<LivePty>` 时，等 Arc drop 就等于永远不关 job。
    #[cfg(windows)]
    containment: Mutex<Option<Containment>>,
}

/// 一条会话的传输 + 进程，各自一把锁。
///
/// 拆成多把锁是刻意的：阻塞写只能占住 `writer`，`kill` / `try_wait` 走 `process.child`，
/// `resize` 走 `master` —— 互不等待（INV-3）。
struct LivePty {
    /// 只有写入会 park 在这把锁上。
    writer: Mutex<Box<dyn Write + Send>>,
    /// resize / try_clone_reader。
    ///
    /// `Option` 是为了「主动关闭」：parked writer 可能持有 `Arc<LivePty>`，
    /// 所以关闭必须由 [`LivePty::close_transport`] 主动 take/drop，不能等 Arc drop。
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    process: LiveProcess,
}

impl LivePty {
    /// 关闭 PTY 传输资源。**不触碰 writer 锁**（可能被 parked 线程持有，等它 = 把 8A 的
    /// 不变量从后门放回来）。
    fn close_transport(&self) {
        if let Ok(mut master) = self.master.lock() {
            // take 之后 drop → ClosePseudoConsole（conhost 退出 → 管道两端都断：
            // parked 的写拿到 Err，reader 拿到 EOF）。
            let _ = master.take();
        }
    }

    /// 显式取出并关闭唯一的 Job 句柄（`KILL_ON_JOB_CLOSE` 因此立即生效）。
    ///
    /// `take()` 保证「即使 parked writer 还持有 `Arc<LivePty>`，句柄也已经关掉」；
    /// 幂等：已经取过就是 `None`。
    #[cfg(windows)]
    fn release_containment(&self) -> Result<()> {
        let mut guard = lock(&self.process.containment)?;
        // take() 之后 drop 掉 Containment → CloseHandle → 最后一句柄关闭 → job 内进程被回收。
        drop(guard.take());
        Ok(())
    }

    /// 取 master 的引用（已被 `forget` 关掉就是明确错误）。
    fn master(&self) -> Result<std::sync::MutexGuard<'_, Option<Box<dyn MasterPty + Send>>>> {
        let guard = lock(&self.master)?;
        if guard.is_none() {
            return Err(Error::InvalidInput("PTY 传输已关闭".to_string()));
        }
        Ok(guard)
    }
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

/// 启动失败时的 best-effort 定向清理：**root + 可枚举的后代**（不能只杀 root，
/// 否则就是一个半受控、还在跑的进程树）。
///
/// 只用于「containment 没能建立」这一条失败路径；正常路径的整树终止走
/// [`Containment::terminate_tree`]。
///
/// 顺序是**先 root 后后代**：root 死了才不会在清理过程中继续 fork 出新的后代；
/// 后代的 `ParentProcessId` 在 Windows 上是陈旧值（不会重挂到别的进程），所以
/// root 死后仍然枚举得到。
#[cfg(windows)]
fn best_effort_cleanup(root_pid: Option<u32>, child: &mut Box<dyn Child + Send + Sync>) {
    let root_killed = child.kill().is_ok();

    if let Some(root_pid) = root_pid {
        for _ in 0..SWEEP_ROUNDS_LIMIT {
            let pending = descendants_of(root_pid);
            if pending.is_empty() {
                break;
            }
            for pid in pending {
                if let Err(error) = terminate_pid(pid) {
                    eprintln!("[pty] 启动失败清理 {pid} 失败（best-effort）：{error}");
                }
            }
        }
    }

    if !root_killed {
        eprintln!("[pty] 启动失败清理无法结束 root 进程（best-effort）");
    }
}

/// 非 Windows：没有 containment 可用，只能结束直接子进程。
#[cfg(not(windows))]
fn best_effort_cleanup(_root_pid: Option<u32>, child: &mut Box<dyn Child + Send + Sync>) {
    let _ = child.kill();
}

/// 定点补扫：直到本轮没有发现新的、尚未纳入 job 的后代。
/// **不靠 sleep**；轮次上限只防御异常进程树（持续 fork），**不是**正确性来源（ADR-0013）。
#[cfg(windows)]
fn reconcile_descendants(containment: &Containment, root_pid: u32) {
    for _ in 0..SWEEP_ROUNDS_LIMIT {
        let pending: Vec<u32> = descendants_of(root_pid)
            .into_iter()
            .filter(|pid| !containment.is_member(*pid).unwrap_or(false))
            .collect();
        if pending.is_empty() {
            return; // 固定点
        }
        for pid in pending {
            // 补扫失败只记日志：不能因为一个后代无权限就把整个会话判成启动失败。
            if let Err(error) = containment.assign(pid) {
                eprintln!("[pty] 补扫 {pid} 失败（best-effort）：{error}");
            }
        }
    }
}

impl PtyBackend for PortablePtyBackend {
    fn spawn(&self, request: PtySpawnRequest) -> Result<PtyProcessHandle> {
        // 1) 先建 containment：无法建立 containment 的会话不允许进入 running
        //    （约束 1：失败 ⇒ launch_failed + best-effort 清理，绝不静默降级）。
        #[cfg(windows)]
        let containment = Containment::create()?;

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

        // 2) openpty + CreateProcess（现状不变）
        let mut child = pair
            .slave
            .spawn_command(builder)
            .map_err(|error| Error::InvalidInput(format!("启动进程失败：{error}")))?;
        // 父进程必须丢掉 slave，否则子进程永远拿不到 EOF。
        drop(pair.slave);

        // 3) root PID 必须拿得到：拿不到就等于「无法建立 containment」，
        //    不能静默返回成功（约束 1）。
        let Some(pid) = child.process_id() else {
            best_effort_cleanup(None, &mut child);
            return Err(Error::InvalidInput(
                "无法建立进程包含：root PID 不可用（containment 无从建立）".to_string(),
            ));
        };

        // 4) **立即** assign root：job 成员资格只被「加入之后创建」的子进程继承。
        #[cfg(windows)]
        {
            if let Err(error) = containment.assign(pid) {
                // best-effort 定向清理：root 与**可枚举的后代**都要尽力收掉，不能只杀 root。
                best_effort_cleanup(Some(pid), &mut child);
                return Err(Error::InvalidInput(format!(
                    "无法建立进程包含（AssignProcessToJobObject 失败）：{error}"
                )));
            }
            // 5) 定点补扫：把 assign 之前已经创建出来的后代补进去（best-effort，见 ADR-0013）。
            reconcile_descendants(&containment, pid);
        }

        let writer = pair
            .master
            .take_writer()
            .map_err(|error| Error::InvalidInput(format!("获取 PTY 写端失败：{error}")))?;

        // 6) 组装 LivePty（master / containment 都是 Option，forget 才能主动关闭）。
        lock(&self.sessions)?.insert(
            request.session_id.clone(),
            Arc::new(LivePty {
                writer: Mutex::new(writer),
                master: Mutex::new(Some(pair.master)),
                process: LiveProcess {
                    child: Mutex::new(child),
                    #[cfg(windows)]
                    containment: Mutex::new(Some(containment)),
                },
            }),
        );

        Ok(PtyProcessHandle {
            session_id: request.session_id,
            pid: Some(pid),
        })
    }

    fn take_reader(&self, session_id: &str) -> Result<Box<dyn Read + Send>> {
        let live = self.live(session_id)?;
        let master = live.master()?;

        master
            .as_ref()
            .expect("master() 已保证 Some")
            .try_clone_reader()
            .map_err(|error| Error::InvalidInput(format!("获取 PTY 读端失败：{error}")))
    }

    fn write(&self, session_id: &str, bytes: &[u8]) -> Result<()> {
        let live = self.live(session_id)?;
        let mut writer = lock(&live.writer)?;

        // 这一行可能在子进程不读 stdin 时阻塞很久 —— 但只 park 本会话的 writer 锁，
        // 不再冻结任何别的会话或本会话的 kill/resize/reap（INV-1/INV-3）。
        //
        // 解 park 的手段是「主动关闭传输」（`forget` → `close_transport`），
        // **不是** `terminate_tree`：进程死了不等于管道写端会返回（spike s5 三档实验）。
        writer
            .write_all(bytes)
            .map_err(|error| Error::InvalidInput(format!("写入 PTY 失败：{error}")))?;
        writer
            .flush()
            .map_err(|error| Error::InvalidInput(format!("刷新 PTY 失败：{error}")))
    }

    fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<()> {
        let live = self.live(session_id)?;
        let master = live.master()?;

        master
            .as_ref()
            .expect("master() 已保证 Some")
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| Error::InvalidInput(format!("调整 PTY 尺寸失败：{error}")))
    }

    /// 终止该会话**拥有的整棵进程树**（Session 拥有的是进程树，不是 PID）。
    ///
    /// **这是控制动作**：不改会话状态机，也不解除 parked 写（那是 `close_transport`）。
    fn terminate_tree(&self, session_id: &str) -> Result<()> {
        let live = self.live(session_id)?;

        #[cfg(windows)]
        {
            let guard = lock(&live.process.containment)?;
            let containment = guard.as_ref().ok_or_else(|| {
                Error::InvalidInput(format!(
                    "会话 {session_id} 的 containment 已释放，无法终止进程树"
                ))
            })?;
            containment.terminate_tree()
        }

        #[cfg(not(windows))]
        {
            // 非 Windows：保持现状语义（direct child），containment 留给各平台实现。
            let mut child = lock(&live.process.child)?;
            child
                .kill()
                .map_err(|error| Error::InvalidInput(format!("结束进程失败：{error}")))
        }
    }

    fn try_wait(&self, session_id: &str) -> Result<Option<i32>> {
        // 未知会话保持原有语义（`Ok(None)`），不改成报错：reaper 依赖这条。
        let Ok(live) = self.live(session_id) else {
            return Ok(None);
        };
        let mut child = lock(&live.process.child)?;

        let status = child
            .try_wait()
            .map_err(|error| Error::InvalidInput(format!("查询进程状态失败：{error}")))?;

        Ok(status.map(|status| status.exit_code() as i32))
    }

    /// 显式丢弃会话（终态写入之后由上层调用）。
    ///
    /// **两步都不能省**（spec §1.3：containment ≠ transport closure）：
    /// 1. `close_transport()`：**主动**关闭 PTY master —— parked writer 自己持有
    ///    `Arc<LivePty>`，等 Arc drop 就等于永远不关 master，写线程永远不返回（spike s5）。
    /// 2. `release_containment()`：**显式取走并关闭唯一 Job 句柄** —— 同样是 parked writer
    ///    持有 Arc 的缘故，等 Arc drop 就等于永远不关 job，后代偷活（约束 4+5）。
    fn forget(&self, session_id: &str) -> Result<()> {
        let Some(live) = lock(&self.sessions)?.remove(session_id) else {
            return Ok(());
        };
        live.close_transport();
        #[cfg(windows)]
        live.release_containment()?;
        Ok(())
    }

    fn is_running(&self, session_id: &str) -> Result<bool> {
        // 未知会话保持原有语义（`Ok(false)`）。
        let Ok(live) = self.live(session_id) else {
            return Ok(false);
        };
        let mut child = lock(&live.process.child)?;

        let status = child
            .try_wait()
            .map_err(|error| Error::InvalidInput(format!("查询进程状态失败：{error}")))?;

        Ok(status.is_none())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::harness::launch::LaunchSpec;
    use crate::pty::containment::test_support::{process_alive, wait_until_dead};

    /// conhost 因为 `PSUEDOCONSOLE_INHERIT_CURSOR` 会在启动时发 `ESC[6n`（DSR）并等应答。
    /// 生产里这个应答由前端终端模拟器（xterm.js）给出；headless 测试必须自己当终端。
    ///
    /// **不应答的后果不只是「子进程黑屏」**：pseudoconsole 卡在初始化里，之后连
    /// `ClosePseudoConsole` 都收不干净 —— master 明明被 drop 了，reader 也拿不到 EOF，
    /// park 的写永远不会返回。Task 3 的 RED 一开始就把它误判成「产品没关传输」。
    const DSR_REQUEST: &[u8] = b"\x1b[6n";
    const DSR_REPLY: &[u8] = b"\x1b[1;1R";

    /// headless 测试的「终端模拟器」：持续排空 console 输出，并在看到 DSR 时**只应答一次**
    /// （7D 的教训：无限重放应答会把子进程的输入缓冲淹掉）。
    struct TestTerminal {
        answered: Arc<std::sync::atomic::AtomicBool>,
        done: Arc<std::sync::atomic::AtomicBool>,
    }

    impl TestTerminal {
        /// 阻塞到握手完成（有界 10s）：park 之后 writer 锁被占，应答就写不进去了。
        fn wait_for_handshake(&self) {
            use std::sync::atomic::Ordering;
            use std::time::{Duration, Instant};

            let deadline = Instant::now() + Duration::from_secs(10);
            while !self.answered.load(Ordering::SeqCst) && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(
                self.answered.load(Ordering::SeqCst),
                "前提：conhost 的 DSR 握手必须被应答，否则测试测不到传输关闭"
            );
        }

        fn is_done(&self) -> bool {
            self.done.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    fn attach_test_terminal(backend: &Arc<PortablePtyBackend>, session_id: &str) -> TestTerminal {
        use std::sync::atomic::{AtomicBool, Ordering};

        let answered = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let reader = backend.take_reader(session_id).expect("reader");
        {
            let answered_for_reader = Arc::clone(&answered);
            let done_for_reader = Arc::clone(&done);
            let backend_for_reader = Arc::clone(backend);
            let session = session_id.to_string();
            std::thread::spawn(move || {
                let mut reader = reader;
                let mut sink = [0u8; 8192];
                loop {
                    match reader.read(&mut sink) {
                        Ok(0) => break,
                        Ok(read) => {
                            let wants_dsr = sink[..read]
                                .windows(DSR_REQUEST.len())
                                .any(|window| window == DSR_REQUEST);
                            if wants_dsr && !answered_for_reader.swap(true, Ordering::SeqCst) {
                                let _ = backend_for_reader.write(&session, DSR_REPLY);
                            }
                        }
                        Err(_) => break,
                    }
                }
                done_for_reader.store(true, Ordering::SeqCst);
            });
        }
        TestTerminal { answered, done }
    }

    fn spec(program: &str, args: &[&str]) -> LaunchSpec {
        LaunchSpec {
            program: std::path::PathBuf::from(program),
            args: args.iter().map(|arg| (*arg).to_string()).collect(),
            cwd: None,
            env: Vec::new(),
            runtime_target_id: "local".to_string(),
        }
    }

    fn request(session_id: &str, spec: LaunchSpec) -> PtySpawnRequest {
        PtySpawnRequest {
            session_id: session_id.to_string(),
            spec,
            cols: 80,
            rows: 24,
        }
    }

    /// 在后台 park 一次 64 MiB 的写（返回「写已返回」的标志）。
    ///
    /// 64 MiB：4 MiB 会被 ConPTY 缓冲吞掉（约 4.7s 自然返回），测不到长期 park。
    fn park_a_write(
        backend: &Arc<PortablePtyBackend>,
        session_id: &str,
    ) -> Arc<std::sync::atomic::AtomicBool> {
        use std::sync::atomic::{AtomicBool, Ordering};

        let returned = Arc::new(AtomicBool::new(false));
        let returned_flag = Arc::clone(&returned);
        let backend_for_writer = Arc::clone(backend);
        let session = session_id.to_string();
        std::thread::spawn(move || {
            let payload = vec![b'x'; 64 * 1024 * 1024];
            let _ = backend_for_writer.write(&session, &payload);
            returned_flag.store(true, Ordering::SeqCst);
        });
        returned
    }

    fn assert_parked(returned: &Arc<std::sync::atomic::AtomicBool>, why: &str) {
        assert!(
            !returned.load(std::sync::atomic::Ordering::SeqCst),
            "前提：写必须还 park 着（{why}）"
        );
    }

    /// `forget` 必须**主动**关闭 PTY master：否则 park 在 `write_all` 的线程（它自己持有
    /// `Arc<LivePty>`）永远不会返回 —— 这是 8B spike s5 三档实验唯一让写返回的那一档。
    #[test]
    fn forget_actively_closes_the_transport_so_a_parked_write_returns() {
        use std::time::Duration;

        let backend = Arc::new(PortablePtyBackend::new());
        let handle = backend
            .spawn(request(
                "hub-parked",
                spec("ping", &["-n", "300", "127.0.0.1"]),
            ))
            .expect("spawn");
        let terminal = attach_test_terminal(&backend, "hub-parked");
        terminal.wait_for_handshake();

        let returned = park_a_write(&backend, "hub-parked");
        std::thread::sleep(Duration::from_secs(2));
        assert_parked(&returned, "64 MiB 才稳");

        // 第一步：用户 kill → 树杀。**这一步解不开写**（spike s5 第二档）：
        // 进程死了不等于管道写端会返回 —— containment ≠ transport closure。
        backend.terminate_tree("hub-parked").expect("树杀");
        std::thread::sleep(Duration::from_secs(1));
        assert_parked(&returned, "树杀不得被当成解开阻塞写的手段");

        // 第二步：reaper 写完终态 → forget。**这一步才解开写**（spike s5 第三档）。
        backend.forget("hub-parked").expect("forget");

        assert!(
            wait_for(&returned, Duration::from_secs(10)),
            "forget 之后 parked write 必须返回（有界 10s 观察窗口，不 join）"
        );
        assert!(
            terminal.is_done(),
            "master 被关闭之后 reader 必须拿到 EOF（传输确实关了）"
        );

        cleanup(&handle);
    }

    /// T5：`forget` 必须**显式取出并关闭唯一 Job 句柄**，不能依赖 `Arc<LivePty>` drop ——
    /// parked writer 自己就持有那个 Arc（spec §4.5 / 约束 4+5）。
    ///
    /// 断言的**判别性来源**是「从句柄被 parked writer 持有的那个 `Arc` 里看，
    /// `containment` 已经被取走（None）」（已用「取走但 `mem::forget` 泄漏句柄」的变异验证过：
    /// 变异后本条与本文件另外两条的「后代被回收」断言**照样通过**）。
    /// 原因是 `ClosePseudoConsole` 本身就会带走挂在同一 console 上的后代，
    /// 所以「后代消失」这条**不足以**区分实现；Job 路径的判别性证据在
    /// `containment::tests::closing_the_last_handle_kills_the_contained_tree`（纯 Job、
    /// 没有 console 参与）与 spike s6（宿主被 `taskkill /F`）。
    /// 「forget 之后不得留下偷活后代」仍然作为产品级结论保留在这里。
    #[test]
    fn forget_releases_the_job_even_while_a_writer_holds_the_arc() {
        use std::time::{Duration, Instant};

        let backend = Arc::new(PortablePtyBackend::new());
        let handle = backend
            .spawn(request(
                "hub-park-tree",
                spec("cmd", &["/c", "ping -n 300 127.0.0.1"]),
            ))
            .expect("spawn");
        let terminal = attach_test_terminal(&backend, "hub-park-tree");
        terminal.wait_for_handshake();
        let root = handle.pid.expect("pid");

        // 等后代出现（cmd → ping）
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut descendants = Vec::new();
        while Instant::now() < deadline {
            descendants = descendants_of(root);
            if !descendants.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(!descendants.is_empty(), "需要 cmd → ping 两层结构");

        let returned = park_a_write(&backend, "hub-park-tree");
        std::thread::sleep(Duration::from_secs(2));
        assert_parked(&returned, "parked writer 必须持有 Arc<LivePty>");

        // forget 之前先拿一份 Arc：forget 之后 map 里没有了，但 parked writer 还持有它。
        let live = backend.live("hub-park-tree").expect("live");
        backend.forget("hub-park-tree").expect("forget");

        assert!(
            wait_for(&returned, Duration::from_secs(10)),
            "forget 之后 parked write 必须返回"
        );
        assert!(
            live.process
                .containment
                .lock()
                .expect("containment")
                .is_none(),
            "forget 必须显式取走并关闭唯一 Job 句柄（不能等 Arc<LivePty> drop）"
        );

        let all: Vec<u32> = std::iter::once(root).chain(descendants.clone()).collect();
        assert!(
            wait_until_dead(&all, Duration::from_secs(10)),
            "forget 之后不得留下偷活的后代（存活：{:?}）",
            all.iter()
                .copied()
                .filter(|pid| process_alive(*pid))
                .collect::<Vec<u32>>()
        );

        cleanup(&handle);
    }

    /// reaper 路径：root **自然退出**、后代仍存活 → `forget` 必须把它们收掉。
    ///
    /// `start /b` 让 ping 成为 cmd 的后代但 cmd 立刻退出 —— 正是「root 走了、后代偷活」的形状。
    #[test]
    fn release_after_a_natural_root_exit_still_reaps_living_descendants() {
        use std::time::{Duration, Instant};

        let backend = Arc::new(PortablePtyBackend::new());
        let handle = backend
            .spawn(request(
                "hub-natural",
                spec("cmd", &["/c", "start /b ping -n 300 127.0.0.1"]),
            ))
            .expect("spawn");
        let terminal = attach_test_terminal(&backend, "hub-natural");
        terminal.wait_for_handshake();
        let root = handle.pid.expect("pid");

        // 前提 1：后代存在；前提 2：root 已自然退出（reaper 观察到的那个事实）。
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut descendants = Vec::new();
        let mut root_exited = false;
        while Instant::now() < deadline {
            descendants = descendants_of(root);
            root_exited = backend.try_wait("hub-natural").expect("try_wait").is_some();
            if !descendants.is_empty() && root_exited {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(root_exited, "前提：root（cmd）必须已经自然退出");
        assert!(!descendants.is_empty(), "前提：root 退出时后代仍然存在");
        assert!(
            descendants.iter().any(|pid| process_alive(*pid)),
            "前提：至少一个后代在 root 退出后仍然存活：{descendants:?}"
        );

        let live = backend.live("hub-natural").expect("live");
        backend.forget("hub-natural").expect("forget");

        assert!(
            live.process
                .containment
                .lock()
                .expect("containment")
                .is_none(),
            "forget 必须显式取走并关闭唯一 Job 句柄"
        );
        assert!(
            wait_until_dead(&descendants, Duration::from_secs(10)),
            "root 自然退出后，forget 仍必须回收存活的后代（存活：{:?}）",
            descendants
                .iter()
                .copied()
                .filter(|pid| process_alive(*pid))
                .collect::<Vec<u32>>()
        );

        cleanup(&handle);
    }

    /// 有界等待一个 `AtomicBool` 变真（测试里到处要用，避免 sleep 猜）。
    fn wait_for(flag: &Arc<std::sync::atomic::AtomicBool>, timeout: std::time::Duration) -> bool {
        use std::time::Instant;

        let deadline = Instant::now() + timeout;
        while !flag.load(std::sync::atomic::Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        flag.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// 收尾：残余进程一律清掉，别给后面的测试留垃圾。
    fn cleanup(handle: &PtyProcessHandle) {
        let _ = std::process::Command::new("taskkill")
            .args([
                "/F",
                "/T",
                "/PID",
                &handle.pid.unwrap_or_default().to_string(),
            ])
            .output();
    }
}
