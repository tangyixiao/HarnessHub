# ADR-0005 — Capability 与 Readiness 是两个维度，不得合并

- **Status**: Accepted（2026-09-21）
- **Context**:
  Harness Hub 需要向 UI 描述「某个 Harness 能做什么」。最初的实现把
  `HarnessCapabilities` 的每个布尔值当成「此刻能不能用」，并写下过一条测试：

  ```text
  capabilities.launch == 一次 launch() 调用是否成功
  ```

  这条契约是错的，并且会在 Task 4（PTY 启动）之后立刻反咬：

  ```text
  Codex adapter 已实现 launch        → capability 应为 true
  但这台机器 binary 被删 / auth 缺失 / profile 配坏 / runtime 不可用
                                     → 这一次 launch() 失败
  ```

  若按旧契约，一次环境问题就会把「Harness Hub 支持 launch」这个**静态事实**改写成
  「不支持」，UI 上的能力矩阵会随环境抖动，用户也无法区分
  「这功能没做」和「这功能做了但现在跑不起来」。

- **Decision**: 拆成两个独立维度，分别回答不同问题：

  | 维度 | 回答的问题 | 稳定性 | 现状 |
  | --- | --- | --- | --- |
  | `capabilities` | **Adapter 是否实现了**该能力 | 静态，随代码版本变化 | 已实现（本 ADR 明确其语义） |
  | `readiness` | **此刻**这台机器 / 这个 Profile / 这个 Session 能否使用 | 动态，随环境变化 | **尚未实现**（v0.1 不做） |

  ```ts
  capabilities: { launch: true, terminal: true, resume: false }
  readiness:    { launch: 'ready' | 'blocked' | 'unknown' }   // 未来形态
  ```

  规则：

  1. `capabilities.*` 为 `true` 的**唯一**依据是「该能力已实现并有测试/验收记录」。
  2. 运行时失败（binary 缺失、auth 过期、配置损坏、runtime 不可达）**只**影响 readiness，
     不得回写 `capabilities`。
  3. 未实现的能力必须为 `false`。UI 上显示 `—`，含义是**当前未支持**。
  4. 不要新增 `待验证` 这类状态：`待验证` 应保留给「实现已经存在，但验证证据不足」的情形，
     与「尚未实现」不是一回事。

- **Alternatives**:
  - 只保留 `capabilities`，用运行时探测结果动态计算：UI 会随环境抖动，且丢失「有没有实现」的信息。
  - 只保留 `readiness`：无法表达「这个 Harness 永远不会有 resume」，能力灰度的初衷丢失。
  - 合并成一个枚举（unsupported / experimental / supported）：规格 §5.1 曾考虑过，
    但它是「实现成熟度」分级，仍然回答不了「此刻能不能用」。
- **Consequences**:
  - UI 需要**同时**能看到两个维度才完整；v0.1 只有 `capabilities`，因此
    环境性问题目前只能通过 `installed` / `version` / `data paths` 间接表达。
  - Task 4 实现 launch 后，`capabilities.launch` 置 `true`；此时若 `launch()` 失败，
    必须表现为**执行错误 + readiness blocked**，而不是能力为 false。
  - 相关测试必须写成「实现状态」断言（例如「launch 仍是 stub，所以 capability 为 false」），
    不得写成「capability 等于某次调用的返回值」。
- **Evidence**: `src-tauri/src/harness/adapters/codex.rs` 的
  `launch_capability_is_false_while_launch_is_still_a_stub`；
  `HarnessAdapter::capabilities` 的文档注释。
- **Revisit Conditions**: 当 readiness 真正落地（Phase 2 之后的多 Harness + Profile 场景），
  评估是否需要把两个维度一起放进 `HarnessSummary` 并在 UI 上并列展示。
