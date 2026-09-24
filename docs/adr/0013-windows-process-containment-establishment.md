# ADR-0013 — Windows 进程包含（containment）的建立方式：post-spawn 立即 assign + 定点补扫；creation-time containment 暂缓

- **Status**: Accepted（2026-09-24，Task 8B spike 结论）
- **Context**:
  Task 7D 实测到「杀掉 `.cmd` shim 之后 `node.exe` / `codex.exe` 仍存活」：Harness Hub 以前只终止
  **直接子进程**，而 `.cmd` 的“直接子进程”其实是 `cmd.exe`（shim 解释器），真正的 Harness 进程
  （node / codex.exe / claude.exe）是它的后代。

  Task 8B spike（throwaway，仓库外）在真机上比较了 Windows Job Object 与主动枚举/树杀：

  ```text
  真实 codex 树 = cmd.exe → node.exe → codex.exe（3 个进程）
  真实 claude 树 = cmd.exe → claude.exe（2 个进程）
  create job(KILL_ON_JOB_CLOSE) → spawn → assign(root) → job.active_processes == 树节点数
  TerminateJobObject → 全部 dead（3/3、2/2）
  taskkill /F 宿主 → 它拥有的 3 个进程全部被 OS 自动回收（KILL_ON_JOB_CLOSE）
  ```

  但同一组实验暴露了一个**无法用当前 spawn API 消除**的窗口：

  ```text
  spike s2 第一版：先 spawn B/C、再 assign A
    → A 的 descendants 不在 job 里 → TerminateJobObject 只杀掉 A 的 root，后代存活
  把 assign 提到紧跟 spawn 之后：5/5 全部纳入（s7）
  ```

  原因：**job 成员资格只被「加入之后创建」的子进程继承**。要彻底消除该窗口，必须让进程
  **出生即在 job 内**或**出生即暂停**：

  ```text
  CREATE_SUSPENDED → AssignProcessToJobObject → ResumeThread
  或  PROC_THREAD_ATTRIBUTE_JOB_LIST（Win10+）
  ```

  而 `portable-pty 0.9` 的 `spawn_command` 两者都不暴露 —— 要走到 creation-time，就得自己
  接管 Windows 的 ConPTY + CreateProcess 路径。

- **Decision**:

  1. **8B 采用**：`create job(KILL_ON_JOB_CLOSE)` → spawn PTY child → **立即**
     `AssignProcessToJobObject(root)` → **定点补扫**（enumerate descendants → assign 尚未在 job 里的
     → 再 enumerate，直到本轮无新增），然后才允许把 Session 宣称为 `running`。
  2. **保证等级如实写进 API 文档**，禁止声称 race-free：

     ```text
     Once containment is established, future descendants inherit the Job.
     Pre-assignment descendants are reconciled best-effort (fixed-point sweep).
     Race-free creation-time containment is deferred to a separate ADR/Task.
     ```

  3. **root assignment 失败 = 启动失败**（`failed` / `launch_failed` + best-effort 定向清理），
     **不得**静默降级成「只杀直接子进程」。
  4. **补扫是 best-effort**：它能极大缩小窗口，但不能消灭「未被 assign 的后代在扫描前又创建
     后代并退出」的理论逃逸；这一点写进 8B spec 的已知限制。
  5. **creation-time containment 暂缓**：自己接管 spawn 属于 spawn subsystem replacement，
     范围远超 Runtime hardening，单独立 Task/ADR 评估，不纳入 8B。

- **Alternatives**:
  - **自己接管 Windows spawn**（`CREATE_SUSPENDED` 或 `PROC_THREAD_ATTRIBUTE_JOB_LIST`）：
    能真正做到 race-free，但要把 ConPTY 建立、环境块、`CommandBuilder` 语义、`.ps1` 宿主等
    全部重写；在 alpha 阶段收益不抵风险。
  - **只 assign root、不补扫**：实现更小，但 8B 的核心症状（杀 shim 留 node）在最坏时序下仍会
    复现，等于没修。
  - **继续用主动枚举树杀**（不引入 Job Object）：这正是要修的「临死前枚举」模式；而且宿主
    异常死亡时没有任何自动回收。
  - **无限补扫直到收敛**：异常进程树（持续 fork）可能不收敛，必须加轮次上限。

- **Consequences**:
  - 每个 Session 多一个 OS 资源（Job handle），所有权集中在 `LiveProcess.containment`，
    **不** `DuplicateHandle`、**不**让 reader/writer 线程持有；最后一句柄关闭即 kill
    （`KILL_ON_JOB_CLOSE`），因此 `user kill` / `natural exit` / `graceful shutdown` /
    `hard crash` 四条路径的后代回收语义统一。
  - Windows 上需要 ToolHelp 快照 + `OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE)` +
    `IsProcessInJob` 来实现补扫；非 Windows 平台将来用 process group / session 实现同一语义。
  - Job Object 细节**不**泄漏到 `TerminalRuntime`：对外只有 `ProcessControl { terminate_tree(),
try_wait(), pid() }` 语义。
  - 已知限制必须与实现同处一地：`docs/specs/2026-09-24-task8b-process-tree-ownership-design.md`。
- **Evidence**: 8B spike（`D:\HarnessHub-E2E\spike-8b\`，throwaway）场景 s1/s2/s3/s4/s6/s7 的实测输出；
  产品级 Gate 见 8B spec §9（真实 Harness Hub 宿主内的 assign 成功 + `Codex A + Codex B + Claude C`
  的 session-scoped 树杀）。
- **Revisit Conditions**:
  - 需要**严格**无竞争的 ownership（例如要支持会快速 fork/exec 的 Harness）；
  - `portable-pty`（或后继 PTY 层）暴露 creation-time job 能力；
  - 接入非 ConPTY / 远程（WSL、SSH、Container）spawn 路径时，containment 语义需要重新定义。
