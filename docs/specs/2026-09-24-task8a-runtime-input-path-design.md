# Task 8A — Runtime 输入路径：跨会话锁下不得有阻塞 I/O（设计 / spec）

> **来源**：Task 7D 真机并发压测（见 `tests/e2e/README.md` 的「7D 发现」第 2 条）。
> **状态**：设计已 freeze（brainstorming gate 通过；backpressure 策略选 **A**，并已并入 5 项补充）。
> **路线**：Task 8B（process-tree ownership）排在 8A 之后；Configuration/Profile Plane 顺延为 Task 9。
> **本文件是 8A 实现的规格源**；实现计划见 `docs/plans/`，实现必须按 TDD。

## 1. 问题（实测事实，不是推断）

7D-A-ii 的第一版真机测试**静默挂死 15+ 分钟**：测试进程 CPU ≈ 0，两个 Harness 的 shim 都还活着
（说明 kill / reap 从未拿到锁）。触发条件是测试按「历史上出现过多少个 `ESC[6n`」**无限重放**
DSR 应答，把 Claude 的 ConPTY 输入缓冲区灌满，`write_all` 阻塞。

读代码得到的机制（观测 + 代码，无线程栈，按假设记录）：

```text
PortablePtyBackend::write
  let sessions = lock(&self.sessions)?;      // ← 全局 map 锁
  session.writer.write_all(bytes)?;          // ← 可能永久阻塞（子进程不读 stdin）
  session.writer.flush()?;
  // 直到这里才释放
```

于是**一条会话的阻塞写冻结了整台 Runtime**：`resize` / `kill` / `try_wait`（reaper）/ `is_running`
全部要拿同一把 `sessions` 锁 —— 连「结束会话」这个唯一的恢复手段也进不去。

现有 `write()` 的同步语义还意味着：调用方（Tauri IPC 命令）会被阻塞在 OS 写系统调用上。

## 2. 目标 / 非目标

**目标**

1. 冻结并实现不变量：**任何可能阻塞的 OS I/O 不得发生在跨会话锁持有期间**。
2. `write_terminal()` 不再阻塞在子进程 stdin 上：输入进入**每会话有界队列**，由专属 writer
   worker 消费；队列满时立即返回带诊断的错误（策略 A），**整批丢弃**。
3. 明确 async 化之后的新语义：`enqueue Ok ≠ OS 已写入 PTY`；worker 失败后输入侧明确不可用。
4. kill 成功后立即关闭该会话输入侧；`forget` / 写入竞争不得出现「forget 之后仍成功入队」。
5. 用**故意永不读 stdin 的 synthetic child** 做验收（确定性 fake + 真机各一套）。

**非目标（8A 不做）**

- 8B 的进程树所有权（`ProcessHandle::terminate_tree()`）、Job Object、异常退出自动清理。
- UI 显示背压/输入不可用状态（即之前讨论里被推迟的选项 C）。
- 无界缓冲、`PtyEvent` DTO 改动、任何 Harness 特判。
- 「回收被 OS syscall 卡死的 writer 线程」—— 见 §7 Known limitations。

## 3. 冻结不变量

```text
INV-1  任何可能阻塞的 OS I/O 不得发生在跨会话锁（sessions map）持有期间。
INV-2  write_terminal 不得阻塞在子进程 stdin 上（只做 O(1) 入队或立即报错）。
INV-3  kill / try_wait / resize / take_reader 不得依赖 writer 的锁。
INV-4  全局 sessions 锁只用于：查找 → clone Arc / 插入 / 删除；绝不嵌套 per-session 锁 + OS 调用。
```

统一访问模式（`portable_pty_backend.rs` 里**所有** backend 操作都必须长这样）：

```rust
let live = {
    let sessions = self.sessions.lock()?;
    sessions.get(session_id).cloned()          // Arc<LivePty>
};                                             // ← 全局锁到这里必须已经释放

live.writer.lock()   // 只有写入会 park 在这里
live.master.lock()   // resize / try_clone_reader
live.child.lock()    // kill / try_wait / is_running
```

**禁止**出现：

```text
global map lock → session-local mutex → OS call
```

## 4. 设计

### 4.1 `portable_pty_backend.rs`：每会话独立加锁

```rust
pub struct PortablePtyBackend {
    sessions: Mutex<HashMap<String, Arc<LivePty>>>,   // 只做查找/插入/删除
}

struct LivePty {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    child:  Mutex<Box<dyn Child + Send + Sync>>,
}
```

- `write` → clone Arc → `live.writer.lock()` → `write_all` + `flush`。**只有这一把锁会被 park。**
- `resize` → `live.master.lock()`。
- `kill` / `try_wait` / `is_running` → `live.child.lock()`。
- `take_reader` → `live.master.lock()` 调 `try_clone_reader()`（句柄克隆，不是数据面阻塞调用；
  若它真的阻塞，也只 park 该会话的 master 锁，kill 走 child 锁不受影响）。
- `forget` → 从 map 移除（`remove` 返回 `Arc<LivePty>`，drop 关闭 PTY）。

结果：A 的 writer 永久 park 只在 A 的 writer 锁上；**B 的 write/resize/kill 与 A 自己的
kill/try_wait/resize、以及 reaper 全部照常**。

### 4.2 `pty/session.rs`：`InputState` 与 worker 生命周期分离

**不要**让 worker 捕获 `Arc<SessionHandle>`（那会与 `SessionHandle` 持有的 `JoinHandle` 形成
自引用）。worker 只捕获三样东西：

```rust
pub(crate) const DEFAULT_INPUT_CAPACITY_BYTES: usize = 64 * 1024;

pub(crate) struct SessionHandle {
    input: Arc<InputState>,
    /// 只表达 detach 语义：drop 它 = detach。**任何路径都不 join**（见 §4.7）。
    /// 「确实没有 join」由 T9 的硬期限用例证明：若哪条路径 join 了正 park 在 OS 写里的 worker，
    /// `forget` 会一直不返回，用例在 2 秒后失败。
    _worker: JoinHandle<()>,
}

pub(crate) struct InputState {
    session_id: String,
    capacity_bytes: usize,
    queue: Mutex<InputQueue>,
    ready: Condvar,
}

struct InputQueue {
    batches: VecDeque<Vec<u8>>,   // 保持批次边界（FIFO）
    pending_bytes: usize,         // 见 §4.3
    closed: bool,                 // 输入侧已关闭（forget / kill 成功 / 会话结束）
    failure: Option<String>,      // worker 遇到真实 backend.write 错误
}

// worker 线程：
//   fn run(state: Arc<InputState>, backend: Arc<dyn PtyBackend>, session_id: String)
String + Arc<InputState> + Arc<dyn PtyBackend>
```

接口：

```rust
impl SessionHandle {
    pub(crate) fn spawn(session_id: &str, backend: Arc<dyn PtyBackend>, capacity_bytes: usize) -> Self;
    pub(crate) fn try_enqueue(&self, bytes: &[u8]) -> Result<()>;
    pub(crate) fn shutdown_input(&self);           // 见 §4.7
    pub(crate) fn pending_bytes(&self) -> usize;   // 诊断 / 测试（真机验收也用它）
}
```

可观测契约就是**三个错误变体**（§4.9）：`InputBackpressure` / `InputClosed` /
`InputWorkerFailed`。**不**额外暴露 `closed` / `failure` 谓词 —— 没有第二个谓词，就没有
「调用方只查了 `closed`、漏掉 worker 失败态」这种坑。`pending_bytes()` 不是谓词，是记账读数。

### 4.3 记账：`pending_bytes` 必须包含 in-flight 批次

**这是 7D 讨论里补的第 1 点，也是「有界」能不能成立的关键。**

```text
pending_bytes = 队列中尚未 pop 的字节 + 正在 backend.write 里的那一批
```

- enqueue 时**增加** `pending_bytes`；
- 只有 `backend.write()` **返回之后**才减少（成功）或清零（失败/关闭）。

准入判定（整批接受或整批拒绝，**禁止 partial enqueue**）：

```text
pending_bytes + attempted_bytes <= capacity_bytes
  → 整批接受（pending_bytes += attempted_bytes）

否则
  → 整批拒绝 → Error::InputBackpressure { session_id, pending_bytes, capacity_bytes, attempted_bytes }
```

推论（必须由测试锁死）：单批本身 `> capacity_bytes` 时，**即使队列是空的也直接拒绝**（此时
`pending_bytes == 0`）。也正因为 in-flight 计入，worker pop 之后队列无法被重新填满 —— 不会出现
「64 KiB 队列实际允许 ~128 KiB outstanding」的漏洞。

### 4.4 异步 write 的失败语义

同步时代的 `backend.write` 错误能直接返回调用方；worker 化之后第一次调用已经返回 `Ok` 了。
必须把语义写清楚：

```text
try_enqueue → Ok   = “Harness Hub 已接受本次输入”
                   ≠ “OS 已经完整写入 PTY”
```

worker 遇到真实的 `backend.write` error：

```text
1. failure = Some(error.to_string())
2. 丢弃尚未发送的队列（batches.clear()；此时无 in-flight，pending_bytes = 0）
3. 退出 worker 线程（不再尝试）
4. 之后所有 try_enqueue → Error::InputWorkerFailed { session_id, detail }
5. kill / resize / try_wait / reaper 继续正常工作
6. **不写 Session 终态**（进程是否退出只有 reaper 是事实来源）
7. **不新增 PtyEvent**
```

### 4.5 kill 成功 → 关闭输入侧

```text
PtyManager::kill(session_id)
  → backend.kill(session_id)?
       Ok  → handle.shutdown_input()      // 之后 write_terminal → InputClosed
       Err → 不动输入侧（不擅自关闭，也不假装成功）
```

理由：`kill` 之后到 reaper 收敛之间，会话已经「不接受了」，此时还能继续 enqueue 语义很怪。
自然退出不经 kill：由 reaper → `forget` → `shutdown_input`（见 §4.8）。

### 4.6 `forget` / `write` 竞争

`write` 已经 clone 到 `Arc<SessionHandle>`、随后 `forget` 关闭它时，`try_enqueue()` **必须**看到
`closed` 并返回 `InputClosed`，不能在已经 forget 的会话后面偷偷入队。

实现要求：`closed` 与队列在同一把 `InputQueue` 锁下读写；`forget` 先 `shutdown_input()` 再从 map
移除（顺序固定），`try_enqueue` 入口先检查 `failure`/`closed` 再做容量判定。

### 4.7 `shutdown_input()` 语义（禁止 join）

```text
1. closed = true
2. 丢弃尚未 pop 的输入（batches.clear()）
3. 修正 pending_bytes = 0（会话已关闭，不再接受输入；此字段诊断意义到此为止）
4. notify_all（唤醒 park 在 Condvar 上的 worker，让它走 closed 分支退出）
5. **不 join**
```

Rust 里 drop `JoinHandle` 就是 detach。**明确禁止任何 Drop / shutdown 路径 join worker** ——
worker 可能正 park 在阻塞的 OS 写里，join 会让 teardown 又冻一次（等于把 8A 的不变量从后门放回去）。
在 `session.rs` 顶部用注释写死这条，并在测试里用硬超时锁住「shutdown 立即返回」。

### 4.8 会话终态后的资源释放（**8A 的必要新增，请 review 时特别看这条**）

现状：`PtyBackend::forget` 有定义、有实现，但**全仓没有一个生产调用点**（`grep forget` 只有
trait / impl / fake）。也就是 `PortablePtyBackend::sessions` 里的 `LivePty` 在会话结束后永久留在
map 里（PTY master 一直不关）。8A 之后每个会话还会多一个 `SessionHandle` + 一个 worker 线程，
不释放就是「每个结束的会话泄漏一个线程 + 一个队列」。

因此 8A 必须补上释放路径，放在**唯一知道进程真的消失的地方** —— reaper：

```text
reaper 拿到退出状态 → on_exit(session, code)   // 现有：写 Session 终态 + 发 Exited
                    → backend.forget(session)
                    → sessions.remove(session) → shutdown_input()
```

- `PtyManager::forget(session_id)` 仍然公开（手动/兜底路径），语义不变。
- reaper 需要能访问 `sessions` map：`sessions: Arc<Mutex<HashMap<String, Arc<SessionHandle>>>>`，
  reaper 闭包 clone 一份 Arc。
- 附带好处（**不是承诺**）：drop `LivePty` 会关闭 PTY master，可能让 park 在写里的 worker
  拿到错误并退出。§7 仍然按「可能长期不返回」记录。

### 4.9 错误类型（`error.rs`）

IPC 最终仍是字符串，`IpcResult` 形状不变 → **不新增跨 IPC DTO，不需要新的序列化契约测试**，
只加错误文案/变体测试。

```rust
#[error("输入被丢弃：会话 {session_id} 的待写输入 {pending_bytes} 字节已达上限 {capacity_bytes}（本次 {attempted_bytes} 字节整批拒绝）")]
InputBackpressure { session_id: String, pending_bytes: usize, capacity_bytes: usize, attempted_bytes: usize },

#[error("输入不可用：会话 {session_id} 的输入侧已关闭")]
InputClosed { session_id: String },

#[error("输入不可用：会话 {session_id} 的写入线程失败：{detail}")]
InputWorkerFailed { session_id: String, detail: String },
```

诊断字段按 review 意见补全：用户贴 5 MiB 时，一眼就能区分「队列本来就满」还是「这一批自己超上限」。

## 5. 行为变化清单（谁会看到什么不同）

| 变化 | 影响 |
| --- | --- |
| `write_terminal` 变成入队 | 返回 `Ok` 不再代表已写入 PTY；紧接着 kill 时排队输入可能永远不发出 |
| 队列满 / 输入关闭 / worker 失败 | 返回新的三个错误变体（前端仍只看到字符串，UI 行为不变） |
| `backend.written` 不再同步可见 | 现有 `pty::manager` 单测必须等 worker 排空后再断言（合法修改，不是放宽） |
| 会话退出后释放 PTY 句柄与输入侧 | 修掉「forget 从没被调用」的既有泄漏；`live` 句柄不再跨会话累积 |
| 每会话多一个线程 | 线程数 = 会话数 × 3（reader / reaper / writer worker），会话结束即回收 |

## 6. 测试设计

生产默认容量 64 KiB；**容量必须可注入**（`PtyManager::with_input_capacity`），单元测试用 8 / 16
字节，禁止到处硬编码 `64 * 1024`。

### 6.1 确定性（`pty/manager.rs` 单测 + fake 扩展）

fake 需要新增（都在 `#[cfg(test)] mod fake` 内）：

```rust
.with_blocking_write(program)        // 该 program 的 write 会 park 在 Condvar 上
.wait_for_write_blocked(session, t)  // 等 worker 真的进入阻塞写（确定性，不靠 sleep）
.release_blocked_write()             // 放行
.with_failing_write(program, msg)    // write 直接返回 Err
```

用例：

```text
T1  a_blocked_write_in_one_session_never_freezes_the_runtime
    A 的 worker park 在阻塞写；容量=8
      → A 再 enqueue → InputBackpressure（不是挂死）
      → B 的 write / resize / kill 全部成功
      → A 的 resize / kill / try_wait 全部成功；reaper 仍能 reap
      → 全程硬超时（每次调用 < 2s）+ 看门狗
T2  pending_bytes_counts_the_in_flight_batch        ← 补的第 1 点
    容量=16：enqueue 8（worker 阻塞）→ pending=8；enqueue 8 → 接受（pending=16）；
    enqueue 1 → 拒绝（pending=16, attempted=1）；release → 排空后 pending=0
T3  a_batch_larger_than_capacity_is_rejected_even_when_empty
    容量=8，enqueue 9 → InputBackpressure{pending_bytes:0, capacity:8, attempted:9}
T4  no_partial_enqueue：被拒批次不得留下任何字节；放行后 backend.written 只含被接受的批次且顺序正确
T5  fifo_order_is_preserved
T6  worker_failure_closes_input_and_discards_queued_bytes   ← 补的第 2 条
    with_failing_write(A)：enqueue A（Ok）→ 等 failure → 下一次 enqueue → InputWorkerFailed；
    pending_bytes=0；A 的 kill/resize/try_wait 仍成功；**DB 无终态变化、无 Exited 事件**
T7  input_is_closed_after_a_successful_kill_but_not_after_a_failed_one
    kill 成功 → enqueue → InputClosed；backend.kill 返回 Err → enqueue 仍成功
T8  enqueue_after_forget_is_rejected                ← 补的第 3 条（竞争）
    forget(A) → enqueue → InputClosed（不得静默入队）
T9  shutdown_discards_queued_batches_and_never_joins
    worker park 在阻塞写时 shutdown_input() 必须在硬期限内返回（禁止 join 的回归锁）
T10 a_blocked_writer_does_not_freeze_a_second_session（同 T1 但走 resize/kill 之外再加写，
    明确证明 B 的输入通道可用）
```

### 6.2 真机 synthetic child（`#[cfg(windows)]`，8A 的验收核心）

新文件 `src-tauri/tests/runtime_input_backpressure.rs`，直接驱动生产 `PtyManager` +
`PortablePtyBackend` + 一个 synthetic `LaunchSpec`（**不经过 Codex/Claude**）：

```text
synthetic child = 永不读 stdin 的程序（首发候选：cmd /c "ping -n 120 127.0.0.1 >nul"；
                  备选：pwsh -NoProfile -NonInteractive -Command "Start-Sleep -Seconds 120"）
测试本身自证：若 child 其实会读 stdin，队列会被排空、背压不会出现 → 用例直接失败
```

```text
R1  A 塞满容量 → 下一次 enqueue 返回 InputBackpressure（in-flight 计入，绝不挂死）
R2  同一时刻（A 的 writer 真 park 在 OS write 里）：
      B 的 write ✓ resize ✓ kill ✓
      A 的 resize ✓ kill ✓ try_wait ✓
    每次调用都在硬期限内返回，并打印实测毫秒数作为证据
R3  A kill 之后 enqueue → InputClosed；进程树清理干净（8A 用 taskkill /T，8B 换成 terminate_tree）
```

### 6.3 回归

```text
7D 的 terminal::concurrency_tests（7 条）必须保持全绿（它们现在走新的队列路径）
7D 的 two_harness_concurrency（3 条真机）必须保持全绿
其余真机套件（claude_lifecycle / real_claude_terminal / real_* ）不得退化
pnpm verify 退出码 0
```

## 7. Known limitations（明确留给 8B，8A 不解决）

```text
如果 A 的 writer 已经真的 park 在 OS write 内部，而此时只 kill 掉直接的 .cmd shim，
descendant node.exe 仍持有 PTY —— 那么这条专属 writer 线程可能暂时甚至长期不返回。
```

8A 的承诺是：

> **即使 A 的 writer thread 永久 park，整个 Runtime、B 会话、以及 A 自己的 kill/resize/reap
> 都不会因此冻结。**

**不是**：

> 8A 一定能够回收这条被 OS syscall 卡死的线程。

后者由 8B 的 process-tree ownership 解决（Job Object / 受控 tree termination）。把这条写进 spec
是为了避免实现过程中为了「线程必须立刻退出」把 Windows cancellation 硬拉进 8A。

另外两条 8A **不处理**的既有边界（如实记录，避免被当成新引入的问题）：

```text
L1  spawn 成功但 mark_running 失败（或 start_reading 失败）时，handle 与 pending_reader 会留下。
    这与现状同源（现有 pending_readers 也是同样处理），8A 只保证 spawn 失败不插入 handle。
L2  reaper 释放资源依赖退出状态可被观测；若 try_wait 永久返回 None 且进程其实已死（平台异常），
    资源会留到进程结束。当前 Windows 实测未出现，8B 的 containment 会顺带收紧这一块。
```

## 8. 验收 Gate

```text
✓ INV-1..INV-4：backend 每个操作都是「先 clone Arc → 放全局锁 → 再锁 per-session」
✓ A 的 write 永久 park 时：B 的 write/resize/kill、A 的 resize/kill/try_wait、reaper 全部成功
✓ pending_bytes 包含 in-flight（容量无法被「pop 后重填」绕过）
✓ 整批接受/拒绝，无 partial enqueue；单批 > capacity 时即使空队列也拒绝
✓ worker 失败 → 输入侧明确不可用 + 丢弃未发送 + 不写 Session 终态 + 不加 PtyEvent
✓ kill 成功关闭输入侧；kill 失败不关；forget 之后写入被拒
✓ shutdown 不 join（Drop 路径禁止 join，硬超时锁住）
✓ 会话退出后释放输入侧与 PTY 句柄（reaper 负责；修掉 forget 从未被调用的泄漏）
✓ 真机 synthetic non-reader：同一结论 + 实测毫秒证据 + 零残留进程
✓ 7D 全部回归绿；pnpm verify 退出码 0
✓ 无 Harness 特判、无新 IPC DTO、UI 无新状态
```

## 9. 需要 review 时特别确认的两处

1. **§4.8 资源释放**：这是原设计之外的必要新增（现状 `forget` 没有任何生产调用点）。
   如果你认为应该单独成 Task，我会把它拆出去 —— 但不做的话 8A 会引入「每会话一个泄漏线程」。
2. **§4.9 错误变体命名**：把「已关闭」与「worker 失败」拆成 `InputClosed` /
   `InputWorkerFailed` 两个变体（而不是一个 `InputUnavailable` + 字符串），以便测试按变体断言。
