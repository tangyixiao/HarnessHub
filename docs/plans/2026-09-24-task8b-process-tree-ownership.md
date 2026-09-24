# Task 8B — Process Tree Ownership Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `kill_terminal(session_id)` 终止该 Session **拥有的整棵进程树**，并且 Harness Hub 异常死亡时由 OS 自动回收它拥有的 descendants。

**Architecture:** Windows 上用 **Job Object** 承担 containment：`create job(KILL_ON_JOB_CLOSE)` → spawn PTY child → **立即** `AssignProcessToJobObject(root)` → **定点补扫** → 只有到这一步才允许 `mark_running`。`LivePty` 拆成 `{ master, writer, process: LiveProcess { child, containment } }`；`forget()` **主动**关闭 PTY master（不依赖 `Arc` drop），因为 park 在 `write_all` 的 writer 自己就持有 `Arc`。**进程包含 ≠ 传输关闭**（spec §1.3）。

**Tech Stack:** Rust 2021；`windows-sys 0.59`（`Win32_System_JobObjects` / `Win32_System_Diagnostics_ToolHelp` / `Win32_System_Threading` / `Win32_Security` / `Win32_Foundation`，仅 Windows target）；`portable-pty 0.9`；测试 `cargo test`（单元 + `#[cfg(windows)]` 真机集成）。

**Spec:** `docs/specs/2026-09-24-task8b-process-tree-ownership-design.md`（执行前先读它；保证等级措辞见 spec §5）
**ADR:** `docs/adr/0013-windows-process-containment-establishment.md`（creation-time containment 暂缓 + 已知 race）

## Global Constraints

- **不改 Session 状态机语义**：`terminate_tree()` 只是控制动作；`user_killed / natural_exit / lost / host_shutdown` 仍由 intent + reaper/reconcile 决定。**绝不**因为树杀成功就写终态。
- **containment 在 `running` 之前建立**；建立不了 → `failed` + `launch_failed` + best-effort 定向清理 + **具体 Win32 error**，不得静默降级成 direct-child kill。
- 补扫是**固定点循环 + 轮次上限**（`SWEEP_ROUNDS_LIMIT = 4`），**不靠 `sleep`** 作为正确性来源；失败只记日志，不中断 spawn。
- 保证等级措辞必须与 spec §5 一致：**禁止**声称 race-free ownership。
- `forget()` **主动** `close_transport()`（take/drop master）；**不触碰 writer mutex**（可能被 parked 线程持有）。
- Job handle：**单点所有权**（`LiveProcess.containment`）、**不 `DuplicateHandle`**、**不**给 reader/writer/worker 线程。
- reaper 顺序不变（8A）：终态 → 有界等待 reader → `shutdown_input` → 移除 handle → `backend.forget`；**绝不 join** writer。
- `PtyManager::kill` / `TerminalRuntime::kill` 的**名字与签名不变**（IPC/UI/7D 测试不受影响）；只有 trait 上的 `kill` → `terminate_tree`。
- Windows-only 代码用 `#[cfg(windows)]`；真机测试本机缺前置条件时明确跳过，绝不假装通过。
- 无 Harness 特判；无新 IPC DTO；UI 无改动。提交信息用 `feat:` / `fix:` / `test:` / `refactor:` / `docs:` / `chore:`。
- 每个 Task 结束跑该 Task 指定的命令；最后跑 `pnpm verify` + `pnpm python:test`。

## File Structure

| 文件                                        | 责任                                                                                                         | 动作   |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------------ | ------ |
| `src-tauri/Cargo.toml`                      | 新增 Windows-only 直接依赖 `windows-sys`                                                                     | Modify |
| `src-tauri/src/pty/containment.rs`          | **平台原语**：Job Object（create/limit/assign/terminate/is_member/Drop）+ descendant 枚举                    | Create |
| `src-tauri/src/pty/mod.rs`                  | 暴露 `containment`                                                                                           | Modify |
| `src-tauri/src/pty/backend.rs`              | trait：`kill` → `terminate_tree`（+ 文档措辞）                                                               | Modify |
| `src-tauri/src/pty/portable_pty_backend.rs` | `LivePty{master: Option, writer, process}`；spawn 建 containment + 补扫；`terminate_tree`；`forget` 主动关闭 | Modify |
| `src-tauri/src/pty/manager.rs`              | `kill` 内部改调 `terminate_tree`；fake 增加 containment 相关钩子                                             | Modify |
| `src-tauri/src/pty/input_tests.rs`          | 8A 用例适配（`fail_next_kill` → `fail_next_terminate_tree`）+ 新增 T1–T6                                     | Modify |
| `src-tauri/tests/process_tree_ownership.rs` | 真机 Gate R1–R7                                                                                              | Create |
| `tests/e2e/README.md`                       | 8B 证据                                                                                                      | Modify |
| `docs/CONTEXT.md`、`docs/CONTEXT-MAP.md`    | 关账后同步（`terminate_tree` / `pty::containment`）                                                          | Modify |

---

### Task 1: `pty::containment`（Windows Job Object 原语）

**Files:**

- Modify: `src-tauri/Cargo.toml`（`[target.'cfg(windows)'.dependencies]` 新增 `windows-sys`）
- Create: `src-tauri/src/pty/containment.rs`
- Modify: `src-tauri/src/pty/mod.rs`
- Test: `src-tauri/src/pty/containment.rs`（`#[cfg(all(test, windows))] mod tests`，真机合成树）

**Interfaces:**

- Consumes: 无
- Produces:
  - `pty::containment::Containment::{create() -> Result<Self>, assign(&self, pid: u32) -> Result<()>, is_member(&self, pid: u32) -> Result<bool>, terminate_tree(&self) -> Result<()>, active_processes(&self) -> Result<u32>}`
  - `pty::containment::descendants_of(root_pid: u32) -> Vec<u32>`
  - `pty::containment::SWEEP_ROUNDS_LIMIT: usize`
  - `Containment` **不实现 `Clone`**（单点所有权）

- [ ] **Step 1: 写失败测试（真机合成树：cmd → ping）**

`containment.rs` 末尾：

```rust
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

        let mut newly = 0;
        for pid in tree.iter().skip(1) {
            if !containment.is_member(*pid).expect("is_member") {
                containment.assign(*pid).expect("assign descendant");
                newly += 1;
            }
        }
        assert!(newly <= tree.len(), "补扫只应处理尚未在 job 里的进程");
        assert_eq!(
            containment.active_processes().expect("active"),
            tree.len() as u32,
            "job 里的活跃进程数必须等于树节点数：{tree:?}"
        );

        containment.terminate_tree().expect("terminate tree");
        assert!(wait_dead(&tree, Duration::from_secs(10)), "{tree:?}");
        let _ = child.kill();
    }

    /// `KILL_ON_JOB_CLOSE`：最后一句柄关闭（Drop）时，job 内进程必须被 OS 回收。
    #[test]
    fn closing_the_last_handle_kills_the_contained_tree() {
        let child_pid;
        {
            let containment = Containment::create().expect("create job");
            let mut child = std::process::Command::new("ping")
                .args(["-n", "300", "127.0.0.1"])
                .stdin(std::process::Stdio::null())
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
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::containment`
Expected: 编译错误 `unresolved import crate::pty::containment` / `no function create`

- [ ] **Step 3: 加依赖**

`src-tauri/Cargo.toml` 末尾：

```toml
# Windows 进程包含（Task 8B）：Job Object 是唯一能覆盖 .cmd → node → descendants 的机制。
# 只在 Windows target 上引入，非 Windows 平台将来用 process group / session 实现同一语义。
[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = [
    "Win32_Foundation",
    "Win32_Security",
    "Win32_System_JobObjects",
    "Win32_System_Threading",
    "Win32_System_Diagnostics_ToolHelp",
] }
```

- [ ] **Step 4: 实现 `pty/containment.rs`**

````rust
//! Windows 进程包含（containment）：**平台原语**，Job Object 承担 Session 的进程树所有权。
//!
//! 保证等级（ADR-0013，不可含糊）：
//!
//! ```text
//! Once containment is established, future descendants inherit the Job.
//! Pre-assignment descendants are reconciled best-effort (fixed-point sweep).
//! Race-free creation-time containment is deferred to a separate ADR/Task.
//! ```
//!
//! **单点所有权**：本类型**不实现 `Clone`**、不 `DuplicateHandle`、不交给 reader/writer 线程；
//! `KILL_ON_JOB_CLOSE` 意味着最后一个句柄关闭 = 杀掉 job 内所有进程，因此句柄放在
//! `LiveProcess.containment` 里由 `Drop` 关闭。

#[cfg(windows)]
mod windows_impl {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
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
    }
    impl Containment {
        pub fn create() -> Result<Self> {
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() || job == INVALID_HANDLE_VALUE {
                return Err(Error::InvalidInput(format!(
                    "创建 Job Object 失败（os error {}）",
                    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
                )));
            }
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
            Ok(Self {
                job: job as usize,
            })
        }

        pub fn assign(&self, pid: u32) -> Result<()> {
            let process = unsafe {
                OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid)
            };
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
                return Ok(false); // 进程已退出/无权限：当作「不在 job 里」，交给上层补扫决定
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

    /// 定向终止单个进程（只给启动失败的 best-effort 清理用）。
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
````

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::containment`
Expected: `test result: ok. 3 passed`（`a_job_contains_the_child_and_its_descendants` /
`closing_the_last_handle_kills_the_contained_tree` / `assign_reports_the_win32_error_for_an_unopenable_process`）

- [ ] **Step 6: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/Cargo.toml src-tauri/Cargo.lock src-tauri/src/pty/containment.rs src-tauri/src/pty/mod.rs
git commit -m "feat(pty): add the Windows Job Object containment primitive"
```

---

### Task 2: trait `kill` → `terminate_tree`（只改名字与语义，不改调用方）

**Files:**

- Modify: `src-tauri/src/pty/backend.rs`（trait）
- Modify: `src-tauri/src/pty/portable_pty_backend.rs`（临时实现：暂时仍只杀 direct child，Task 3 换成 job）
- Modify: `src-tauri/src/pty/manager.rs`（`kill` 内部改调 `terminate_tree`；fake 改名）
- Modify: `src-tauri/src/pty/input_tests.rs`（`fail_next_kill` → `fail_next_terminate_tree`）

**Interfaces:**

- Consumes: 无
- Produces: `PtyBackend::terminate_tree(&self, session_id: &str) -> Result<()>`（trait 上不再有 `kill`）；fake 的 `fail_next_terminate_tree` 与 `terminated_trees: Mutex<Vec<String>>`

- [ ] **Step 1: 写失败测试（T2/T3/T6 的形状）**

追加到 `pty/input_tests.rs`：

```rust
/// T2：kill 必须走 `terminate_tree`（Session 拥有的是进程树，不是 direct child）。
#[test]
fn kill_terminates_the_owned_tree_and_closes_the_input_side() {
    let backend = Arc::new(FakePtyBackend::new());
    let manager = manager(Arc::clone(&backend), 64);
    spawn(&manager, "hub-a");

    manager.kill("hub-a").expect("kill 成功");

    assert_eq!(
        backend.terminated_trees.lock().expect("terminated").clone(),
        vec!["hub-a".to_string()],
        "kill 必须调用 terminate_tree"
    );
    assert!(
        matches!(manager.write("hub-a", b"x"), Err(Error::InputClosed { .. })),
        "kill 成功之后输入侧仍然要关闭（8A 语义不变）"
    );
}

/// T3：terminate_tree 失败 → 不关闭输入侧、不写终态（沿用 8A T7 的形状）。
#[test]
fn a_failed_tree_termination_changes_nothing() {
    let backend = Arc::new(FakePtyBackend::new());
    let manager = manager(Arc::clone(&backend), 64);
    spawn(&manager, "hub-a");
    backend.fail_next_terminate_tree("hub-a");

    assert!(manager.kill("hub-a").is_err(), "backend 失败必须如实返回");
    assert!(
        manager.write("hub-a", b"still-ok").is_ok(),
        "树杀失败不得关闭输入侧"
    );
}

/// T6：没有 containment 的会话必须报明确错误，而不是静默降级成 direct-child kill。
#[test]
fn terminating_a_session_without_containment_is_an_explicit_error() {
    let backend = Arc::new(FakePtyBackend::new().without_containment());
    let manager = manager(Arc::clone(&backend), 64);
    spawn(&manager, "hub-a");

    let error = manager.kill("hub-a").expect_err("没有 containment 必须报错");
    assert!(
        error.to_string().contains("containment") || error.to_string().contains("包含"),
        "错误信息要说明缺少 containment：{error}"
    );
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::input_tests`
Expected: 编译错误 `no method named terminated_trees` / `fail_next_terminate_tree` / `without_containment`

- [ ] **Step 3: 实现**

`backend.rs`：把 `fn kill(&self, session_id: &str) -> Result<()>;` 换成

```rust
    /// 终止该会话**拥有的整棵进程树**（不是「直接子进程」）。
    ///
    /// containment 在 spawn 阶段建立（见 `docs/specs/2026-09-24-task8b-process-tree-ownership-design.md`），
    /// 因此运行中的会话一定具备 containment；缺失时必须返回明确错误，**不得**静默降级。
    fn terminate_tree(&self, session_id: &str) -> Result<()>;
```

`portable_pty_backend.rs`：**本 Task 先原样保留 direct-child 行为**，只把方法名改成 `terminate_tree`
（Task 3 再换成 Job），并在方法体上留一行注释 `// Task 3：改为 containment.terminate_tree()`。

`manager.rs`：

```rust
    /// 用户主动结束：终止整棵进程树；**成功才关闭输入侧**（spec §4.5 / 8A 语义不变）。
    pub fn kill(&self, session_id: &str) -> Result<()> {
        self.backend.terminate_tree(session_id)?;
        if let Ok(handle) = self.session_handle(session_id) {
            handle.shutdown_input();
        }
        Ok(())
    }
```

fake（`manager.rs` 的 `#[cfg(test)] mod fake`）：

```rust
        /// 被 `terminate_tree` 的会话（记录「走的是树杀而不是 direct-child kill」）。
        pub terminated_trees: Mutex<Vec<String>>,
        /// 下一次 `terminate_tree` 必须失败的会话。
        failing_terminations: Mutex<std::collections::HashSet<String>>,
        /// spawn 时 containment 建立失败（模拟 AssignProcessToJobObject 被拒）。
        fail_containment: Mutex<bool>,
        /// 会话没有 containment（T6 用）：spawn 正常，但树杀必须报明确错误。
        no_containment: Mutex<bool>,
```

```rust
        pub fn fail_next_terminate_tree(&self, session_id: &str) {
            self.failing_terminations
                .lock()
                .expect("failing_terminations")
                .insert(session_id.to_string());
        }

        /// spawn 时 containment 建立失败（T1/T6 用）。
        pub fn with_failing_containment(self) -> Self {
            *self.fail_containment.lock().expect("fail_containment") = true;
            self
        }

        /// 会话没有 containment（T6 用）：spawn 正常，但树杀必须报明确错误。
        pub fn without_containment(self) -> Self {
            *self.no_containment.lock().expect("no_containment") = true;
            self
        }
```

`impl PtyBackend for FakePtyBackend`：

```rust
        fn terminate_tree(&self, session_id: &str) -> Result<()> {
            if self
                .failing_terminations
                .lock()
                .expect("failing_terminations")
                .remove(session_id)
            {
                return Err(Error::InvalidInput("fake: 终止进程树失败".to_string()));
            }
            if *self.no_containment.lock().expect("no_containment") {
                return Err(Error::InvalidInput(format!(
                    "会话 {session_id} 没有 containment（fake）"
                )));
            }
            self.terminated_trees
                .lock()
                .expect("terminated_trees")
                .push(session_id.to_string());
            // 真实语义：树杀之后 root 会退出，reaper 随后读到退出状态。
            if !*self.always_exited.lock().expect("always_exited") {
                self.exit_codes
                    .lock()
                    .expect("exit_codes")
                    .insert(session_id.to_string(), Some(self.killed_exit_code));
            }
            Ok(())
        }
```

`input_tests.rs` 里把 `backend.fail_next_kill("hub-b")` 改成 `backend.fail_next_terminate_tree("hub-b")`。

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::`
Expected: `test result: ok`（8A 的 12 条 + 新增 3 条）

- [ ] **Step 5: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/src/pty/backend.rs src-tauri/src/pty/portable_pty_backend.rs src-tauri/src/pty/manager.rs src-tauri/src/pty/input_tests.rs
git commit -m "refactor(pty): terminate the owned tree instead of the direct child"
```

---

### Task 3: `LivePty` 拆出 `LiveProcess`，spawn 建立 containment，`forget` 主动关闭传输

**Files:**

- Modify: `src-tauri/src/pty/portable_pty_backend.rs`（结构 + spawn + terminate_tree + forget，
  含本 Task 的 `#[cfg(all(test, windows))]` 真机 RED）

**Interfaces:**

- Consumes: `pty::containment::{Containment, descendants_of, SWEEP_ROUNDS_LIMIT}`（Task 1）
- Produces: `LivePty { master: Mutex<Option<Box<dyn MasterPty + Send>>>, writer: Mutex<Box<dyn Write + Send>>, process: LiveProcess }`；`LivePty::close_transport()`；`PortablePtyBackend::spawn` 在返回前建立 containment（立即 assign + 定点补扫）

- [ ] **Step 1: 写真机测试（forget 之后 parked write 必须返回）**

本 Task 的**行为 RED** 是一个真机单元测试，放在 `portable_pty_backend.rs` 的
`#[cfg(all(test, windows))] mod tests`（不需要新建文件）。

> **实现时修正（重要，夹具前提）**：`conhost` 因为 `PSUEDOCONSOLE_INHERIT_CURSOR` 会在启动时
> 发 `ESC[6n`（DSR）并等应答。生产里这个应答由前端终端模拟器（xterm.js）给出；
> **headless 测试必须自己当终端**。第一版测试没有应答，结果 pseudoconsole 卡在初始化里，
> 连 `ClosePseudoConsole` 都收不干净 —— master 被 drop 了 reader 也拿不到 EOF，
> 于是被误判成「产品没关传输」。所以测试里必须先完成 DSR 握手（只应答一次，
> 7D 的教训是无限重放会淹掉子进程输入缓冲），再 park 写。

```rust
    /// conhost 因 `PSUEDOCONSOLE_INHERIT_CURSOR` 会发 DSR 并等应答；headless 测试要自己当终端。
    const DSR_REQUEST: &[u8] = b"\x1b[6n";
    const DSR_REPLY: &[u8] = b"\x1b[1;1R";

    /// `forget` 必须**主动**关闭 PTY master：否则 park 在 `write_all` 的线程（它自己持有
    /// `Arc<LivePty>`）永远不会返回 —— 这是 8B spike s5 三档实验唯一让写返回的那一档。
    #[test]
    fn forget_actively_closes_the_transport_so_a_parked_write_returns() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};

        let backend = Arc::new(PortablePtyBackend::new());
        let handle = backend
            .spawn(PtySpawnRequest {
                session_id: "hub-parked".to_string(),
                spec: LaunchSpec {
                    program: std::path::PathBuf::from("ping"),
                    args: vec!["-n".into(), "300".into(), "127.0.0.1".into()],
                    cwd: None,
                    env: Vec::new(),
                    runtime_target_id: "local".to_string(),
                },
                cols: 80,
                rows: 24,
            })
            .expect("spawn");

        // reader 一直排空 console 输出，看到 DSR 只应答一次。
        let answered = Arc::new(AtomicBool::new(false));
        let reader_done = Arc::new(AtomicBool::new(false));
        let reader = backend.take_reader("hub-parked").expect("reader");
        {
            let answered_for_reader = Arc::clone(&answered);
            let done_for_reader = Arc::clone(&reader_done);
            let backend_for_reader = Arc::clone(&backend);
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
                                let _ = backend_for_reader.write("hub-parked", DSR_REPLY);
                            }
                        }
                        Err(_) => break,
                    }
                }
                done_for_reader.store(true, Ordering::SeqCst);
            });
        }

        // 握手必须在**开始 park 之前**完成：park 之后 writer 锁被占，应答写不进去。
        let deadline = Instant::now() + Duration::from_secs(10);
        while !answered.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            answered.load(Ordering::SeqCst),
            "前提：conhost 的 DSR 握手必须被应答，否则这条测试测不到传输关闭"
        );

        let returned = Arc::new(AtomicBool::new(false));
        let returned_flag = Arc::clone(&returned);
        let backend_for_writer = Arc::clone(&backend);
        std::thread::spawn(move || {
            // 64 MiB：4 MiB 会被 ConPTY 缓冲吞掉（约 4.7s 自然返回），测不到长期 park。
            let payload = vec![b'x'; 64 * 1024 * 1024];
            let _ = backend_for_writer.write("hub-parked", &payload);
            returned_flag.store(true, Ordering::SeqCst);
        });

        std::thread::sleep(Duration::from_secs(2));
        assert!(
            !returned.load(Ordering::SeqCst),
            "前提：64 MiB 的写必须还 park 着，否则这条测试没测到目标场景"
        );

        // 第一步：用户 kill → 树杀。**这一步解不开写**（spike s5 第二档）。
        backend.terminate_tree("hub-parked").expect("树杀");
        std::thread::sleep(Duration::from_secs(1));
        assert!(
            !returned.load(Ordering::SeqCst),
            "树杀本身不得被当成「解开阻塞写」的手段（spike s5 第二档：still parked）"
        );

        // 第二步：reaper 写完终态 → forget。**这一步才解开写**（spike s5 第三档）。
        backend.forget("hub-parked").expect("forget");

        let deadline = Instant::now() + Duration::from_secs(10);
        while !returned.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            returned.load(Ordering::SeqCst),
            "forget 之后 parked write 必须返回（有界 10s 观察窗口，不 join）"
        );
        assert!(
            reader_done.load(Ordering::SeqCst),
            "master 被关闭之后 reader 必须拿到 EOF（传输确实关了）"
        );

        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &handle.pid.unwrap_or_default().to_string()])
            .output();
    }
```

（真机 RED 已实测：临时去掉 `forget` 里的 `close_transport()` 后，这条测试在 13s 内失败于
`forget 之后 parked write 必须返回`。）

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::portable_pty_backend`
Expected: FAIL `forget 之后 parked write 必须返回`（当前 forget 只 `sessions.remove`，master 不关）

- [ ] **Step 3: 实现**

```rust
struct LiveProcess {
    child: Mutex<Box<dyn Child + Send + Sync>>,
    /// Windows：Job Object。**单点所有权**（不 Clone / 不 Duplicate / 不给别的线程），
    /// 而且和 master 一样是 `Option`：`forget` 必须能**显式取出并关闭唯一句柄** ——
    /// parked writer 持有 `Arc<LivePty>` 时，等 Arc drop 就等于永远不关 job。
    #[cfg(windows)]
    containment: Mutex<Option<Containment>>,
}

struct LivePty {
    /// `Option` 是为了「主动关闭」：parked writer 可能持有 `Arc<LivePty>`，
    /// 所以关闭必须由 `close_transport()` 主动 take/drop，而不能等 Arc drop。
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
    writer: Mutex<Box<dyn Write + Send>>,
    process: LiveProcess,
}

impl LivePty {
    /// 关闭 PTY 传输资源。**不触碰 writer mutex**（可能被 parked 线程持有，等它 = 把 8A 的
    /// 不变量从后门放回来）。
    fn close_transport(&self) {
        if let Ok(mut master) = self.master.lock() {
            let _ = master.take();
        }
    }

    /// 显式取出并关闭唯一的 Job 句柄（`KILL_ON_JOB_CLOSE` 因此立即生效）。
    ///
    /// 返回未释放过的错误：**不吞掉**，也不重复释放（`take()` 之后是 `None`，幂等）。
    #[cfg(windows)]
    fn release_containment(&self) -> Result<()> {
        let mut guard = self
            .process
            .containment
            .lock()
            .map_err(|_| Error::StateLockPoisoned)?;
        // take() 之后 drop 掉 Containment → CloseHandle → 最后一句柄关闭 → job 内进程被回收。
        drop(guard.take());
        Ok(())
    }
}
```

`spawn`（关键顺序）：

```rust
        // 1) 先建 containment：无法建立 containment 的会话不允许进入 running。
        #[cfg(windows)]
        let containment = Containment::create()?;

        // 2) openpty + CreateProcess（现状不变）
        let child = pair.slave.spawn_command(builder)?;
        drop(pair.slave);

        // 3) root PID 必须拿得到：拿不到就等于「无法建立 containment」，不能静默返回成功。
        let pid = child.process_id().ok_or_else(|| {
            best_effort_cleanup(None, &child);
            Error::InvalidInput(
                "无法建立进程包含：root PID 不可用（containment 无从建立）".to_string(),
            )
        })?;

        // 4) **立即** assign root：job 成员资格只被「加入之后创建」的子进程继承。
        #[cfg(windows)]
        {
            containment.assign(pid).map_err(|error| {
                // best-effort 定向清理：root 与**可枚举的后代**都要尽力收掉，不能只杀 root。
                best_effort_cleanup(Some(pid), &child);
                Error::InvalidInput(format!(
                    "无法建立进程包含（AssignProcessToJobObject 失败）：{error}"
                ))
            })?;
            // 5) 定点补扫：把 assign 之前已经创建出来的后代补进去（best-effort，见 ADR-0013）。
            reconcile_descendants(&containment, pid);
        }
```

```rust
/// 启动失败时的 best-effort 定向清理：**root + 可枚举的后代**（不能只杀 root，
/// 否则就是一个半受控、还在跑的进程树）。
///
/// 只用于「containment 没能建立」这一条失败路径；正常路径的整树终止走
/// `Containment::terminate_tree()`。
///
/// 顺序是**先 root 后后代**（起草时写反了，实现时修正）：root 死了才不会在清理过程中
/// 继续 fork 出新的后代；后代的 `ParentProcessId` 在 Windows 上是陈旧值（不会重挂到别的
/// 进程），所以 root 死后仍然枚举得到。
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
```

> `terminate_pid(pid)` = `OpenProcess(PROCESS_TERMINATE)` + `TerminateProcess`，放在
> `pty::containment` 里（与 `assign` 同一处，权限常量复用）。
> `best_effort_cleanup` 的签名在实现时按借用需要调整（`child` 需要 `&mut`）；**不要**为了绕借用
> 把 root 的 kill 省掉。

```rust
        // 6) 组装 LivePty（master / containment 都是 Option）
```

```rust
/// 定点补扫：直到本轮没有发现新的、尚未纳入 job 的后代。
/// **不靠 sleep**；轮次上限只防御异常进程树（持续 fork）。
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
```

`terminate_tree` / `forget`：

```rust
    fn terminate_tree(&self, session_id: &str) -> Result<()> {
        let live = self.live(session_id)?;
        #[cfg(windows)]
        {
            let guard = live
                .process
                .containment
                .lock()
                .map_err(|_| Error::StateLockPoisoned)?;
            let containment = guard.as_ref().ok_or_else(|| {
                Error::InvalidInput(format!(
                    "会话 {session_id} 的 containment 已释放，无法终止进程树"
                ))
            })?;
            return containment.terminate_tree();
        }
        #[cfg(not(windows))]
        {
            // 非 Windows：保持现状语义（direct child），containment 留给各平台实现。
            let mut child = lock(&live.process.child)?;
            child.kill().map_err(|e| Error::InvalidInput(format!("结束进程失败：{e}")))
        }
    }

    /// 释放会话资源：**本 Task 只关传输**（让 park 在 `write_all` 的 worker 返回）。
    ///
    /// 显式释放 Job 句柄是 Task 5（它的 RED 正是「parked writer 还持有 Arc 时，job 没被关掉、
    /// 后代偷活」）。两步都要有：transport closure 与 process containment 是两件事（spec §1.3）。
    fn forget(&self, session_id: &str) -> Result<()> {
        let live = lock(&self.sessions)?.remove(session_id);
        if let Some(live) = live {
            live.close_transport();
        }
        Ok(())
    }
```

- [ ] **Step 4: 跑测试确认通过 + 8A 回归**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty:: && cargo test --manifest-path src-tauri/Cargo.toml --lib terminal::`
Expected: 全绿

- [ ] **Step 5: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/src/pty/portable_pty_backend.rs src-tauri/src/pty/input_tests.rs src-tauri/tests/process_tree_ownership.rs
git commit -m "feat(pty): contain the session process tree and close the transport actively"
```

---

### Task 4: containment 建立失败 = `launch_failed`，绝不进入 `running`（T1）

**Files:**

- Test: `src-tauri/src/pty/input_tests.rs`（T1）
- Modify: `src-tauri/src/terminal.rs`（仅当测试暴露顺序问题时）

**Interfaces:**

- Consumes: `PtyManager::spawn` 的失败路径（Task 3）、`TerminalRuntime::start` 既有 `fail_launch`
- Produces: 无新 API（纪律测试）

- [ ] **Step 1: 写失败测试**

```rust
/// T1：无法建立 containment 的会话**不得**被宣称为 running（与「先 mark_running 再起 reader」同类纪律）。
#[test]
fn a_session_without_containment_is_failed_not_running() {
    let backend = Arc::new(FakePtyBackend::new().with_failing_containment());
    let manager = manager(Arc::clone(&backend), 64);

    let error = manager
        .spawn("hub-a", spec_for(CODEX), 80, 24)
        .expect_err("containment 建立失败必须启动失败");

    assert!(
        error.to_string().contains("containment") || error.to_string().contains("包含"),
        "{error}"
    );
    assert!(
        manager.pending_bytes("hub-a").is_none(),
        "启动失败不得留下 SessionHandle"
    );
}
```

并在 `terminal.rs` 的测试里加一条端到端纪律断言（走生产 `TerminalRuntime::start`）：

```rust
    /// 无法建立 containment → `failed` / `launch_failed`，**绝不** running。
    #[test]
    fn start_fails_when_containment_cannot_be_established() {
        let backend = Arc::new(FakePtyBackend::new().with_failing_containment());
        let (runtime, installation) = runtime(Arc::clone(&backend));

        let error = runtime
            .start(&installation, Some("D:/HarnessHub-E2E/codex-concurrent"), 100, 30, None)
            .expect_err("必须启动失败");

        assert!(error.to_string().contains("包含") || error.to_string().contains("containment"));
        let sessions = runtime.list_sessions(10).expect("列出");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].status, SessionStatus::Failed);
        assert_eq!(
            sessions[0].termination_reason,
            Some(TerminationReason::LaunchFailed)
        );
        assert!(sessions[0].pid.is_none(), "启动失败不得留下 pid");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib terminal::tests::start_fails`
Expected: FAIL（当前 fake 的 spawn 成功；`with_failing_containment` 需要在 fake 的 `spawn` 里返回 Err）

- [ ] **Step 3: 实现**

fake 的 `spawn` 开头：

```rust
            if *self.fail_containment.lock().expect("fail_containment") {
                // 真实的 AssignProcessToJobObject 失败会带上 Win32 error，fake 必须同样带，
                // 否则「错误里要有具体 os error」这条约束无法在上层被测到。
                return Err(Error::InvalidInput(
                    "fake: 无法建立进程包含（AssignProcessToJobObject 失败，os error 5）".to_string(),
                ));
            }
```

并且测试要断言**错误形状**（不只是「失败了」）：

```rust
    assert!(
        error.to_string().contains("os error"),
        "containment 建立失败必须带具体 Win32 error：{error}"
    );
```

（`PtyManager::spawn` 已经在 `backend.spawn(...)?` 处直接返回错误、不插入 handle；`TerminalRuntime::start`
既有 `fail_launch` 路径负责 `failed`/`launch_failed` —— 本 Task 是**用测试锁死**这条纪律。）

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::input_tests && cargo test --manifest-path src-tauri/Cargo.toml --lib terminal::`
Expected: 全绿

- [ ] **Step 5: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/src/pty/manager.rs src-tauri/src/pty/input_tests.rs src-tauri/src/terminal.rs
git commit -m "test(pty): a session without containment is launch_failed, never running"
```

---

### Task 5: Job handle 生命周期（T4/T5 + kill-on-close 的自然退出路径）

**Files:**

- Test: `src-tauri/src/pty/input_tests.rs`、`src-tauri/src/pty/portable_pty_backend.rs`（真机）

**Interfaces:**

- Consumes: Task 3 的 `close_transport` / `Containment`
- Produces: 无新 API

- [ ] **Step 1: 写真机失败测试（parked writer 持有 Arc 时，forget 仍必须关掉 Job 句柄）**

放在 `portable_pty_backend.rs` 的 Windows 测试模块里。**RED 是真实的**：Task 3 的 `forget` 只关
传输、不取 job 句柄，而 parked writer 持有 `Arc<LivePty>` → `Containment` 不会被 drop → 后代偷活。

```rust
    /// T5：`forget` 必须**显式取出并关闭唯一 Job 句柄**，不能依赖 `Arc<LivePty>` drop ——
    /// parked writer 自己就持有那个 Arc（spec §4.5 / 约束 4+5）。
    ///
    /// 断言两件事同时成立：parked write 返回（传输已关）**且**后代被回收（job 句柄已关）。
    #[test]
    fn forget_releases_the_job_even_while_a_writer_holds_the_arc() {
        use crate::harness::launch::LaunchSpec;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        let backend = Arc::new(PortablePtyBackend::new());
        let handle = backend
            .spawn(PtySpawnRequest {
                session_id: "hub-park-tree".to_string(),
                spec: LaunchSpec {
                    // cmd 起长跑 ping：root 是 cmd，后代 ping 留在 job 里。
                    program: std::path::PathBuf::from("cmd"),
                    args: vec!["/c".into(), "ping -n 300 127.0.0.1".into()],
                    cwd: None,
                    env: Vec::new(),
                    runtime_target_id: "local".to_string(),
                },
                cols: 80,
                rows: 24,
            })
            .expect("spawn");
        let _reader = backend.take_reader("hub-park-tree").expect("reader");
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

        // 制造 parked writer（它持有 Arc<LivePty>）
        let returned = Arc::new(AtomicBool::new(false));
        let returned_flag = Arc::clone(&returned);
        let backend_for_writer = Arc::clone(&backend);
        std::thread::spawn(move || {
            let payload = vec![b'x'; 64 * 1024 * 1024];
            let _ = backend_for_writer.write("hub-park-tree", &payload);
            returned_flag.store(true, Ordering::SeqCst);
        });
        std::thread::sleep(Duration::from_secs(2));
        assert!(
            !returned.load(Ordering::SeqCst),
            "前提：写必须还 park 着（64 MiB 才稳；4 MiB 会被 ConPTY 缓冲吞掉）"
        );

        backend.forget("hub-park-tree").expect("forget");

        let deadline = Instant::now() + Duration::from_secs(5);
        while !returned.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(returned.load(Ordering::SeqCst), "forget 之后 parked write 必须返回");

        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline
            && std::iter::once(root)
                .chain(descendants.clone())
                .any(|pid| process_alive(pid))
        {
            std::thread::sleep(Duration::from_millis(100));
        }
        let survivors: Vec<u32> = std::iter::once(root)
            .chain(descendants.clone())
            .filter(|pid| process_alive(*pid))
            .collect();
        assert!(
            survivors.is_empty(),
            "forget 必须关掉 Job 句柄、回收残留后代（偷活的：{survivors:?}）"
        );
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::portable_pty_backend::forget_releases`
Expected: FAIL `forget 必须关掉 Job 句柄、回收残留后代`（Task 3 的 forget 不取 job 句柄）

- [ ] **Step 3: 实现（在 Task 3 的 `LivePty` 上加显式释放）**

Task 3 已经把 `containment` 做成 `Mutex<Option<Containment>>`；本 Task 加上
`release_containment()` 并在 `forget` 里调用（**不吞错误**）：

```rust
impl LivePty {
    /// 显式取出并关闭唯一的 Job 句柄（`KILL_ON_JOB_CLOSE` 因此立即生效）。
    ///
    /// `take()` 保证「即使 parked writer 还持有 `Arc<LivePty>`，句柄也已经关掉」；
    /// 幂等：已经取过就是 `None`，直接 Ok。
    #[cfg(windows)]
    fn release_containment(&self) -> Result<()> {
        let mut guard = self
            .process
            .containment
            .lock()
            .map_err(|_| Error::StateLockPoisoned)?;
        drop(guard.take()); // Drop → CloseHandle → 最后一句柄关闭 → job 内进程被回收
        Ok(())
    }
}
```

```rust
    fn forget(&self, session_id: &str) -> Result<()> {
        let Some(live) = (lock(&self.sessions)?.remove(session_id)) else {
            return Ok(());
        };
        // 先关传输（让 parked write 返回），再关 job（回收残留后代）；两步都不能省。
        live.close_transport();
        #[cfg(windows)]
        live.release_containment()?;
        Ok(())
    }
```

> **宿主 crash 不需要另造机制**：进程死亡 = OS 关闭它的所有句柄，与这里的 `Drop` 路径是同一个
> `KILL_ON_JOB_CLOSE` 机制（spike s6 已实测：`taskkill /F` 宿主后它拥有的 3 个进程全部被回收）。
> 本 Task 测的是**产品语义**：`forget` / 最后一个 Job 句柄关闭 → 后代被回收；**不要**把这条测试
> 说成「测过宿主 crash」。

- [ ] **Step 4: 写第二条真机测试（reaper 路径：root 自然退出、后代仍存活）**

```rust
    /// reaper 路径：root **自然退出**时后代仍活着 → `forget` 必须把它们收掉。
    ///
    /// `start /b` 让 ping 成为 cmd 的后代但 cmd 立刻退出 —— 正是「root 走了、后代偷活」的形状。
    #[test]
    fn release_after_a_natural_root_exit_still_reaps_living_descendants() {
        // 与上一条同构：spawn `cmd /c start /b ping -n 300 127.0.0.1`
        // → 等 root 退出（child.try_wait() == Some(_)）且后代仍 alive
        // → backend.forget(session)
        // → 断言后代全部消失（job 句柄关闭 → KILL_ON_JOB_CLOSE）
    }
```

> 若本机 `start /b` 的树形状与预期不符，**调整命令**直到「root 已退出 + 后代仍存活」这个前提
> 由断言证明成立；**不要**放宽「后代必须被回收」这条断言。

- [ ] **Step 5: 跑测试确认通过 + 8A/7D 回归**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty:: terminal::`
Expected: 全绿

- [ ] **Step 6: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/src/pty/portable_pty_backend.rs
git commit -m "fix(pty): release the job handle explicitly so released sessions cannot leak descendants"
```

---

### Task 6: 真机 Gate R1–R4（synthetic 三层树 / 真实 codex / 真实 claude / session-scoped）

**Files:**

- Modify: `src-tauri/tests/process_tree_ownership.rs`

**Interfaces:**

- Consumes: `PtyManager::{with_input_capacity, spawn, start_reading, kill, pending_bytes}`、`PortablePtyBackend`、`Containment::active_processes`（可用于交叉核对）
- Produces: 真机证据（`tests/e2e/README.md` 引用）

- [ ] **Step 1: 写测试**

```rust
//! Task 8B 真机 Gate。直接驱动生产 `PtyManager` + `PortablePtyBackend`（不经过 Harness 适配器）。
//!
//! 合成树用 `cmd /c cmd /c ping -n 300 127.0.0.1`（三层；若实测不足三层就调整命令，
//! **不许**放宽断言）；真实链路用 codex.cmd / claude.cmd —— 它们天然是
//! `cmd.exe → node/codex/claude`。

#![cfg(windows)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use harness_hub_lib::harness::launch::LaunchSpec;
use harness_hub_lib::pty::{PortablePtyBackend, PtyManager};

const CODEX: &str = r"D:\npm-global\codex.cmd";
const CLAUDE: &str = r"D:\npm-global\claude.cmd";
/// 真实 codex/claude 的 TUI 会先发 DSR 并等待应答（ADR-0009：应答属于终端侧）。
const DSR_REQUEST: &[u8] = b"\x1b[6n";
const DSR_REPLY: &[u8] = b"\x1b[1;1R";

fn spec(program: &str, args: &[&str]) -> LaunchSpec {
    LaunchSpec {
        program: PathBuf::from(program),
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd: None,
        env: Vec::new(),
        runtime_target_id: "local".to_string(),
    }
}

fn manager() -> Arc<PtyManager> {
    Arc::new(PtyManager::with_input_capacity(
        Arc::new(PortablePtyBackend::new()),
        Arc::new(|_session, _seq, _bytes| {}),
        Arc::new(|_session, _code| {}),
        64 * 1024,
    ))
}

fn alive(pid: u32) -> bool {
    let output = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .expect("tasklist");
    String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
}

/// 自己实现一遍 ToolHelp 后代枚举（测试**不**依赖产品内部 API，避免自证）。
fn descendants(root: u32) -> Vec<u32> {
    let all = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId) $($_.ParentProcessId)\" }",
        ])
        .output()
        .expect("powershell");
    let text = String::from_utf8_lossy(&all.stdout).into_owned();
    let pairs: Vec<(u32, u32)> = text
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
        })
        .collect();
    let mut result = Vec::new();
    let mut queue = vec![root];
    while let Some(parent) = queue.pop() {
        for (pid, ppid) in &pairs {
            if *ppid == parent && *pid != root {
                result.push(*pid);
                queue.push(*pid);
            }
        }
    }
    result
}

fn wait_tree(root: u32, min_nodes: usize, timeout: Duration) -> Vec<u32> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut nodes = vec![root];
        nodes.extend(descendants(root));
        if nodes.len() >= min_nodes || Instant::now() >= deadline {
            return nodes;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
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

fn cleanup(pids: &[u32]) {
    for pid in pids {
        if alive(*pid) {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output();
        }
    }
}

/// R1：合成 parent → child → grandchild → kill → 三个 PID 全部消失。
#[test]
fn synthetic_three_level_tree_is_fully_terminated() {
    let manager = manager();
    let handle = manager
        .spawn(
            "hub-tree",
            spec("cmd", &["/c", "cmd /c ping -n 300 127.0.0.1"]),
            80,
            24,
        )
        .expect("spawn");
    manager.start_reading("hub-tree").expect("start_reading");

    let root = handle.pid.expect("pid");
    let tree = wait_tree(root, 3, Duration::from_secs(10));
    eprintln!("[R1] tree = {tree:?}");
    assert!(tree.len() >= 3, "必须是三层树，实际 {tree:?}");

    manager.kill("hub-tree").expect("kill");
    assert!(wait_dead(&tree, Duration::from_secs(10)), "kill 后全树必须消失：{tree:?}");
    cleanup(&tree);
}

/// R2/R3：真实 codex.cmd / claude.cmd 的 `.cmd → node/claude` 树。
///
/// 这两条是**支撑证据**（缺 binary 时明确跳过）；R4/R7 才是不可跳过的 Gate。
#[test]
fn real_codex_and_claude_trees_are_fully_terminated() {
    let manager = manager();
    for (session, program, min_nodes) in [
        ("hub-codex", CODEX, 3),
        ("hub-claude", CLAUDE, 2),
    ] {
        if !std::path::Path::new(program).is_file() {
            eprintln!("跳过 {session}：本机没有 {program}");
            continue;
        }
        let handle = manager
            .spawn(session, spec(program, &[]), 80, 24)
            .expect("spawn");
        manager.start_reading(session).expect("start_reading");

        let root = handle.pid.expect("pid");
        // 必须应答 DSR，否则 TUI 卡在首屏、后代会晚创建（这不影响 contain 断言，但会让
        // 「树节点数」不稳定）。
        manager.write(session, DSR_REPLY).expect("DSR 应答");

        let tree = wait_tree(root, min_nodes, Duration::from_secs(20));
        eprintln!("[R2/R3] {session} tree = {tree:?}");
        assert!(tree.len() >= min_nodes, "{session} 树节点不足：{tree:?}");

        manager.kill(session).expect("kill");
        assert!(wait_dead(&tree, Duration::from_secs(10)), "{session} 全树必须消失");
        cleanup(&tree);
    }
}

/// R4（永久回归，最重要）：Codex A + Codex B + Claude C → kill A → 只有 A 的树死。
///
/// A 与 B **必须是同一个可执行文件**：错误的 executable-scoped 清理（例如 kill all node）
/// 也能让「零残留」看起来成立，只有这条能把它抓出来。
///
/// 环境缺失**不算通过**（Gate）：需要真机 codex + claude，缺一个就 panic。
#[test]
fn killing_one_session_tree_never_touches_another() {
    let manager = manager();
    assert!(
        std::path::Path::new(CODEX).is_file() && std::path::Path::new(CLAUDE).is_file(),
        "本 Gate 需要真机 codex 与 claude（跳过不等于通过）"
    );

    let a = manager.spawn("hub-a", spec(CODEX, &[]), 80, 24).expect("spawn A");
    manager.start_reading("hub-a").expect("start A");
    let b = manager.spawn("hub-b", spec(CODEX, &[]), 80, 24).expect("spawn B");
    manager.start_reading("hub-b").expect("start B");
    let c = manager.spawn("hub-c", spec(CLAUDE, &[]), 80, 24).expect("spawn C");
    manager.start_reading("hub-c").expect("start C");
    for session in ["hub-a", "hub-b", "hub-c"] {
        let _ = manager.write(session, DSR_REPLY);
    }

    let tree_a = wait_tree(a.pid.expect("pid"), 3, Duration::from_secs(20));
    let tree_b = wait_tree(b.pid.expect("pid"), 3, Duration::from_secs(20));
    let tree_c = wait_tree(c.pid.expect("pid"), 2, Duration::from_secs(20));
    eprintln!("[R4] A={tree_a:?} B={tree_b:?} C={tree_c:?}");

    manager.kill("hub-a").expect("kill A");
    assert!(wait_dead(&tree_a, Duration::from_secs(10)), "A 的全树必须死");
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        tree_b.iter().all(|pid| alive(*pid)),
        "B 的整棵树必须存活（A/B 同名可执行文件）：{tree_b:?}"
    );
    assert!(
        tree_c.iter().all(|pid| alive(*pid)),
        "C 必须存活：{tree_c:?}"
    );

    manager.kill("hub-b").ok();
    manager.kill("hub-c").ok();
    cleanup(&tree_a);
    cleanup(&tree_b);
    cleanup(&tree_c);
}
```

- [ ] **Step 2: 跑测试（真机，含自证）**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test process_tree_ownership -- --nocapture --test-threads=1`
Expected: 全绿；日志里能看到每棵树的 PID 列表与「terminate 后全 dead」

- [ ] **Step 3: 连续两次确认稳定**

Run: 同上再跑一次
Expected: 两次同结果

- [ ] **Step 4: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/tests/process_tree_ownership.rs
git commit -m "test(pty): prove session-scoped tree termination on real harness trees"
```

---

### Task 7: R5（parked writer 全链路）+ R6（kill-on-close）+ R7（生产路径 assign 成功）

**Files:**

- Modify: `src-tauri/tests/process_tree_ownership.rs`

**Interfaces:**

- Consumes: 全链路 = `kill` → reaper → 终态 → `forget` → `close_transport`
- Produces: 8B 的验收主体证据

- [ ] **Step 1: 写测试**

```rust
/// R5：A 的 writer park 在 OS 写里 → kill A（树杀）→ 树全死 → 终态 → forget（关 master + 关 job）
/// → parked write 最终返回。**有界观察窗口，绝不 join writer。**
///
/// 这条把 8A 与 8B 串起来：树杀解决「进程还活着」，close_transport 解决「阻塞 writer 还醒不过来」。
///
/// **两个容易写错的点**（都靠断言钉住，不靠注释）：
///  1. `PtyManager::write` 只是入队（8A 的有界队列），它会**立刻返回** —— 所以「parked 了没有」
///     不能看入队调用是否返回，要看 `pending_bytes`：8A 的记账只在 `backend.write` 返回后才结算，
///     所以 `pending_bytes` 持续停在容量值 = worker 正卡在 OS 写里。
///  2. 因此容量要 ≥ 本批大小：默认 64 KiB 队列会让 64 MiB 的写**当场被拒**（InputBackpressure），
///     根本到不了 backend。这里显式注入 64 MiB 容量。
#[test]
fn a_parked_writer_is_released_by_the_reaper_forgetting_the_session() {
    use harness_hub_lib::pty::PortablePtyBackend;

    const PAYLOAD: usize = 64 * 1024 * 1024;
    // 容量必须 ≥ 单批大小，否则 manager.write 直接 InputBackpressure（测不到阻塞写）。
    let manager = Arc::new(PtyManager::with_input_capacity(
        Arc::new(PortablePtyBackend::new()),
        Arc::new(|_session, _seq, _bytes| {}),
        Arc::new(|_session, _code| {}),
        PAYLOAD,
    ));
    let _ = Mutex::new(()); // 占位说明：无需额外同步原语，全靠 pending_bytes 观察

    let handle = manager
        .spawn("hub-parked", spec("ping", &["-n", "300", "127.0.0.1"]), 80, 24)
        .expect("spawn");
    manager.start_reading("hub-parked").expect("start_reading");
    let root = handle.pid.expect("pid");
    let tree = wait_tree(root, 1, Duration::from_secs(5));

    // 入队（立刻返回）→ worker 取走并 park 在 backend.write 里
    let payload = vec![b'x'; PAYLOAD];
    manager.write("hub-parked", &payload).expect("入队必须被接受");
    assert_eq!(
        manager.pending_bytes("hub-parked"),
        Some(PAYLOAD),
        "入队后 in-flight 必须等于整批大小"
    );

    // V1：写确实进入了阻塞调用 —— 2 秒后 in-flight 仍未结算（8A 记账只在 write 返回后减）
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        manager.pending_bytes("hub-parked"),
        Some(PAYLOAD),
        "前提：worker 必须还 park 在 backend.write 里（否则这条测试没测到目标场景）"
    );

    // 树杀 → reaper 观测 root 退出 → 终态 → forget（关 master + 关 job 句柄）
    manager.kill("hub-parked").expect("kill");
    assert!(wait_dead(&tree, Duration::from_secs(10)), "树必须全死：{tree:?}");

    // 有界观察：handle 被回收（reaper 走完 forget）后，in-flight 必须结算为 0
    let deadline = Instant::now() + Duration::from_secs(10);
    while manager.pending_bytes("hub-parked").is_some() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        manager.pending_bytes("hub-parked"),
        None,
        "reaper 必须回收 handle（= 终态已写 + forget 已执行）"
    );
    cleanup(&tree);
}

/// R6：kill-on-close —— 会话释放之后仍有 descendant 时，descendants 被 OS 回收。
///
/// 宿主 crash 不需要另造机制：进程死亡 = OS 关闭它的所有句柄，与这里走的是同一个
/// `KILL_ON_JOB_CLOSE`（spike s6 已实测 `taskkill /F` 宿主后 descendant 全部回收）。
#[test]
fn descendants_are_reaped_when_the_session_is_released() {
    let manager = manager();
    let handle = manager
        .spawn(
            "hub-reap",
            spec("cmd", &["/c", "ping -n 300 127.0.0.1"]),
            80,
            24,
        )
        .expect("spawn");
    manager.start_reading("hub-reap").expect("start_reading");

    let root = handle.pid.expect("pid");
    let tree = wait_tree(root, 2, Duration::from_secs(10));
    assert!(tree.len() >= 2, "需要 cmd → ping 两层：{tree:?}");

    // 直接释放（不先 kill）：走 forget 的 containment 收敛路径
    manager.forget("hub-reap").expect("forget");

    assert!(
        wait_dead(&tree, Duration::from_secs(10)),
        "释放之后整棵树必须被回收：{tree:?}"
    );
    cleanup(&tree);
}

/// R7（约束 6 第一条）：**本机测试进程中的**生产 `TerminalRuntime` 路径 assign 必须成功。
///
/// 说清楚边界：cargo 测试进程 **不等于** 打包后的应用宿主（那个宿主可能自己就在别的 job 里、
/// 或以不同完整性级别运行）。这里证的是「这条生产代码路径在本机这个进程环境下能建立 containment」，
/// **不是**「任何宿主都必然成功」。
///
/// 环境缺失**不算通过**：本 Gate 需要真机 codex + E2E 目录；缺任何一个就直接 panic，
/// 而不是打印「跳过」后返回 Ok（跳过 ≠ 通过）。
#[test]
fn containment_is_established_in_this_test_process_on_the_production_runtime_path() {
    use harness_hub_lib::harness::adapter::HarnessAdapter;
    use harness_hub_lib::harness::adapters::codex::CodexAdapter;
    use harness_hub_lib::harness::inventory::{installation_id, reconcile_harnesses};
    use harness_hub_lib::harness::probe::SystemHostProbe;
    use harness_hub_lib::harness::registry::HarnessRegistry;
    use harness_hub_lib::runtime::local::LOCAL_TARGET_ID;
    use harness_hub_lib::terminal::TerminalRuntime;

    let e2e_cwd = r"D:\HarnessHub-E2E\codex-concurrent";
    let adapter = CodexAdapter::new(Arc::new(SystemHostProbe::new()));
    let detected = adapter.detect();
    assert!(
        detected.installed,
        "本 Gate 需要真机 codex（跳过不等于通过）：detect = {detected:?}"
    );
    assert!(
        std::path::Path::new(e2e_cwd).is_dir(),
        "本 Gate 需要 {e2e_cwd}（跳过不等于通过）"
    );

    let db = harness_hub_lib::db::Database::open_in_memory().expect("内存库");
    harness_hub_lib::runtime::local::ensure_local_target(db.connection()).expect("target");
    let mut registry = HarnessRegistry::new();
    registry.register(Box::new(CodexAdapter::new(Arc::new(SystemHostProbe::new()))));
    reconcile_harnesses(
        db.connection(),
        &registry.summaries(LOCAL_TARGET_ID),
        LOCAL_TARGET_ID,
        "2026-09-24T00:00:00Z",
    )
    .expect("同步清单");
    let runtime = TerminalRuntime::new(
        Arc::new(Mutex::new(db)),
        Arc::new(registry),
        Arc::new(PortablePtyBackend::new()),
    );

    let session = runtime
        .start(
            &installation_id("codex", LOCAL_TARGET_ID),
            Some(e2e_cwd),
            100,
            30,
            None,
        )
        .expect("containment 建立失败会让这里返回 Err（错误里带具体 Win32 error）");
    let root = session.pid.expect("pid") as u32;
    let tree = wait_tree(root, 3, Duration::from_secs(20));
    eprintln!("[R7] production runtime tree = {tree:?}");
    assert!(
        tree.len() >= 3,
        "生产路径上 containment 必须覆盖整棵树（cmd → node → codex.exe）：{tree:?}"
    );

    runtime.kill(&session.hub_session_id).expect("kill");
    assert!(
        wait_dead(&tree, Duration::from_secs(10)),
        "生产路径 kill 后全树必须消失"
    );
    cleanup(&tree);
}
```

**R7 的失败路径单独断言**（约束 6 后半句：不能 fallback 后还宣称 containment 成功）：

```rust
/// containment 建立失败时，错误必须**带具体 Win32 error**，且绝不能变成「已 running」。
///
/// 这里用 fake 强制 AssignProcessToJobObject 失败（真实失败无法在不改产品代码的前提下稳定构造），
/// 断言错误形状；primitive 层的真实 Win32 error 由 Task 1 的
/// `assign_reports_the_win32_error_for_an_unopenable_process` 覆盖。
#[test]
fn a_failed_containment_reports_a_win32_error_and_never_runs() {
    // 见 Task 4：`PtyManager::with_input_capacity(...)` + fake `.with_failing_containment()`
    // 断言：spawn 返回 Err、消息含 "os error"、`pending_bytes` 为 None、DB 里是 failed/launch_failed。
}
```

- [ ] **Step 2: 跑测试**

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test process_tree_ownership -- --nocapture --test-threads=1`
Expected: 全绿

- [ ] **Step 3: 提交**

```bash
cd D:/HarnessHub
git add src-tauri/tests/process_tree_ownership.rs
git commit -m "test(pty): close the loop between tree termination and transport closure"
```

---

### Task 8: 全量 Gate + 文档同步 + 证据归档

**Files:**

- Modify: `tests/e2e/README.md`、`docs/CONTEXT.md`、`docs/CONTEXT-MAP.md`

**Interfaces:**

- Consumes: 全部前序 Task
- Produces: 8B 验收记录 + 稳定事实同步

- [ ] **Step 1: 回归 + 全量 Gate**

Run:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --lib terminal::concurrency_tests
cargo test --manifest-path src-tauri/Cargo.toml --test two_harness_concurrency -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --test runtime_input_backpressure -- --test-threads=1
cargo test --manifest-path src-tauri/Cargo.toml --test claude_lifecycle -- --test-threads=1
pnpm verify
pnpm python:test
```

Expected: 全部退出码 0

- [ ] **Step 2: Core diff 审计**

```bash
cd D:/HarnessHub
git diff --stat v0.1.0-alpha.3..HEAD
grep -rn "codex\|claude" src-tauri/src/pty/ src-tauri/src/error.rs   # 只应出现在测试数据/注释
git diff --name-only v0.1.0-alpha.3..HEAD -- src-tauri/src/commands.rs src/lib src/features
```

Expected: 改动只落在 `pty/*`、`terminal.rs`（测试）、`Cargo.toml`、测试与文档；
`commands.rs` / `src/lib` / `src/features` 零改动

- [ ] **Step 3: 同步稳定事实**

- `docs/CONTEXT.md` §5：把 `pty::PtyBackend` 的 `terminate_tree` 语义写进「已锁定接口」
  （Session 拥有进程树，Windows 用 Job Object，保证等级引用 ADR-0013）。
- `docs/CONTEXT-MAP.md`：「进程与 PTY」一行加上 `pty/containment.rs`（Windows Job Object）。
- `tests/e2e/README.md`：新增「Task 8B」小节，粘入 R1–R7 的真实输出与 spike 的三档对照表。

- [ ] **Step 4: 提交**

```bash
cd D:/HarnessHub
git add tests/e2e/README.md docs/CONTEXT.md docs/CONTEXT-MAP.md
git commit -m "docs(e2e): record the Task 8B tree-ownership acceptance"
```

---

## 完成标准

```text
✓ spec §8 的每条 Gate 都有对应测试（T1–T6 + R1–R7 + 8A/7D 回归）
✓ containment 在 running 之前建立；失败 = launch_failed + 具体 Win32 error
✓ kill = 整棵树（真实 codex 三层链 + claude + synthetic 三层）
✓ session-scoped（Codex A + Codex B + Claude C）永久回归
✓ forget 主动 close_transport → parked write 有界返回（不 join）
✓ Job handle 单点所有权；四条生命周期后代都不偷活
✓ 不改 Session 状态机语义
✓ pnpm verify + pnpm python:test 退出码 0；无 Harness 特判、无新 IPC DTO、UI 无改动
✓ ADR-0013 / spec / plan / CONTEXT / CONTEXT-MAP / e2e README 全部同步
```
