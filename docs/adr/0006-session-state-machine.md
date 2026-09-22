# ADR-0006 — Session 状态机：没有进程就不得是 running

- **Status**: Accepted（2026-09-22）
- **Context**:
  Task 3 引入了 `create_session`：它登记一条会话记录，但**不启动任何进程**
  （PTY 属于 Task 4）。第一版实现把新会话直接写成 `status = 'running'`，
  于是数据层出现了「假 running」：

  ```text
  create_session → status = running → 实际没有 process / PTY
  ```

  这与项目的零宣称原则（ADR-0005）冲突：UI 会显示「仍在运行 · 尚未收到退出码」，
  而事实是**从来没有运行过**。等到 Task 4 才修，就要同时处理状态机、迁移与已有数据。

- **Decision**: 状态机显式区分「已登记」与「已运行」：

  ```text
  created ──mark_running──▶ running ──finish(exit=0)────▶ exited
     │                        └────finish(exit≠0)───▶ failed
     └──fail──▶ failed        └────finish(exit=None)─▶ unknown

  unknown 仅用于：拿不到退出码，或存量数据无法识别
  ```

  约束：

  1. `create_session` 只能产生 `created`；`running` **必须**由一次成功的 spawn 之后
     调用 `mark_running` 得到。
  2. `finish` 只接受 `running`。对 `created` 会话结束会返回 `false` ——
     结束一个从未运行的会话只会造出假历史。
  3. `fail` 接受 `created` / `running` → `failed`（启动失败）。
  4. `unknown` 不是兜底状态，不得用来掩盖「不知道该写什么」。
  5. 迁移 0002 把存量数据里无进程的 `running` 诚实地降级为 `created`。

- **Alternatives**:
  - 只在 UI 层区分（DB 仍写 running）：数据层依旧说谎，任何查询都会得到错误的「运行中」。
  - 用 `NULL` 表示未启动、`running` 表示已启动：状态字段变成可空后，所有查询都要处理三态。
  - 把 `created` 叫 `pending`：`pending` 语义偏「排队等待」，与「已登记但没人启动」不同。
- **Consequences**:
  - UI 需要新增一种状态显示：**已创建 · 尚未启动**（绝不能显示「仍在运行」）。
  - Task 4 的接线点是明确的：spawn 成功 → `mark_running`；spawn 失败 → `fail`；
    进程退出 → `finish(exit_code)`。
  - 旧数据在迁移中被改写（`running` → `created`）。这是安全的，因为 PTY 当时尚未实现，
    不可能存在真的在跑的会话。
- **Evidence**: `src-tauri/src/session/store.rs` 的状态机测试
  （`a_new_session_is_created_not_running`、`finish_is_rejected_for_never_started_sessions`、
  `full_lifecycle_created_running_exited`），以及迁移 0002 的数据搬运测试。
- **Revisit Conditions**: 引入「排队 / 调度」语义（多 Agent 并行编排）时，评估是否
  需要独立的 `queued` 状态，而不是复用 `created`。
