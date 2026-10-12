# OpenCode CLI Terminal 生命周期（launch / terminal）

## 目的与范围

接 OpenCode 检测阶段（[opencode-detection](2026-10-12-opencode-detection-design.md)）之后，
沿用 Gemini 终端生命周期的路径交付 OpenCode 的启动与终端：`build_launch_spec` → 现有
`TerminalRuntime` / PTY → 真实 OpenCode TUI → 会话状态机收敛。

**做**：`build_launch_spec`、真实 PTY 启动、headless 生命周期取证、真 GUI 验收、
证据到位后翻转 `launch` / `terminal` 两个能力。

**不做**：Usage（ccusage 不覆盖 opencode，数据源另立设计）、`resume` / `replay` /
`tool_calls` / `subagents` / `live_state` / `worktree`（保持 false）、配置投影、
不读写 `opencode.db` 与 `auth.json`。

## 本机事实（2026-10-12 取证）

```text
PATH:      D:\Scoop\shims\opencode.exe（Scoop 原生 exe，**没有** .cmd → node 中间层）
--version: 1.18.23（纯数字串，现有 parse_version 直接可解析）
数据目录   %USERPROFILE%\.local\share\opencode（另有 .config / .local\state / .cache）
凭据       opencode auth list → 0 credentials（无凭据）
打包方式   Bun 单文件二进制（栈帧形如 B:/~BUN/root/chunk-*.js）
```

PTY 内实测（`opencode_pty_spike.rs`，全部为本机真实字节）：

```text
DSR        发送 ESC[6n 并等待；不应答只有 4 字节（与 Claude / Gemini 同型）
TUI        **alternate screen 开启（?1049h）、bracketed paste 开启（?2004h）、
           mouse 开启（?1000h/?1002h/?1006h）、hide cursor、OSC 0 标题**
首屏内容   ASCII「OpenCode」标题 + 「Ask anything... "Fix broken tests"」输入框 +
           「Build · Big Pickle OpenCode Zen」模型行 + 「tab agents / ctrl+p commands」+
           「● Tip Run /connect to add an AI provider and start coding」+
           底部 cwd 与版本号 1.18.23
自然退出   Ctrl+C×2 干净退出，exit_code = 0（依次 ?2004l / ?1049l / ?2031l + 清屏）
输入往返   写入 "x" 触发重绘（[16;17Hx），输入→输出往返成立
噪声       stderr 可见 MaxListenersExceededWarning（来自本机 davinci-resolve MCP 的
           提示路径），**不是**致命错误；日志侧同一 run 正常 bootstrapping → init
```

**与 Gemini / Claude 的关键差别**：Gemini 与 Claude 的 `.cmd` shim 在本会话宿主上下文里
停在 node 启动前（xterm 空白），而 OpenCode 的原生 exe **在 PTY 里真的渲染出完整
TUI 首屏**，截图级可读（ASCII 标题 + 输入框 + 模型行 + 提示行）。因此本任务的
GUI 证据预期强于 Gemini 轮：首屏渲染可达。

**关键归因（避免误读）**：spike 的 `printable_tail` 里出现的 Bun 栈帧来自一次
`MaxListenersExceededWarning`（EventTarget 监听器计数告警），由本机用户配置的
`davinci-resolve` MCP server 触发；同一 run 的 OpenCode 自身日志显示
`creating instance → bootstrapping → loading config → init count=27`，
即**启动正常**。首屏文案也印证：这是正常的欢迎界面，不是崩溃屏。

本机 0 credentials 属 **readiness**（ADR-0005）：只影响「此刻能不能真的对话」，
不影响「Hub 终端机制是否对 OpenCode 全链路工作」，不得回写 capability。

## 设计

- `opencode.rs` 补 `build_launch_spec`：复用 `program_for`（本机为 `.exe`，直跑），
  cwd 透传，**默认不注入任何参数**（不替用户选 model / agent，不传 `--auto`）。
- **不注入 `--mini`**：OpenCode 有 `--mini`（minimal interactive interface），但那是
  改变用户界面形态的产品决定，Hub 的默认必须是「与裸终端一致」；要限幅应由用户
  在设置里显式选择（v0.1 未实现该维度）。适配器测试锁死「零默认参数」。
- Core 零改动：`TerminalRuntime::start` 已是 Harness 无关（installation → adapter →
  `LaunchSpec` → PTY），前端过滤纯按能力 DTO，能力翻转后 OpenCode 自动进入启动选择器。
- 能力翻转必须同时改：`fixtures/opencode/summary.json`（Rust/TS 共享契约）、
  `two_harness_inventory.rs` 矩阵锁、`opencode.rs` 单测。

## 验收（能力翻转的证据门）

1. **spike**（`opencode_pty_spike.rs`）：真实 `opencode.exe` 进真实 PTY；实测并记录
   DSR、alternate screen、bracketed paste、mouse、首屏内容、Ctrl+C×2 自然退出；
   原生 `.exe` 可直接作为 program。
2. **headless 生命周期**（`real_opencode_terminal.rs`）：spawn → `running` + 真实
   pid → 真实 TUI 字节 → DSR 闭环 → resize 不掉线 → user kill → `user_killed` +
   真实退出码；binary 缺失时启动失败绝不假装 running。
3. **四条状态链**（`opencode_lifecycle.rs`）：user_killed / natural_exit（Ctrl+C×2）/
   host_shutdown / lost 各自经生产收敛函数 `reconcile_orphans` 落库，零 ghost。
4. **真 GUI 验收**：真实 Terminal 页面选择 OpenCode → 启动 → xterm 出现真实 TUI
   首屏 → 终态，记录进 `tests/e2e/README.md`。chat 级交互因 0 credentials 不可达，
   如实记录为 readiness 边界。
5. 能力矩阵：`opencode.launch` / `opencode.terminal` = true 且其余 false，由矩阵锁
   测试锁死；fixture 契约测试（Rust serialization + TS normalize）同步。
6. 完成 AGENTS.md 全部门禁并记录真实结果。

## 来源与限制

- 本机 `opencode --version` = `1.18.23`；`opencode auth list` = 0 credentials。
- OpenCode 是 Bun 单文件二进制；`--mini` / `--auto` / `--model` / `--agent` 等选项
  实测存在，但默认一律不注入（见上）。
- 首屏内容含用户自己的配置产物（本机 MCP 告警文案）；fixture 与测试只锁协议层
  事实（TUI 标志位、DSR、退出码），不把用户配置文案写进仓库。
- 0 credentials 下的 chat 级往返不可达；与 Gemini 轮同规则，留待凭据恢复后的
  GUI 轮补充。
