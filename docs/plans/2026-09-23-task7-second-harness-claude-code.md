# Task 7 — Second Harness Validation: Claude Code

> 目标不是「多支持一个 Harness」，而是**验证 alpha.2 的抽象是真的通用，还是披着 Adapter
> 外衣的 Codex 专用实现**。因此本 Task 的成功标准是「接 Claude 时 Core 基本不用改」，
> 不是「Claude 能跑起来」。
>
> 版本语义：本 Task 完成后的 `v0.1.0-alpha.3` = **Multi-Harness Runtime**。

## 范围

**做**（只有这些）：

```text
Claude Code
├─ detect / version
├─ installation persistence（claude@local）
├─ build_launch_spec
├─ launch → PTY terminal → session lifecycle
├─ kill / resize / crash recovery
├─ ccusage usage association
└─ capability matrix
```

**不做**：Claude profile / OAuth 账号切换、CLAUDE.md 管理、Skills、MCP 同步、Provider 预设、
Configuration Plane。这些属于 Task 8 之后（alpha.4 = Profiles & Configuration）。

## 取证：本机真实事实（2026-09-23）

```text
PATH:      D:\npm-global\claude.ps1 (ExternalScript)
           D:\npm-global\claude.cmd (Application)
           D:\npm-global\claude     (Application, 无扩展名)
--version: 2.1.126 (Claude Code)

数据目录 %USERPROFILE%\.claude\
           backups/  ide/  plugins/  projects/  sessions/
           history.jsonl  settings.json  .last-cleanup
另有 %USERPROFILE%\.claude.json

ccusage 已经在报 claude（真机库里已有 4 条）：agent = "claude"，
period 是**裸 UUID**（不含日期），时间只能来自 metadata.lastActivity。
```

因此 Windows shim 形态与 Codex **完全同构**（`.cmd` / `.ps1` / 无扩展名三件套），
`candidate_paths`（`.cmd` 优先、无扩展名最后）与 `program_for`（`.ps1` → pwsh）应可直接复用 ——
但这是**待验证的假设**，7B 必须用真实启动证明，不能靠推断。

## 事前审计：Core 里到底有没有 Codex 特判

在写任何代码之前，对全仓做了 `codex|Codex|CODEX` 审计（`src-tauri/src` 240 处、`src/` 51 处）。
**生产代码路径上只有 4 处，全部是合法的，没有一处是 `if harness_id == "codex"`**：

| 位置                                                              | 性质                                                    | 结论                                                                                    |
| ----------------------------------------------------------------- | ------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| `lib.rs` `build_harness_registry()`                               | **组合根**：必须有地方注册适配器                        | 加 Claude = 多一行 `registry.register(...)`                                             |
| `harness/probe.rs` / `launch.rs` / `store.rs` / `adapter.rs` 注释 | 文档举例（「Windows 上 codex 有三种 shim」）            | 代码本身与 Harness 无关（`candidate_paths(dir, name)`）                                 |
| `usage/ccusage.rs` `day_from_period`                              | 行为与 Harness 无关：period 长得像日期就解析，否则 NULL | Claude 的裸 UUID 已有测试覆盖（`leaves_occurred_at_null_when_it_can_never_be_derived`） |
| `src/features/harnesses/HarnessesPage.tsx:150`                    | UI 文案「当前注册表中只有 Codex」                       | **注册 Claude 后这句会变成错的**，必须改（唯一的前端生产改动）                          |

其余 280+ 处全是测试夹具与注释。`harness/adapters/mod.rs` 的注释里本来就写着
`codex.rs`、`claude_code.rs`、`gemini_cli.rs`、`opencode.rs`。

**结论：验收标准 #1（Core 不出现 Harness 特判）在 Task 7 开始前已经成立**，
且 `usage/*`、`terminal.rs`、`session/*` 对 Harness 完全无感。这降低了 Task 7 的风险，
但也意味着「Core 没改」这句话必须在 7D 之后**用 diff 证明**，而不是靠这段审计。

## 四条验收标准（写成可执行的门）

1. **Core 无 Harness 特判**：`git diff` 里不得出现新的 `harness_id ==` / `harness == "codex"` 分支；
   除组合根与 `adapters/claude_code.rs` 外，Core 文件不得新增 Claude 相关分支。
2. **同一个 Terminal Runtime**：不新增 `ClaudeTerminalRuntime`；Claude 会话走
   `ClaudeAdapter::build_launch_spec` → 现有 `PtyBackend` → 现有 `TerminalRuntime` → 现有 Channel → 现有 xterm.js。
3. **零宣称的能力矩阵**：`detect` 先 true；`launch` / `terminal` / `usage` 各自拿到**指定的真机证据后**才翻 true；
   `resume` / `replay` / `toolCalls` / `subagents` / `liveState` / `worktree` / `profiles` 保持 false。
4. **不复制第二个导入器**：保持 `ccusage → CcusageAdapter → NormalizedUsage → {codex, claude}`；
   不得出现 `ClaudeCcusageImporter`。Harness 差异只允许停在 normalize/mapping 的边缘。

## 步骤

### 7A — Detection（Claude 出现在 Harnesses 页）

- Create: `src-tauri/src/harness/adapters/claude_code.rs`
  （`CLAUDE_ID = "claude"`、`EXECUTABLE_NAME = "claude"`、`claude_data_dir_candidates` =
  `~/.claude`，`detect` / `version` / `capabilities`）
- Modify: `src-tauri/src/harness/adapters/mod.rs`、`src-tauri/src/lib.rs`（组合根注册）
- Modify: `src/features/harnesses/HarnessesPage.tsx`（那句会变错的文案）+ 其测试
- Tests:
  ```rust
  #[test] fn claude_adapter_reports_the_real_binary_and_version()   // 真机：D:\npm-global\claude.cmd / 2.1.126
  #[test] fn claude_data_dir_is_dot_claude()                        // ~/.claude（实测存在）
  #[test] fn claude_capabilities_start_with_only_detect()           // 零宣称
  #[test] fn registry_holds_two_adapters_without_core_changes()     // 注册表是 Harness 无关的
  #[test] fn reconcile_persists_claude_at_local_installation()      // claude@local
  ```
- 真机验收：启动应用 → Harnesses 页同时列出 Codex 与 Claude（版本、binary、数据目录）。

### 7B — Terminal Runtime（用现有地基跑真实 Claude）

- 预期**零 Core 改动**；若发现需要改，先记录再改，并说明为什么不是「Claude 专用」。
- Tests（沿用 Task 4 的地基与 `windows_pty_spike` 手法）：
  ```text
  claude@local → LaunchSpec（program = claude.cmd，cwd 透传）
  PTY spawn → session=running + pid
  真实输出到达（Claude 的首屏，含 DSR 行为**实测**：Claude 是否也用 ESC[6n 必须测量，不能假设）
  GUI 输入 → Claude 收到（真实交互）
  resize 生效且不串
  kill → user_killed + 真实退出码；正常退出 → natural_exit
  崩溃恢复：强杀应用 → 下次启动收敛为 unknown/lost，不出现 ghost running
  ```
- 验收后翻 `launch` / `terminal`。

### 7C — Usage（Claude 记录进同一个管道）

- 预期**零导入器改动**：ccusage 的 `agent = "claude"` 记录已经在真机库里导入过 4 条。
- Tests：
  ```text
  真实跑几次 Claude → ccusage session → CcusageAdapter → SQLite
  → usage_events.harness = "claude"（source_session_id 是裸 UUID，occurred_at 来自 lastActivity）
  → Dashboard byHarness 自然出现 Claude 一行（不改 Dashboard 代码）
  幂等：重复导入 inserted = 0
  与 codex 记录共存时，stable_source_key 不冲突、两个 harness 的 usage_sessions 分别计数
  ```
- 验收后翻 `usage`（前提：Dashboard 上真的出现 Claude 分组）。

### 7D — Cross-Harness E2E（"Hub" 才真正成立）

同时跑一个 Codex 会话与一个 Claude 会话，验证：

```text
两个 PTY 不串流          输出只出现在各自的终端
两个 session_id 独立     sessions 表两行、installation 各自正确
输入不串                 在 A 打字不会到 B
resize 不串              改 A 的尺寸不影响 B
kill 一个不影响另一个    被 kill 的进终态，另一个仍在 running
DB lifecycle 独立        终态、退出码、termination_reason 各自落库
Dashboard usage 可区分   byHarness 同时有 codex 与 claude
重启无 ghost             退出重启后无 running 残留
```

## 已知风险（必须实测，不许推断）

1. **Claude 的 TTY 行为未知**：它可能要求真实 TTY、可能首屏需要 DSR 应答、可能拒绝在
   `--no-tty` 环境下启动。DSR 由 xterm.js（GUI）或 headless responder（测试）承担，
   但 Claude 是否需要是**待测量**的事实。
2. **`.ps1` 与 `.cmd` 的优先级**：Codex 实测 `.cmd` 可直跑；Claude 是否同样必须实测
   （若 `claude.cmd` 有包装行为差异，要记录而不是猜）。
3. **ccusage 的 claude `period` 是裸 UUID**：本地 `sessions.source_session_id` 与它的对应关系
   目前**不可证明**，因此 `hub_session_id` 仍必须为 NULL（ADR-0011 决策八）。
   绝不能因为「都是 claude」就硬连。
4. 前端那句「当前注册表中只有 Codex」的文案会过期（审计已发现）。

## alpha.3 完成标准

```text
Codex   ✓ detect ✓ launch ✓ terminal ✓ usage
Claude  ✓ detect ✓ launch ✓ terminal ✓ usage
Cross-Harness concurrent sessions ✓
pnpm verify / format:check / python:test 全绿
真机 GUI 验收：两个 Harness 同时可用、Dashboard 同时显示两者
```

## 7B spike 实测结果（`tests/claude_pty_spike.rs`，2026-09-23）

真机 `D:\npm-global\claude.cmd` 进 `portable-pty`，两段观察：

```text
[1] 不应答任何查询：bytes=4   dsr=1  → 输出只有 ESC[6n，然后**一直等**
[2] 应答 ESC[1;1R  ：bytes=1067 dsr=1 bracketed_paste=1 hide_cursor=1 mouse=0 alt_screen=0
    printable head: ESC[6n ESC[?9001h ESC[?1004h ESC[m ]0;C:\Windows\system32\cmd.exe
                    ESC[?25h ]0;claude ESC[?25l ESC[?2004h ESC[?1004h ESC[?2031h
                    ──────────────…（box drawing，真 TUI 画出来了）
```

由此得到的事实（全部是实测，不是从 Codex 推断）：

| 问题                          | 实测结论                                                              |
| ----------------------------- | --------------------------------------------------------------------- |
| `.cmd` 能否直接作为 `program` | **能**（不包命令解释器；与 Codex 相同结论）                           |
| 是否要求真实 TTY              | **是**：不应答 DSR 时它只发一个 `ESC[6n` 就不再输出                   |
| 是否发送 DSR                  | **是**（`ESC[6n`），必须由终端侧应答，否则永久等待                    |
| alternate screen              | **否**（无 `?1049h`）                                                 |
| bracketed paste               | **是**（`?2004h`）                                                    |
| mouse mode                    | **否**（无 `?1000/1002/1003/1006h`）                                  |
| 其他模式                      | 出现 `?9001h`、`?1004h`（focus reporting）、`?2031h`、窗口标题 `]0;…` |
| PATH / node 依赖              | 正常（TUI 真的画出来了，说明 node 起来了）                            |

**PTY 层仍然一个字节都不解析**：`?9001h / ?1004h / ?2031h` 这类模式交给 xterm.js
（认识就处理、不认识就忽略），Rust 侧只做 raw 透传（ADR-0009）。

### 一个必须诚实标注的未知

第一次跑 spike 时测试**挂住**了（读取线程 join 不返回）。我的 spike 代码本身有缺陷
（只要 `pair.master` 还活着，读取就不会 EOF），所以**不能**据此断言「Claude 的 `.cmd`
shim 被杀后子进程仍持有 PTY」。7B 的 kill 验收必须把这件事**单独测出来**，
而不是拿这次的现象当结论。清理用 `taskkill /F /T /PID`（定向树杀，绝不安杀所有 node）。

### 7B 收口状态（2026-09-25 复核）

上一条「仍然欠的验收」写于 2026-09-23 spike 之后，**已被后续真机验收完成**，现按证据改写：

```text
链路                        证据
启动                       7B 最终 GUI 验收（真实按钮 → running+pid → 首屏显示 cwd）
输入输出                   7B 最终 GUI 验收（xterm 输入 → Claude → 答案 585987 只出现在提交之后）
resize / reflow            7B.1 最后一块（ShowWindow → viewport → xterm → TUI 横线 136→196）
结束·用户主动              7B 最终 GUI 验收（user_killed + 真实退出码 1 + 零 ghost）
结束·自然退出              claude_lifecycle S2（headless 真机）
孤儿恢复（host_shutdown/lost）claude_lifecycle S3/S4 + 7D-B 步骤 6（真实 GUI 强杀重启收敛）
启动失败不假装 running      real_claude_terminal::a_missing_claude_binary_never_produces_a_running_session
能力翻转                    launch / terminal / usage 已为 true，并由 the_capability_matrix_… 锁死
```

证据原文见 `tests/e2e/README.md` 的「Task 7B 最终 GUI 验收」「Task 7B.1」两节。

2026-09-25 时的缺口清单；B 的更新状态见下方 2026-09-27 复核：

```text
A  自然退出只在 headless 观测过（S2）；真实 GUI 里让 Claude 自己退出（/exit）尚无证据
B  Claude **单独**会话的 GUI 强杀重启收敛已于 2026-09-27 取证；见 tests/e2e/README.md
C  GUI 取证驱动（cdp.mjs / dbdump.py）在仓库外 D:\HarnessHub-E2E，验收无法从仓库复现
   （属于「把 GUI 验收固化进仓库」，本 Task 不动，已在 e2e 记录里如实标注）
```

2026-09-27 复核：A 仍缺真实 GUI `/exit` 证据。本轮 GUI 的 Claude 会话进入 running，
但 xterm 为空，观察到 `claude.cmd` 的 `cmd.exe` 而没有常驻的 `claude.exe` 子进程；
两次启动均如此。独立的 `claude_lifecycle::s2_claude_can_exit_naturally` 预编译测试
仍可完成 `/exit`，所以不能把 headless 结果当作 GUI 通过。B 的新证据只覆盖孤儿收敛，
不覆盖 Claude TUI 的启动或交互。

2026-09-28 对照：同一预编译桌面 exe 的 Codex GUI 启动也出现空白 xterm，停在
`cmd.exe /c codex.cmd`；所以本轮不能把空白归因于 Claude 适配器。先用能从当前源码
重新构建的 Windows 桌面程序复现，再判断是否有产品缺陷。
