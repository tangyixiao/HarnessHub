# 生命周期测试同步修复计划

- 规格：`docs/specs/2026-10-23-lifecycle-test-races-design.md`
- 日期：2026-10-23

## Task 1：输入隔离测试

提取 Linux CI `39954236167` 的精确断言与上下文，跟踪 fake backend 与输入 worker 的同步点。保留隔离语义，按真实状态等待，定向测试并重复验证。

## Task 2：OpenCode 自然退出

真实 RED：`s2_opencode_can_exit_naturally` 在第二次 Ctrl+C 写入时报 `InvalidInput("未知会话")`。处理进程在两次输入之间已自然结束的合法状态；最终退出原因与退出码断言必须保留。定向复验后重跑完整 Rust 测试。

## Task 3：独立 PR 与发布接续

两项同类测试同步修复形成独立 PR，先合入插件宿主分支，Linux CI 通过后再完成主 PR 与发布分支的集成。记录实际 RED/GREEN 和完整门输出。

## 证据

### Task 1

CI 精确失败位于 `src/pty/input_tests.rs:305`，kill 后写入只接受 `InputClosed`。
fake backend kill 发布退出码后，reaper 可以先移除 handle，再让 write 返回合法的「未知会话」。
现有规格 §4.4.1 和同文件 T8 已记录该语义。修正为仍要求输入被拒绝；如已回收，则有界等待 `backend.forgotten` 并断言 handle 已移除。保留跨会话隔离和控制面 2 秒硬期限。

定向 `a_blocked_write_in_one_session_never_freezes_the_runtime` 首次及后续 3 次均 `1 passed; 0 failed`；未修改生产代码。

### Task 2

先依据数据库终态确认已退出的输入竞态，仍 running 则保持失败。最终断言仍要求 `natural_exit`、退出码存在、`running=0`。

定向 S2 连续 3 次 `1 passed; 0 failed`（49.29s / 32.42s / 31.80s），均实际观察到 `exit_code=Some(0)`。完整 Rust 门与 Linux CI 待再验。
