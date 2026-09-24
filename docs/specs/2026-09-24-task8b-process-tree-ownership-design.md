# Task 8B — Process Tree Ownership / Termination（设计 / spec）

> **来源**：Task 7D 实测「杀 `.cmd` shim 之后 node 仍存活」+ Task 8B spike（Windows Job Object 可行性）。
> **状态**：设计已 freeze（spike 结论 + 用户 6 条约束；ADR-0013 记录暂缓项）。
> **前置**：Task 8A（跨会话锁下不得有阻塞 I/O）已关账；8A 的 `forget`/reaper 顺序在本 Task 中不变。
> **实现计划**：`docs/plans/2026-09-24-task8b-process-tree-ownership.md`（按 TDD 执行）。

## 1. 问题与 spike 证据

### 1.1 症状

```text
kill_terminal(session_id)
  → 只终止「直接子进程」
  → .cmd 的直接子进程其实是 cmd.exe（shim 解释器）
  → node.exe / codex.exe / claude.exe 是它的后代 → 继续存活
```

### 1.2 spike 实测（真机，`D:\HarnessHub-E2E\spike-8b\`，throwaway）

| 场景                                       | 实测结果                                                                           |
| ------------------------------------------ | ---------------------------------------------------------------------------------- |
| s1 synthetic `parent → child → grandchild` | `job.active_processes = 3`；`TerminateJobObject` → 3/3 dead                        |
| s2 session-scoped（A、B **同名 exe** + C） | 杀 A → `A 全 dead / B 全 ALIVE / C ALIVE`（`same_exe=true`）                       |
| s3 真实 `codex.cmd`                        | 树 = `cmd.exe → node.exe → codex.exe`（3）；`job.active = 3`；terminate → 3/3 dead |
| s4 真实 `claude.cmd`                       | 树 = `cmd.exe → claude.exe`（2）；`job.active = 2`；terminate → 2/2 dead           |
| s6 宿主被 `taskkill /F`                    | `KILL_ON_JOB_CLOSE` → 它拥有的 3 个进程**全部**被 OS 回收                          |
| s7 立即 assign 的可靠性                    | `fully_contained = 5/5`                                                            |
| s2 第一版（延迟 assign）                   | **只杀掉 root**，descendants 逃出 job → 见 §6 保证等级                             |

### 1.3 最重要的**负面**结论（s5 三档对照，载荷 64 MiB，动作前都确认真的 park 住）

```text
1) 只杀 shim (child.kill)          → 后代存活（shim 泄漏），parked write 15s 内不返回
2) 终止整棵树 (TerminateJobObject) → 全部 dead，parked write **仍然** 15s 内不返回
3) 终止整棵树 + 关闭 PTY master    → parked write 0.91s 后返回 Err(管道已结束, os error 109)
```

因此本 Task 的**核心设计原则**是：

```text
Process containment ≠ PTY transport closure

terminate_tree()  解决「进程还活着」
close_transport() 解决「阻塞 writer 还醒不过来」
```

两者必须**分开建模**，再由既有顺序串起来：

```text
kill_terminal → terminate_tree()          （本 Task：结束 Session 拥有的进程树）
reaper 观测到 root 退出 → 写终态 → forget()（Task 8A：关 PTY master）
                                        → park 的 backend.write 返回 Err
                                        → writer worker 自然退出
```

### 1.4 其他实测细节（写进实现注释，避免后人重新踩）

- `.cmd` 的“直接子进程”是 `cmd.exe`；`CreateProcess` 以 `lpApplicationName = <path>.cmd` 是可行的
  （本机 probe + s8 四种形式全部成功）。
- **4 MiB 的写不足以长期 park**：ConPTY 缓冲最终会吞下整批（实测约 4.7s 自然返回）；
  要造稳定 park 需要更大载荷（64 MiB 在 15s 内未返回）。
- 一次 `codex.cmd` spawn 出现瞬时 `ERROR_FILE_NOT_FOUND`，之后 4/4 不复现 —— 记为一次性异常
  （疑似 AV/文件锁），不作为机制。

## 2. 目标 / 非目标

**目标**

1. `kill_terminal(session_id)` = 终止该 Session **拥有的整棵进程树**（不是直接子进程）。
2. containment 必须在 Session 被宣称为 `running` **之前**建立；建立不了就 `launch_failed`。
3. Harness Hub 正常 kill / 自然退出 / 正常关闭 / **异常死亡** 四条路径下，都不留后台 descendant。
4. `forget()` 必须**主动**关闭 PTY 传输资源（不能依赖 `Arc<LivePty>` drop）——这是让 parked
   writer 返回的唯一机制（s5 第 3 档）。
5. 平台细节（Job Object）不泄漏到 `TerminalRuntime`：对外只有
   `ProcessControl { terminate_tree(), try_wait(), pid() }` 语义。

**非目标**

- 不重写 Windows spawn/ConPTY（creation-time containment）——见 ADR-0013，单独立 Task。
- **不改 Session 状态机语义**：`terminate_tree()` 只是控制动作；`user_killed / natural_exit /
lost / host_shutdown` 仍由既有 intent + reaper/reconcile 规则决定，绝不因“树杀成功”就写终态。
- 不做跨平台 containment（Linux/macOS 的 process group / session 留待各自平台实现）。
- 不做 UI 改动、不新增跨 IPC DTO。

## 3. 冻结语义

```text
Session owns a process tree, not a PID.

kill_terminal(session_id)   → terminate the whole owned tree
Harness Hub graceful shutdown → existing host_shutdown semantics remain
Harness Hub abnormal death   → OS tears down owned descendants automatically
reaper                       → remains the only source of observed exit fact
```

`pid()` 仍然只是 root PID，**诊断用**，不是 Session identity（沿用 7D 的既有决定）。

## 4. 组件与接口

### 4.1 结构（Windows）

```text
LivePty
├─ master:  Mutex<Option<Box<dyn MasterPty + Send>>>   ← Option 是为了「主动关闭」（§4.5）
├─ writer:  Mutex<Box<dyn Write + Send>>
└─ process: LiveProcess

LiveProcess
├─ child:       Mutex<Box<dyn Child + Send + Sync>>
└─ containment: Containment            ← Windows: Job Object 句柄（单点所有权）
```

```text
Containment（平台原语，Windows 实现）
  create()                  CreateJobObjectW + SetInformationJobObject(KILL_ON_JOB_CLOSE)
  assign_process(pid)       OpenProcess(SET_QUOTA|TERMINATE) + AssignProcessToJobObject
  terminate_tree()          TerminateJobObject
  is_member(pid)            IsProcessInJob
  drop()                    CloseHandle（最后一句柄 → KILL_ON_JOB_CLOSE）
```

### 4.2 `PtyBackend` trait 变化

```rust
pub trait PtyBackend: Send + Sync {
    fn spawn(&self, request: PtySpawnRequest) -> Result<PtyProcessHandle>;

    /// 终止该会话**拥有的整棵进程树**（不再是「直接子进程」）。
    /// containment 无法建立时 spawn 已经失败，因此运行中的会话一定具备 containment。
    fn terminate_tree(&self, session_id: &str) -> Result<()>;

    fn take_reader(&self, session_id: &str) -> Result<Box<dyn Read + Send>>;
    fn write(&self, session_id: &str, bytes: &[u8]) -> Result<()>;
    fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<()>;
    fn try_wait(&self, session_id: &str) -> Result<Option<i32>>;
    fn is_running(&self, session_id: &str) -> Result<bool>;
    fn forget(&self, session_id: &str) -> Result<()>;
}
```

- 旧 `kill()` 从 trait 上**移除**：它的语义（只杀直接子进程）正是本 Task 要消灭的东西。
- `PtyManager::kill(session_id)` / `TerminalRuntime::kill(session_id)` **名字与签名不变**
  （IPC `kill_terminal`、UI、7D 测试都不受影响），内部改调 `terminate_tree`。

### 4.3 containment 必须在 `running` 之前建立（约束 1）

```text
PtyManager::spawn
  1. backend.spawn(request)
       内部：create Job(KILL_ON_JOB_CLOSE)
             → openpty → CreateProcess(root)
             → 立即 AssignProcessToJobObject(root)
             → descendant 定点补扫（§4.4）
             → Ok(LivePty{ master, writer, process })
       失败：best-effort 定向清理（终止已创建的 root 与其可枚举后代）+ Err
  2. backend.take_reader
  3. pending_readers.insert
  4. SessionHandle::spawn（8A 的输入侧）

TerminalRuntime::start
  5. mark_running(pid)          ← 到这里才允许宣称 running
  6. start_reading              ← 此刻才起 reader/reaper
```

**纪律**：`无法建立 containment 就不能把 Session 宣称为 running`（与 ADR-0010 的
「先 `mark_running` 再起 reader」同类）。root assignment 失败 → `failed` + `launch_failed`
（`TerminalRuntime::fail_launch` 既有路径）+ best-effort 定向清理；**不得**静默降级成
「只有 direct-child kill」。

失败时 `spawn` 的错误信息必须带**具体 Win32 error**（例如
`"无法建立进程包含（AssignProcessToJobObject 失败，os error 5）"`），便于真机排障。

### 4.4 descendant 定点补扫（约束 2）

```text
assign root
↓
loop（最多 SWEEP_ROUNDS_LIMIT = 4 轮）
    descendants := enumerate_descendants(root_pid)      // ToolHelp 快照，不含 root
    newly := 0
    for pid in descendants
        if !containment.is_member(pid)
            containment.assign_process(pid)             // 失败只记日志，不中断整体
            newly += 1
    if newly == 0: break                                // 固定点
```

- **不靠 `sleep` 作为正确性来源**；轮次上限只用于防御异常进程树（持续 fork）。
- 这是 best-effort：它极大缩小 post-spawn 窗口，但**不能**消灭「未被 assign 的后代在扫描前
  又创建后代并退出」的理论逃逸（§6）。

### 4.5 `forget()` 必须主动关闭传输资源（约束 4）

s5 已经证明：`sessions.remove(id)` **不足以**让 `LivePty` drop —— 卡在 `write_all` 的 writer
自己还持有 `Arc<LivePty>`。因此正式 cleanup 是**主动资源关闭**：

```rust
fn forget(&self, session_id: &str) -> Result<()> {
    let live = lock(&self.sessions)?.remove(session_id);   // 拿回 Arc（可能还有别的持有者）
    if let Some(live) = live {
        live.close_transport();                            // 主动关：与 Arc drop 无关
    }
    Ok(())
}

impl LivePty {
    /// 关闭 PTY 传输资源：**主动 take/drop master**。
    /// 不触碰 writer mutex —— 它可能正被一个 park 在 `write_all` 的线程持有，等它 = 把 8A 的
    /// 不变量从后门放回来。
    fn close_transport(&self) {
        if let Ok(mut master) = self.master.lock() {
            let _ = master.take();     // drop MasterPty → 关闭 pseudoconsole
        }
    }
}
```

结果（s5 第 3 档实测）：parked `write_all` → `Err(管道已结束 / os error 109)` → writer worker
返回并自然退出。**reaper 绝不 join writer**（8A §4.7 不变）。

### 4.6 Job handle 所有权与四条生命周期（约束 5）

- **单点所有权**：`Containment` 只在 `LiveProcess` 里，`Drop` 时 `CloseHandle`。
- **不 `DuplicateHandle`**；**不**把 job handle 给 reader/writer/worker 线程；含补扫在内的所有
  访问都只发生在 spawn / terminate_tree / forget / Drop。字段私有 + 无 `Clone` 即结构性保证。
- 生命周期语义统一：

| 路径          | 机制                          | 结果                                                     |
| ------------- | ----------------------------- | -------------------------------------------------------- |
| user kill     | `TerminateJobObject`          | 整棵树死；输入侧关闭（8A）；reaper 观测 root 退出 → 终态 |
| root 自然退出 | reaper 观测 → 终态 → `forget` | `forget` 关 job → 若还有 descendant，**一并被杀**        |
| 宿主正常关闭  | `host_shutdown` 收敛（现状）  | 宿主进程退出时 job 句柄关闭 → descendants 被 OS 清理     |
| 宿主异常死亡  | OS 关闭 process handles       | `KILL_ON_JOB_CLOSE` → descendants 被 OS 清理             |

> 注意第 2 行：**自然退出也要靠 `forget` 关 job**，否则「root 走了但后代还活着」会留下后台进程。
> 这也是 8A 的 reaper 顺序（终态 → forget）在本 Task 里继续是唯一释放点的原因。

## 5. 保证等级（约束 2 的措辞，必须原样写进 API 文档）

```text
Once containment is established, future descendants inherit the Job.
Pre-assignment descendants are reconciled best-effort (fixed-point sweep).
Race-free creation-time containment is deferred to a separate ADR/Task.
```

**禁止**在代码注释/文档里写 `spawn guarantees race-free ownership` 之类的说法。

## 6. 已知限制（诚实记录）

```text
L1  pre-assignment race：在 assign(root) 之前创建、且在补扫看到它之前又创建后代并退出的进程，
    理论上可以逃出 containment（spike s2 第一版实测过一次「只杀掉 root」）。
    8B 用「立即 assign + 定点补扫」把窗口压到最小，但**不是**数学意义上的 race-free。
L2  terminate_tree 不会让 parked writer 返回；返回靠 close_transport（s5 三档）。
L3  parked writer 线程可能持续存在到 master 关闭之后才返回；产品代码**不 join**（8A §4.7）。
L4  补扫依赖 Windows ToolHelp + OpenProcess 权限；无权限的 descendant（例如更高完整性级别）
    只能记录，不能强行纳入。
L5  非 Windows 平台本 Task 不实现 containment（保持现状语义），但 trait 形状已按
    「Session 拥有进程树」表达。
L6  **console 关闭本身也会带走挂在同一 ConPTY 上的后代**：所以「forget 之后没有偷活后代」
    这条端到端断言**无法区分** Job 路径是否生效（实现时用「取走 Job 句柄但 mem::forget 泄漏」
    的变异验证过：变异后断言照样通过）。Job 路径的判别性证据是
    `containment::tests::closing_the_last_handle_kills_the_contained_tree`（纯 Job，无 console）
    与 spike s6（宿主 `taskkill /F`）；产品测试的判别性断言是
    「forget 之后从句柄被 parked writer 持有的 `Arc` 里看 containment 已是 `None`」。
    `terminate_tree`（session-scoped kill）与宿主异常死亡仍然**只有** Job 一条路径能覆盖。
L7  headless 测试必须自带「终端模拟器」：conhost 因 `PSUEDOCONSOLE_INHERIT_CURSOR` 会发
    `ESC[6n` 并等应答，**不应答时 pseudoconsole 会卡在初始化**，连 `ClosePseudoConsole`
    都收不干净（表现为 master 已 drop、reader 却拿不到 EOF、parked 写永不返回）。
    生产里这一角色由前端 xterm.js 承担；测试夹具在
    `pty::portable_pty_backend` 的 `attach_test_terminal()`，且只应答一次（7D 教训：
    无限重放应答会淹掉子进程输入缓冲）。
```

## 7. 测试设计

### 7.1 确定性（fake backend，进 CI）

fake 需要新增（test-only）：

```rust
.with_failing_containment()                // spawn 时 containment 建立失败（带 os error 形状）
.without_containment()                     // spawn 成功但会话没有 containment（树杀必须报错）
.terminated_trees: Mutex<Vec<String>>     // 记录 terminate_tree 调用
.fail_next_terminate_tree(session_id)     // 让下一次 terminate_tree 失败
```

```text
T1  未建立 containment 不得进入 running：spawn 失败 → failed + launch_failed + 无 running；
    且错误信息里带具体原因（不得静默降级）
T2  kill 走的是 terminate_tree（不是 direct-child）：断言 fake 收到 terminate_tree；
    kill 成功后输入侧关闭（8A 语义不变）
T3  terminate_tree 失败时**不**关闭输入侧、也不写终态（沿用 8A T7 的形状）
T4  forget 会调用 backend.forget 且**先关输入侧**（8A 顺序不变）
T5  reaper 顺序不变：终态 → （有界等待 reader）→ shutdown_input → 移除 handle → backend.forget
T6  未建立 containment 时 terminate_tree 报明确错误（防御性路径，不静默）
```

### 7.2 真机（`#[cfg(windows)]`，本 Task 的 Gate）

新文件 `src-tauri/tests/process_tree_ownership.rs`（直接驱动生产 `PtyManager` +
`PortablePtyBackend`，与 8A 的真机测试同构）：

```text
R1  synthetic parent → child → grandchild（cmd /c cmd /c ping…；不足三层则调整到三层）：
    terminate_tree → 三个 PID 全部消失（job.active_processes → 0）
R2  真实 codex.cmd：树 = cmd.exe → node.exe → codex.exe（3 层），terminate_tree → 全消失
R3  真实 claude.cmd：树 = cmd.exe → claude.exe，terminate_tree → 全消失
R4  session-scoped（永久回归，最重要）：
    Codex A + Codex B + Claude C 同时运行 → kill A → A 树全死、B 树全活、C 全活
    （A/B 同名 exe 是这条的关键：错误的 executable-scoped 清理也能让「零残留」看起来成立）
R5  parked writer 全链路（有界观察窗口，不 join）：
    A 的 writer park 在 OS 写里 → terminate_tree(A) → 树全死 →
    reaper 观测退出 → 终态 → forget → close_transport →
    parked write 返回 Err（ERROR_BROKEN_PIPE/109），且 writer worker 退出（is_finished）
R6  kill-on-close：会话被 forget/释放之后仍有 descendant 时，descendants 被 OS 回收。
    判别性说明见 L6：console 关闭也会收掉挂在同一 console 上的后代，所以产品级测试里
    **同时**断言「显式取走 Job 句柄」这个机制事实；本 Task 实现时的变异验证记录在 plan Task 5。
R7  真实 Harness Hub 宿主内 assign 成功（约束 6 第一条）：断言在**生产 TerminalRuntime +
    PortablePtyBackend** 路径上 containment 建立成功；失败时必须暴露具体 Win32 error
```

R5 的观察窗口用硬期限（例如 5s）判定「write 最终返回」，**不得**用 join。

### 7.3 回归

```text
8A 全部（pty::input_tests 12 条 + runtime_input_backpressure 2 条）必须保持绿：
  trace 到本次改动的点：trait kill→terminate_tree 的改名、forget 的主动关闭、
  LivePty.master 变成 Option<…>。
7D terminal::concurrency_tests（7）+ two_harness_concurrency（3）保持绿。
claude_lifecycle（4）/ real_codex_terminal（2）/ real_claude_terminal 保持绿。
pnpm verify 退出码 0。
```

## 8. 验收 Gate

```text
✓ kill_terminal = 终止 Session 拥有的整棵树（synthetic + 真实 codex/claude 三层链）
✓ containment 在 running 之前建立；建立失败 → failed/launch_failed + 定向清理，无静默降级
✓ root assign 失败时错误信息含具体 Win32 error（约束 6）
✓ descendant 补扫是固定点循环 + 轮次上限，不靠 sleep
✓ forget 主动 close_transport（不依赖 Arc drop）；parked write 因此返回（s5 复现）
✓ Job handle 单点所有权、无 DuplicateHandle、无 worker/reader 持有（结构 + 测试）
✓ 四条生命周期（user kill / natural exit / graceful shutdown / hard crash）后代都不偷活
✓ session-scoped：Codex A + Codex B + Claude C → kill A 只杀 A（永久回归）
✓ 不改 Session 状态机语义：terminate_tree 只是控制动作（T1/T3 锁死）
✓ 保证等级措辞正确：不声称 race-free；ADR-0013 记录暂缓项
✓ 8A/7D/真机全回归绿；pnpm verify 退出码 0；无 Harness 特判、无新 IPC DTO、UI 无改动
```

## 9. 对其他文档的影响

- **ADR-0013**（新增）：记录「post-spawn 立即 assign + 定点补扫」的取舍与暂缓项。
- **ADR-0010**（Reader 与 Reaper 分离）：本 Task **不修改**其语义；reaper 仍是退出状态唯一事实来源，
  仍是唯一资源回收点（顺序：终态 → forget）。
- **Task 8A spec**：`forget` 的**主动关闭**是本 Task 对 8A §4.8 的必要补强（8A 当时假设
  「remove → drop → master 关闭」，s5 证明这个假设在 writer 持有 Arc 时不成立）。
- **CONTEXT.md / CONTEXT-MAP.md**：8B 关账后需要更新（`PtyBackend` 新增 `terminate_tree`、
  新增 `pty::containment`、Windows containment 语义），由 Task 8B 的最后一个 Task 负责。
