# Gemini CLI Terminal 生命周期（launch / terminal）

## 目的与范围

接 Gemini 检测阶段（[gemini-detection](2026-10-01-gemini-detection-design.md)）之后，
按 Claude Task 7B 的路径交付 Gemini 的启动与终端：`build_launch_spec` → 现有
`TerminalRuntime` / PTY → 真实 Gemini TUI → 会话状态机收敛。

**做**：`build_launch_spec`、真实 PTY 启动、headless 生命周期取证、真 GUI 验收、
证据到位后翻转 `launch` / `terminal` 两个能力。

**不做**：Usage（ccusage 不覆盖 gemini，数据源另立设计）、`resume` / `replay` /
`tool_calls` / `subagents` / `live_state` / `worktree`（保持 false）、配置投影。

## 本机事实（2026-10-11 取证）

```text
PATH:      D:\npm-global\gemini.cmd（另有同目录无扩展名 POSIX shim）
--version: 当日 0.59.0 → 08:26 更新后 0.63.0（npm 全局包当日被更新过一次）
数据目录   %USERPROFILE%\.gemini\（oauth_creds.json 存在但被服务端拒绝）
信任配置   {"c:/users/tangy": "TRUST_PARENT"} —— D:\ 下的目录不在信任范围
```

PTY 内实测（`gemini_pty_spike.rs`，全部为本机真实字节）：

```text
DSR        发送 ESC[6n 并等待；不应答只有 4 字节（与 Claude 同型）
TUI        无 alternate screen；bracketed paste 开启；mouse 关闭；
           kitty keyboard（?u）与 XTVERSION（>q）查询一并出现，PTY 原样透传
信任弹窗   未信任目录先弹「Do you trust the files in this folder?」三选一；
           --skip-trust（会话级）可跳过
认证状态   认证对话框显示红字：Failed to sign in … This client is no longer
           supported for Gemini Code Assist for individuals … migrate to
           https://antigravity.google —— 本机 chat 级交互不可达
自然退出   两个对话框状态下 Ctrl+C×2 都干净退出，exit_code = 0
瞬态       CreateProcess 偶发一次 os error 2，重试即成功（spike 已内置一次重试）
```

按 ADR-0005：认证失效属 **readiness**（v0.1 尚未实现该维度），只影响「此刻能否
用」，不得回写 capability。因此本任务的能力证据停留在「Hub 终端机制对 Gemini
真实 TUI 全链路工作」，chat 级往返证据在认证恢复后由后续 GUI 轮补充。

## 设计

- `gemini_cli.rs` 补 `build_launch_spec`：复用 `program_for`（`.cmd` 直跑、`.ps1` →
  pwsh 宿主），cwd 透传，**默认不注入任何参数**。
- **不绕过工作区信任**：适配器不默认带 `--skip-trust`。用户在 Hub 终端里看到的
  信任弹窗与裸终端一致，由用户在 xterm 里自行应答（这是 Gemini 自己的安全语义，
  Hub 不替用户做策略决定）。测试通过显式 `args` 或一次性交互应答解决。
- Core 零改动：`TerminalRuntime::start` 已是 Harness 无关（installation → adapter →
  `LaunchSpec` → PTY），前端过滤纯按能力 DTO，能力翻转后 Gemini 自动进入启动选择器。
- 能力翻转必须同时改：`fixtures/gemini/summary.json`（Rust/TS 共享契约）、
  `two_harness_inventory.rs` 矩阵锁、`gemini_cli.rs` 单测。

## 验收（能力翻转的证据门）

1. **spike**（`gemini_pty_spike.rs`）：真实 `gemini.cmd` 进真实 PTY；实测并记录
   DSR、alternate screen、bracketed paste、信任弹窗、Ctrl+C×2 自然退出；
   `.cmd` 可直接作为 program。
2. **headless 生命周期**（`real_gemini_terminal.rs`）：spawn → `running` + 真实
   pid → 真实 TUI 字节 → DSR 闭环 → resize 不掉线 → user kill → `user_killed` +
   真实退出码；binary 缺失时启动失败绝不假装 running。
3. **四条状态链**（`gemini_lifecycle.rs`）：user_killed / natural_exit（Ctrl+C×2）/
   host_shutdown / lost 各自经生产收敛函数 `reconcile_orphans` 落库，零 ghost。
4. **真 GUI 验收**：真实 Terminal 页面选择 Gemini CLI → 启动 → xterm 出现真实
   TUI 首屏 → 对话框级交互（方向键移动单选项，可见字节回应）→ 终态，记录进
   `tests/e2e/README.md`。chat 级交互因认证失效不可达，如实记录为 readiness 边界。
5. 能力矩阵：`gemini.launch` / `gemini.terminal` = true 且其余 false，由矩阵锁
   测试锁死；fixture 契约测试（Rust serialization + TS normalize）同步。
6. 完成 AGENTS.md 全部门禁并记录真实结果。

## 来源与限制

- [CLI 参考](https://geminicli.com/docs/cli/cli-reference/)：默认交互模式；
  `--skip-trust` 为会话级信任。
- 2026-10-11 本机 `gemini.cmd --version` 输出 `0.59.0`；当日 08:26 全局包更新到
  0.63.0 后认证仍被服务端拒绝。
- 信任弹窗只在本会话或经用户显式应答后消失；Hub 不修改 `trustedFolders.json`。
- **GUI 阻塞记录**（2026-10-11）：本会话宿主环境下，GUI 内所有 Harness 的
  shim 进程都停在 node 启动前（Claude 对照同型，见 tests/e2e/README.md），
  xterm 无法渲染 TUI。headless 真机证据不受影响；健康环境的 GUI 轮补强
  首屏/chat 证据是后续动作。
- Usage 与 GUI 自动化脚本入库不在本任务范围。
