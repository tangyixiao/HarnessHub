# 生命周期测试的同步边界

- 日期：2026-10-23
- 范围：发布验收中实际失败的 PTY 输入隔离与 OpenCode 自然退出测试；不改变生产运行时。
- 依据：ADR-0006、ADR-0009、ADR-0010。

## 问题与目标

PR #13 的 Linux CI `39954236167` 在 `a_blocked_write_in_one_session_never_freezes_the_runtime` 失败；Windows 发布候选完整测试在 OpenCode S2 的第二次 Ctrl+C 写入时报 `InvalidInput("未知会话")`。测试必须等待真正需要的条件，不得把输入已入队当作已写出，也不得要求已经结束的会话继续接受输入。

保持隔离测试对其他会话输入/resize/kill 的完整断言；保持自然退出测试对 session 终态、`natural_exit` 与 exit code 的真实断言。只修测试的等待和清理时序，不延长生产超时、不把错误吞成成功、不禁用测试。

## 验证

使用上述真实失败作为 RED 证据。分别定向复验修改目标，并在最终候选重新执行完整 `pnpm rust:test`，再次检查 Linux CI。若定位显示生产故障，必须重新界定方案，不以测试修正掩盖。
