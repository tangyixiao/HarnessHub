# Task 7D — Cross-Harness Concurrency（"Hub" 才真正成立）

> 7D 的重点**不是**再证明 Codex / Claude 各自能跑（7A–7C 已各自取证），而是证明它们
> **同时跑时仍然互不污染**：两个 process / reader / emitter / session / DB 行 / PTY 尺寸
> 都不共享状态。
>
> 版本语义：7D 完成后的 `v0.1.0-alpha.3` = **Multi-Harness Runtime**。

## 分层（避免一次真机测试承担太多责任）

```text
7D-A-i  确定性矩阵（fake PTY backend，进 CI，每次提交都跑）
7D-A-ii 真实 PTY 无头并发（真 codex + 真 claude，两个 cwd，本机没装则明确跳过）
7D-B    真实 GUI 双会话（真实 WebView2 + CDP；一个 GUI 会话 + 一个无头真实会话）
```

理由：GUI 轮慢、依赖桌面环境、且只能稳定驱动一个 active session；
隔离矩阵必须能作为**自动回归**反复执行，所以主体证据放在 A-i / A-ii。

## 7D-A-i — 确定性并发隔离矩阵（fake backend）

位置：`src/terminal/concurrency_tests.rs`（`#[cfg(test)] mod concurrency_tests;`，只随测试编译）。

两个安装：`codex@local` 与 `claude@local`（生产组合根 + 假宿主 probe），两个不同 cwd。
fake backend 必须能**按 session 精确记录** `(session_id, cols, rows)`（`spawned` / `resized`
已有），并**按程序分流输出**（否则「隔离」根本无从表达）。

断言：

```text
两个 session 同时 running，pid 不同，session_id 不同
DB：两行，installation_id / harness_id / cwd 各自正确且不同
输出隔离：codex emitter 只收到 codex 的 marker + session_id；claude 同理
          两侧 emitter 都不得出现对方的 marker / session_id
输入隔离：write(A) 只到达 A 的 PTY；B 的 written 记录为空
resize 隔离：resize(A, c, r) 只产生 (A, c, r)；B 的尺寸记录为空
kill 隔离（两个方向）：kill 一个 → 另一个仍 running，且仍能 write / resize / output
DB lifecycle 独立：两行各自的 termination_reason / exit_code / ended_at
                  互不覆盖，且与 kill 顺序无关
restart/ghost：两条 running → 生产 reconcile_orphans(Lost) → 两条都收敛、running = 0
              （证明收敛不是「只处理第一行」），二次收敛为 0（幂等）
```

## 7D-A-ii — 真实 PTY 无头并发（真机）

位置：`src-tauri/tests/two_harness_concurrency.rs`（`#![cfg(windows)]`，单文件内串行）。

```text
Codex  → D:\HarnessHub-E2E\codex-concurrent
Claude → D:\HarnessHub-E2E\claude-concurrent
```

覆盖：

1. 同时启动，两个真实 child 同时存活（DB running + `runtime.is_running` + `tasklist` 级独立观测）；
2. 两个 reader 各自产出真实 TUI 字节；DSR 由 test-only responder 分会话应答；
3. marker 隔离：向 codex 写 `CODEX_ONLY_31415`、向 claude 写 `CLAUDE_ONLY_27182`，
   断言「对方 stream 一定不含我的 marker」，并在能看到回显时锁死正向包含；
4. resize 隔离：resize codex 后 claude 仍 running / 仍产出；
5. kill 顺序两个方向：先 kill Claude → Codex 仍 running 且仍可 write/resize/output；
   先 kill Codex → Claude 仍健康；
6. DB lifecycle 独立：终态、退出码、`termination_reason` 各自落库；最后 running = 0；
7. 两条 running + 生产 `reconcile_orphans(Lost)` → 两条都收敛成 unknown/lost（真实进程）。

## 7D-B — 真实 GUI 双会话

不新增 multi-tab / split terminal 生产功能（7D 验证的是 Runtime 并发，不是 terminal
workspace manager）。形式：

```text
真实 pnpm tauri dev（WebView2 + CDP 9222，不置顶、不动鼠标键盘）
GUI 会话（真实 UI：Terminal Launcher → 选 Harness → cwd → 启动）
   +
同一时间由无头侧（生产 TerminalRuntime）跑着的真实会话
```

观测：两个真实 child 同时存在、GUI 会话仍 running、kill GUI 那个不影响另一方；
hard kill 应用 → 重启 → 真实应用库里的旧 running 收敛为 lost、running = 0。

## 最终 Gate

```text
✓ simultaneous Codex + Claude processes
✓ different cwd
✓ independent session_id / pid / installation
✓ output isolation        ✓ input isolation      ✓ resize isolation
✓ kill Claude leaves Codex alive      ✓ kill Codex leaves Claude alive
✓ independent DB lifecycle
✓ Dashboard distinguishes both harnesses（7C 已取证，7D 不重新设计 importer）
✓ restart → zero ghost running（两条旧 running 都要收敛）
✓ no harness-specific Core branch introduced（用 diff 证明，不靠审计）
```

## 明确**不**做

- 不新增 `ClaudeTerminalRuntime` / 任何 Harness 特判分支；
- 不重新设计 ccusage importer，不做 `hub_session_id` 猜关联（ADR-0011 决策八）；
- 不为取证新增 multi-tab / split terminal UI，不加任何生产 debug API；
- 不改产品行为；`FakeHostProbe` 去重与 `act()` 警告只做测试卫生。

## 执行顺序

```text
targeted tests → pnpm verify → git status → Core diff audit → commit → push
→ cleanup（测试卫生）→ v0.1.0-alpha.3
```
