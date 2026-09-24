//! Windows 进程包含（containment）：**平台原语**，Job Object 承担 Session 的进程树所有权。
//!
//! 为什么需要它（Task 7D 实测）：`.cmd` 的“直接子进程”是 `cmd.exe`（shim 解释器），真正干活的
//! `node.exe` / `codex.exe` / `claude.exe` 是它的**后代**。只终止直接子进程会留下偷活的后代。
//!
//! 保证等级（ADR-0013，不可含糊）：
//!
//! ```text
//! Once containment is established, future descendants inherit the Job.
//! Pre-assignment descendants are reconciled best-effort (fixed-point sweep).
//! Race-free creation-time containment is deferred to a separate ADR/Task.
//! ```
//!
//! **单点所有权**：本类型**不实现 `Clone`**、不 `DuplicateHandle`、不交给 reader/writer 线程。
//! `KILL_ON_JOB_CLOSE` 意味着「最后一个句柄关闭 = 杀掉 job 内所有进程」，因此句柄放在
//! `LiveProcess.containment` 里由 `Drop` 关闭；释放会话时必须**显式**取出并关闭它。

#[cfg(windows)]
mod windows_impl {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob,
        JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
        QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
        PROCESS_TERMINATE,
    };

    use crate::error::{Error, Result};

    /// 补扫的轮次上限：只用于防御异常进程树（持续 fork），**不是**正确性来源。
    pub const SWEEP_ROUNDS_LIMIT: usize = 4;

    /// 一个 Session 的进程树所有权。**不可 Clone**。
    ///
    /// 句柄存成 `usize` 而不是 `HANDLE`：`HANDLE = *mut c_void` 既不是 `Send` 也不是 `Sync`，
    /// 而 `Containment` 要活在 `Arc<LivePty>` 里被多个线程共享。存整数即可自动获得
    /// `Send + Sync`，**不需要** `unsafe impl`（字段私有，只有本模块把它转回 `HANDLE`）。
    pub struct Containment {
        job: usize,
    }

    impl Containment {
        fn handle(&self) -> HANDLE {
            self.job as HANDLE
        }

        pub fn create() -> Result<Self> {
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() || job == INVALID_HANDLE_VALUE {
                return Err(Error::InvalidInput(format!(
                    "创建 Job Object 失败（os error {}）",
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                )));
            }

            // `KILL_ON_JOB_CLOSE` 是「会话结束后不允许后代偷活」的机制本身：最后一个句柄关闭
            // （forget 显式 take/drop，或宿主进程死亡时 OS 关闭所有句柄）就回收 job 内全部进程。
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                let error = std::io::Error::last_os_error();
                unsafe { CloseHandle(job) };
                return Err(Error::InvalidInput(format!(
                    "设置 KILL_ON_JOB_CLOSE 失败（os error {}）",
                    error.raw_os_error().unwrap_or(0)
                )));
            }

            Ok(Self { job: job as usize })
        }

        pub fn assign(&self, pid: u32) -> Result<()> {
            let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
            if process.is_null() {
                return Err(Error::InvalidInput(format!(
                    "打开进程 {pid} 失败（os error {}）",
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                )));
            }
            let ok = unsafe { AssignProcessToJobObject(self.handle(), process) };
            let error = std::io::Error::last_os_error();
            unsafe { CloseHandle(process) };
            if ok == 0 {
                return Err(Error::InvalidInput(format!(
                    "把进程 {pid} 纳入 Job 失败（os error {}）",
                    error.raw_os_error().unwrap_or(0)
                )));
            }
            Ok(())
        }

        pub fn is_member(&self, pid: u32) -> Result<bool> {
            let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if process.is_null() {
                // 进程已退出 / 无权限：当作「不在 job 里」，由上层补扫决定。
                return Ok(false);
            }
            let mut member: i32 = 0;
            let ok = unsafe { IsProcessInJob(process, self.handle(), &mut member) };
            unsafe { CloseHandle(process) };
            if ok == 0 {
                return Ok(false);
            }
            Ok(member != 0)
        }

        pub fn terminate_tree(&self) -> Result<()> {
            let ok = unsafe { TerminateJobObject(self.handle(), 1) };
            if ok == 0 {
                return Err(Error::InvalidInput(format!(
                    "终止进程树失败（os error {}）",
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                )));
            }
            Ok(())
        }

        pub fn active_processes(&self) -> Result<u32> {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            let ok = unsafe {
                QueryInformationJobObject(
                    self.handle(),
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as *mut c_void,
                    std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(Error::InvalidInput("查询 Job 活跃进程数失败".to_string()));
            }
            Ok(info.ActiveProcesses)
        }
    }

    impl Drop for Containment {
        fn drop(&mut self) {
            // 最后一句柄关闭 → KILL_ON_JOB_CLOSE 生效（宿主异常死亡时由 OS 做同样的事）。
            unsafe { CloseHandle(self.handle()) };
        }
    }

    /// 定向终止单个进程（只给「启动失败」的 best-effort 清理用）。
    pub fn terminate_pid(pid: u32) -> Result<()> {
        let process = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if process.is_null() {
            return Err(Error::InvalidInput(format!(
                "打开进程 {pid} 失败（os error {}）",
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
            )));
        }
        let ok = unsafe { TerminateProcess(process, 1) };
        let error = std::io::Error::last_os_error();
        unsafe { CloseHandle(process) };
        if ok == 0 {
            return Err(Error::InvalidInput(format!(
                "终止进程 {pid} 失败（os error {}）",
                error.raw_os_error().unwrap_or(0)
            )));
        }
        Ok(())
    }

    /// 从 root 开始的后代 PID（不包含 root），基于 ToolHelp 快照。
    pub fn descendants_of(root_pid: u32) -> Vec<u32> {
        let mut all: Vec<(u32, u32)> = Vec::new();
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
                return Vec::new();
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snapshot, &mut entry) != 0 {
                loop {
                    all.push((entry.th32ProcessID, entry.th32ParentProcessID));
                    if Process32NextW(snapshot, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snapshot);
        }

        let mut result = Vec::new();
        let mut queue = vec![root_pid];
        while let Some(parent) = queue.pop() {
            for (pid, ppid) in &all {
                if *ppid == parent && *pid != root_pid {
                    result.push(*pid);
                    queue.push(*pid);
                }
            }
        }
        result
    }
}

#[cfg(windows)]
pub use windows_impl::{descendants_of, terminate_pid, Containment, SWEEP_ROUNDS_LIMIT};

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn alive(pid: u32) -> bool {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .expect("tasklist");
        String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
    }

    fn wait_dead(pids: &[u32], timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if pids.iter().all(|pid| !alive(*pid)) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Job 能否覆盖「直接子进程 + 它的后代」：cmd 起一个 ping，两个进程都要在 job 里。
    #[test]
    fn a_job_contains_the_child_and_its_descendants() {
        let containment = Containment::create().expect("create job");
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 300 127.0.0.1"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn cmd");

        // **立即** assign：job 成员资格只被加入之后创建的子进程继承。
        containment.assign(child.id()).expect("assign root");

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut tree = vec![child.id()];
        while Instant::now() < deadline {
            tree = std::iter::once(child.id())
                .chain(descendants_of(child.id()))
                .collect();
            if tree.len() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(tree.len() >= 2, "cmd 必须起出 ping：{tree:?}");

        for pid in tree.iter().skip(1) {
            if !containment.is_member(*pid).expect("is_member") {
                containment.assign(*pid).expect("assign descendant");
            }
        }
        assert_eq!(
            containment.active_processes().expect("active"),
            tree.len() as u32,
            "job 里的活跃进程数必须等于树节点数：{tree:?}"
        );

        containment.terminate_tree().expect("terminate tree");
        assert!(wait_dead(&tree, Duration::from_secs(10)), "{tree:?}");
        // 收尾：子进程已经被 job 杀掉，但仍要 wait() 回收句柄（否则 clippy::zombie_processes）。
        let _ = child.kill();
        let _ = child.wait();
    }

    /// `KILL_ON_JOB_CLOSE`：最后一句柄关闭（Drop）时，job 内进程必须被 OS 回收。
    #[test]
    fn closing_the_last_handle_kills_the_contained_tree() {
        let child_pid;
        let mut child;
        {
            let containment = Containment::create().expect("create job");
            child = std::process::Command::new("ping")
                .args(["-n", "300", "127.0.0.1"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("spawn ping");
            child_pid = child.id();
            containment.assign(child_pid).expect("assign");
            assert!(alive(child_pid));
            // containment 在这里 drop → 最后一句柄关闭 → KILL_ON_JOB_CLOSE
        }
        assert!(
            wait_dead(&[child_pid], Duration::from_secs(10)),
            "job 句柄关闭后 {child_pid} 必须被 OS 回收"
        );
        // 回收句柄（进程已被 job 杀掉）。
        let _ = child.wait();
    }

    /// 失败路径：assign 一个打不开的进程必须报**具体 Win32 error**，不能返回 Ok、
    /// 也不能只报一句「失败了」（约束 6 后半句）。
    #[test]
    fn assign_reports_the_win32_error_for_an_unopenable_process() {
        let containment = Containment::create().expect("create job");

        // 0xFFFF_FFFF 不可能是真实进程（OpenProcess 必定失败）。
        let error = containment
            .assign(0xFFFF_FFFF)
            .expect_err("打不开的进程必须报错");

        let message = error.to_string();
        assert!(
            message.contains("os error"),
            "错误信息必须带具体 Win32 error：{message}"
        );
    }
}
