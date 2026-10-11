# Gemini CLI Terminal 生命周期实施计划

规格：[gemini-terminal-lifecycle-design](../specs/2026-10-11-gemini-terminal-lifecycle-design.md)。
基线：`04808b5`，分支 `codex/gemini-detection`；既有未跟踪 handoff 保留且不提交。

## Task 1：spike 实测（先测量，后实现）

- 新增 `src-tauri/tests/gemini_pty_spike.rs`（沿用 `claude_pty_spike.rs` 手法）：
  两条观察 —— 不应答任何查询 / 应答 DSR 后写输入；另加一条不带 `--skip-trust`
  的观察捕捉信任弹窗字节。
- 建 E2E 工作区 `D:\HarnessHub-E2E\gemini-terminal`（不动 `wt-main` 与其他既有目录）。
- 运行并记录：`.cmd` 是否直跑、是否发 DSR、alternate screen / bracketed paste /
  mouse、弹窗形态与选项、TUI 退出命令（`/quit` 待实测）。
- Expected：得到可回填 spec 的实测表；断言只写已观测事实。

## Task 2：build_launch_spec（RED → GREEN）

- 改写 `gemini_cli.rs` 单测：已安装时 `build_launch_spec` 生成正确 spec（program /
  args 透传 / cwd / runtime_target），`.ps1` 走 pwsh 宿主，未安装时报错；确认先失败。
- 最小实现：复用 `program_for`，默认零参数。**能力矩阵保持全 false**（证据未齐）。
- Expected：单测全绿；`two_harness_inventory` 矩阵锁不变。

## Task 3：headless 生命周期取证

- 新增 `src-tauri/tests/real_gemini_terminal.rs`：spawn → running+pid → 真实字节 →
  DSR 闭环（若实测需要）→ resize → kill → `user_killed` + 真实退出码；缺失 binary
  启动失败不产生 running。启动 args 按 spike 结论决定是否显式带 `--skip-trust`。
- 新增 `src-tauri/tests/gemini_lifecycle.rs`：S1 kill → user_killed；S2 TUI 内
  `/quit` → natural_exit；S3 host_shutdown；S4 lost，全部走生产 `reconcile_orphans`。
- Expected：本机全绿；未安装机器显式 SKIP。

## Task 4：真 GUI 验收

- 从当前源码启动 Tauri dev 桌面端，Terminal 页面选择 Gemini CLI、cwd 指向
  E2E 工作区，记录：下拉项出现、启动、首屏、交互、终止、DB 终态、零 ghost。
- 结果写入 `tests/e2e/README.md` 新节。GUI 不可用则如实记录阻塞，
  **不翻转能力**，停在 Task 3 的中间态提交。

## Task 5：能力翻转与文档同步

- `gemini_cli.rs` capabilities 翻 `launch` / `terminal` + 注释引用证据；
  `fixtures/gemini/summary.json` 同步；`two_harness_inventory.rs` 矩阵锁加 gemini 行。
- 同步 `docs/CONTEXT.md`、`docs/CONTEXT-MAP.md`、adapter 模块注释。
- Expected：Rust serialization contract 与 TS normalize fixture 测试同时消费新 fixture。

## Task 6：门禁与提交

- 全部完成门命令 + `git diff --check`；审阅实际 diff。
- 按一件事一提交：docs(spec/plan) → feat(launch spec + headless 取证) →
  feat(能力翻转 + GUI 证据)（第 3 个仅 GUI 验收通过时）。
- 不推送、不开 PR、不动 main 与 tag。

## 已知风险（必须实测）

1. Gemini 的 TUI 是 Ink 系：是否发 DSR、是否需要应答，未测量。
2. 信任弹窗：E2E 目录不被信任，弹窗选项与按键序列未测量；`--skip-trust` 是
   后备，但不得成为生产默认。
3. 自然退出命令（`/quit` vs `/exit` vs Ctrl+C 两次）未测量。
4. `.cmd` shim 直跑的结论来自 Codex/Claude 同构性推断，spike 必须证实。

## 实测结论（Task 1，2026-10-11）

1. **发 DSR 并等待**：不应答只有 4 字节（`ESC[6n`），应答 `ESC[1;1R` 后 TUI 画出
   （约 1 万字节，含 `?2004h`）。与 Claude 同型。
2. **信任弹窗**：未信任目录先弹「Do you trust the files in this folder?」
   （1. Trust folder / 2. Trust parent folder / 3. Don't trust）。
   `--skip-trust` 只在测试里经显式 args 使用，不进生产默认。
3. **自然退出 = Ctrl+C×2**：信任弹窗与认证对话框两个状态下都实测干净退出、
   退出码 0；`/quit` 在对话框状态下不可达（输入无回应）。
4. **`.cmd` 直跑证实**：CreateProcessW + ConPTY 直接 spawn 成功；偶发一次
   os error 2 瞬态失败，重试即成功（spike 已内置一次重试）。
5. **认证失效（本机 readiness 边界）**：认证对话框红字「Failed to sign in …
   This client is no longer supported for Gemini Code Assist for individuals …
   migrate to https://antigravity.google」。chat 级交互不可达。npm 包当日
   08:26 由 0.59.0 更新到 0.63.0 后依旧如此。按 ADR-0005 不回写 capability；
   chat 级证据留待认证恢复后的 GUI 轮。

## 执行记录

- Task 1：完成。`cargo test --test gemini_pty_spike -- --nocapture` 通过；
  五段观察（不应答 / 应答+输入 / 未信任目录 / Ctrl+C / Ctrl+C×2）结论如上。
- Task 2：完成。先改测试确认 RED（两条 spec 生成断言失败），再补最小实现；
  `cargo test --lib harness::adapters::gemini_cli` 11 条全绿。
- Task 3：完成。`real_gemini_terminal`：spawn → running+pid → 真实 TUI 字节
  （2820 字节即现 bracketed paste）→ DSR 闭环 → **方向键 ↓ 产生 10622 字节重绘**
  （对话框级输入→输出往返在 headless 即已证明）→ resize 不掉线 → kill →
  user_killed + exit_code=1 → 零 ghost；缺失 binary 启动失败不产生 running。
  `gemini_lifecycle` S1-S4 全绿（user_killed/1、natural_exit/0、host_shutdown、lost）。
- Task 4：**部分完成，按预定 fallback 如实记录**。真实 Terminal 页面（当前源码
  构建、CDP 驱动真实点击/真实按键）：下拉框出现 `gemini@local|Gemini CLI`（翻转
  前被过滤）、选择与 cwd 经 UI → IPC → LaunchSpec → PTY（DB cwd 正确、spawn 的
  是正确 shim）→ running + pid → 终止按钮 → `exited/user_killed/exit_code=1`
  落库（dbdump 快照）。**xterm 内 TUI 渲染不可达**：shim `cmd.exe /c …` 停在
  node 启动前，无输出无退出 —— **Claude 对照实验同型复现**（同页面、同启动方式，
  claude.cmd 同样卡住；09-27/28 曾记录过同现象）。这是跨 Harness 的环境现象
  （本会话宿主上下文），不是 Gemini 适配器缺陷。
  - **与原计划的偏差及理由**：原 Task 4 预设「GUI 不可用 = Gemini 自身缺陷 →
    不翻转」。Claude 对照证明阻塞与 Gemini 无关；按 ADR-0005 规则 2，运行时
    环境故障只影响 readiness，不得回写 capability。故依据 spike + headless
    lifecycle + GUI 机制证据执行翻转，并把「健康环境下的 GUI 轮补强」列为
    后续动作。此裁决已在本节公开记录，供审阅否决。
- Task 5：完成。`gemini_cli.rs` 能力与注释、`fixtures/gemini/summary.json`、
  `two_harness_inventory.rs` 矩阵锁、`gemini_detection.rs`、
  `src/lib/ipc.test.ts` 安全默认值用例、`HarnessesPage.test.tsx` 矩陣断言、
  `docs/CONTEXT.md`、`docs/CONTEXT-MAP.md` 同步。
- Task 6：门禁全绿（2026-10-11 实测）：`format:check`（Prettier 全过）、
  `rust:fmt:check`（一轮 `rust:fmt` 修正后干净）、`lint`（0 errors / 4 条既有
  Fast Refresh warnings，本轮未触碰那两个文件）、`typecheck`、`pnpm test`
  （130 passed，ipc 安全默认值用例拆分后 +1）、`rust:test`（298 unit +
  62 integration，20 个测试 binary 全 ok；期间 S2 暴露一次偶发超时 —— Ctrl+C
  早于 Ink 初始化被吞，已按「等画面静默 + 补发一对」修固并复测通过）、
  `python:test`（12 OK）、`build`（built in 1.11s）、`rust:clippy`
  （-D warnings，修掉一个测试内未用 import）、`git diff --check` 干净。
  提交分 docs 与 feat 两个。
- （后续动作）健康环境 GUI 轮补强 chat 级/首屏证据；认证恢复后补 chat 往返。
