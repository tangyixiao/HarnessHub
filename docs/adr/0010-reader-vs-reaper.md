# ADR-0010 — Reader 与 Reaper 分离，退出状态只有一个事实来源

- **Status**: Accepted（2026-09-22，Task 4）
- **Context**:
  最初的 `PtyManager` 只有一个读线程：读到 EOF 之后「顺便」重试取退出码，再上报终态。
  写编排层测试时立刻暴露出两个问题：

  1. **EOF ≠ 退出状态**。子进程 fork 出后台进程、或 shell 提前关闭 pty 时，
     输出流结束但进程仍在运行。把 EOF 当结束信号会让状态机说谎。
  2. 更糟的是**顺序竞态**（测试实测复现）：读线程可能在 `mark_running` 提交之前
     就观察到 EOF 并调用 `finish()`，而 `finish()` 只接受 `running` →
     迁移失败 → 会话**永久卡在 `created`**。

- **Decision**:

  1. **两条独立生命周期**：

     ```text
     reader 线程：输出字节 + EOF   —— 只表达「输出流结束了」
     reaper 线程：wait / try_wait  —— **唯一**的退出状态事实来源，唯一调用 ExitSink
     ```

     禁止由 EOF 推导退出码；reaper 拿不到状态时如实上报 `None`
     （上层映射为 `unknown`/`lost`），不猜。

  2. **顺序由调用方确定，不靠调度巧合**：

     ```text
     spawn（只取读端，不起线程）
       → mark_running(pid)（提交）
         → start_reading（此刻才起 reader + reaper）
     ```

     这样 EOF 在物理上不可能早于状态迁移。

  3. **`kill` 不得隐式丢弃会话句柄**：否则 reaper 之后读不到退出状态，
     终态永远写不下去。改为终态写入完成后由上层显式 `forget`。

  4. **kill 只表达意图，不写终态**：进程已结束则什么都不做；确认向仍存活的 child
     发出 terminate 后才 arm `user_killed`；kill 失败则撤销意图。
     残余竞态（`is_running` 与 `kill` 之间进程恰好自然退出）如实记录在代码注释里 ——
     没有 OS 级「谁先动手」证据时不可避免，但从未发出 terminate 时绝不误标。

- **Alternatives**:
  - 单线程 EOF 后重试（原实现）：实现简单，但把两个事实来源混在一起，且存在上述竞态。
  - 阻塞式 `wait()` 放在 spawn 调用方：会阻塞命令线程，无法同时处理 write/resize。
  - 用 `sleep` 或重试掩盖竞态：掩盖问题而不是消除问题，且在高并发下随机复现。
- **Consequences**:
  - 每个会话多一个常驻轮询线程（间隔 50ms）。对桌面应用可接受；
    未来可换成 `clone_killer` + 阻塞 `wait` 的专用 reaper 线程以去掉轮询。
  - `PtyBackend` 增加 `forget`，语义更明确：kill 与回收是两件事。
  - 上层（`TerminalRuntime`）成为唯一写终态的地方，状态机的合法性由 CAS 保证。
- **Evidence**: `src-tauri/src/pty/manager.rs` 的双线程实现与顺序注释；
  `terminal::tests::natural_exit_finishes_the_session_and_emits_exit_code`
  （修复前该测试失败：会话卡在 created）；
  `kill_on_a_running_session_is_recorded_as_user_killed` 与
  `kill_after_a_natural_exit_does_not_rewrite_the_reason`。
- **Revisit Conditions**: 接入可重连 Runtime（WSL/SSH/远程）时，
  reaper 可能需要在宿主重启后重新建立进程身份，届时重新评估轮询与事件驱动方案。
