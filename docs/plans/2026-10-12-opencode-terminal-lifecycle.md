# OpenCode CLI Terminal 生命周期实施计划

规格：[opencode-terminal-lifecycle-design](../specs/2026-10-12-opencode-terminal-lifecycle-design.md)。
基线：`e882c0b`，分支 `feat/opencode-terminal-lifecycle`；既有未跟踪 `docs/handoffs/` 保留且不提交。

## Task 1：spike 实测（先测量，后实现）

- 新增 `src-tauri/tests/opencode_pty_spike.rs`（沿用 `gemini_pty_spike.rs` 手法）：
  三条观察 —— 不应答任何查询 / 应答 DSR 后写输入 / 应答后 Ctrl+C×2。
- 建 E2E 工作区 `D:\HarnessHub-E2E\opencode-terminal`（不动 `wt-main` 与其他既有目录）。
- 运行并记录：原生 `.exe` 是否直跑、是否发 DSR、alternate screen / bracketed paste /
  mouse、首屏内容、TUI 自然退出路径与退出码。
- Expected：得到可回填 spec 的实测表；断言只写已观测事实。

## Task 2：build_launch_spec（RED → GREEN）

- 改写 `opencode.rs` 单测：已安装时 `build_launch_spec` 生成正确 spec（program /
  args 透传 / cwd / runtime_target），未安装时报错；确认先失败。
- 最小实现：复用 `program_for`，**默认零参数**。能力矩阵保持全 false（证据未齐）。
- Expected：单测全绿；`two_harness_inventory` 矩阵锁不变。

## Task 3：headless 生命周期取证

- 新增 `src-tauri/tests/real_opencode_terminal.rs`：spawn → running+pid → 真实字节 →
  DSR 闭环 → resize → kill → `user_killed` + 真实退出码；缺失 binary 启动失败不产生
  running。
- 新增 `src-tauri/tests/opencode_lifecycle.rs`：S1 kill → user_killed；S2 Ctrl+C×2 →
  natural_exit；S3 host_shutdown；S4 lost，全部走生产 `reconcile_orphans`。
- Expected：本机全绿；未安装机器显式 SKIP。

## Task 4：真 GUI 验收

- 从当前源码启动 Tauri dev 桌面端，Terminal 页面选择 OpenCode、cwd 指向 E2E 工作区，
  记录：下拉项出现、启动、首屏、终止、DB 终态、零 ghost。
- 结果写入 `tests/e2e/README.md` 新节。GUI 不可用则如实记录阻塞，**不翻转能力**，
  停在 Task 3 的中间态提交。

## Task 5：能力翻转与文档同步

- `opencode.rs` capabilities 翻 `launch` / `terminal` + 注释引用证据；
  `fixtures/opencode/summary.json` 同步；`two_harness_inventory.rs` 矩阵锁加 opencode 行；
  `src/features/harnesses/HarnessesPage.test.tsx` / Terminal 过滤断言按需同步。
- 同步 `docs/CONTEXT.md`、`docs/CONTEXT-MAP.md`、adapter 模块注释。
- Expected：Rust serialization contract 与 TS normalize fixture 测试同时消费新 fixture。

## Task 6：门禁与提交

- 全部完成门命令 + `git diff --check`；审阅实际 diff。
- 按一件事一提交：`docs(spec)` → `feat(harness)`。计划原写的第三个提交
  「能力翻转 + GUI 证据」最终并入第二个：能力翻转与之配套的单测/矩阵锁/fixture 都在
  同一个 `opencode.rs` 与同一组契约文件里，拆成两笔会产生「测试断言的能力与实现不一致」
  的中间态（违反 §20「每一步都必须保持项目可运行」），且 CodeBuddy PR 的先例也是两笔。

## 已知风险（必须实测）

1. 原生 `.exe` 直跑：结论来自 OpenCode 是 Bun 单文件二进制，spike 必须证实。
2. 首屏含 Bun 告警噪声（本机 MCP 引起）：必须区分「告警」与「崩溃」，否则会误判 flake。
3. 自然退出路径：Ctrl+C×2 已由 spike 证实；是否会被初始化吞键需在 lifecycle 里像
   Gemini 那样「等画面静默 + 最多补一对」。
4. 0 credentials：chat 级往返不可达，属 readiness，不得回写 capability。

## 执行记录

执行日期 2026-10-12；基线 `e882c0b`，分支 `feat/opencode-terminal-lifecycle`。

### Task 1：spike

`src-tauri/tests/opencode_pty_spike.rs` 新建，`cargo test --test opencode_pty_spike
-- --nocapture` → **1 passed / 34.11s**：原生 `D:\Scoop\shims\opencode.exe` 直跑，不应答
DSR 时只发 4 字节 `\x1b[6n` 并停住；应答后 16508 字节、`alt_screen=1 bracketed_paste=11
mouse=3 clear=3`；Ctrl+C×2 → `exited=Some(0)`。断言已从「只断言有输出」升级为锁协议事实
（不应答 < 64 字节；应答含 `\x1b[?1049h` / `\x1b[?2004h`；退出含 `\x1b[?2004l` /
`\x1b[?1049l` 且码 0）。

风险 2 已排除：Bun 栈帧是 `MaxListenersExceededWarning`（11 drain listeners）告警，
另有一条本机 davinci-resolve MCP 的 `failed to getPrompt`；一次性 FULL SCREEN dump 证实
屏幕是正常欢迎界面（`]0;OpenCode` + ASCII 标题 + `Ask anything...` + `Build · Big Pickle
OpenCode Zen` + 工作目录 + `1.18.23`），dump 代码未提交。

### Task 2：build_launch_spec RED → GREEN

RED：4 条新单测（替换 `launch_is_rejected_while_unimplemented`）→ **3 failed / 11 passed**，
签名 `panicked at src\harness\adapters\opencode.rs:365:14: spec: InvalidInput("OpenCode 启动
尚未实现：当前仅支持安装检测")`（另两条 :386 / :406）。
GREEN：实现 `build_launch_spec`（复用 `program_for`、args 原样透传、空 cwd 视为未指定、
默认零参数）→ **14 passed / 0 failed**。

### Task 3：headless 取证

- `cargo test --test real_opencode_terminal -- --nocapture --test-threads=1` → **2 passed /
  8.95s**：`[1] spawn ok pid=51716`、`[2][3] bytes=3649 dsr_answered=1 alt_screen=1
bracketed_paste=11`、`[4] 新字节 = 8247`、`[5] resize ok（累计 23737）`、
  `[6] kill ok status=Exited reason=Some(UserKilled) exit_code=Some(1)`、
  `[8]` 缺失 binary 走 `CreateProcessW … (os error 3)` 且不留 running。
- `cargo test --test opencode_lifecycle -- --nocapture --test-threads=1` → **4 passed /
  20.37s**：`S1 user_killed: exit_code=Some(1)`；`S2 首屏 21514 bytes，bracketed_paste = 12
alternate_screen = 1` → `S2 natural_exit: exit_code=Some(0)`；`S3 host_shutdown:
status=Unknown`；`S4 lost: pid=Some(31316)`（幂等）。

### Task 5：能力翻转

`opencode.rs` `capabilities()` → `launch` / `terminal` true（其余七项 false，ccusage 不覆盖
opencode）；`fixtures/opencode/summary.json` 同步；`two_harness_inventory.rs` 矩阵锁由
「9 项全 false」改为断言这两项为真、false 列表降到 26 项。复验：`--lib
harness::adapters::opencode` **14 passed**、`--test two_harness_inventory` **6 passed /
7.26s**；`pnpm test` **7 files / 135 passed**。前端无需改代码（`harness-colors.ts:27` 已有
`opencode: 130`；`ipc.test.ts` 的 fixture 契约测试自动跟随）。

### Task 4：真 GUI

dev app 用 `--config D:\HarnessHub-E2E\7b2-tauri-override.json`（端口 1420 落入 Windows
保留区间，改用 3300）+ `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`，
CDP 驱动真实点击。

- 下拉出现 `["Codex","Claude Code","Gemini CLI","OpenCode"]`，选中 `opencode@local`、cwd
  `D:\HarnessHub-E2E\opencode-terminal`；启动 `pid 50816 · session d246dd2e-…`；终止 → UI
  「已结束 · 用户主动结束 · 退出码 1」；只读 DB 快照 `status=exited reason=user_killed
exit=1`、全库 `running=0`。
- **TUI 渲染阻塞，归因为环境现象**：xterm 零输出（`.xterm-rows` 31 行全空但 DOM renderer
  已挂载）；**Codex 对照在同一实例同样零输出**（pid 44796 · `non-empty rows after hold: 0`），
  其 `cmd.exe /c D:\npm-global\codex.cmd` 只有 1 个线程、无 node 子进程；GUI 启动的
  `opencode.exe` 无子进程、CPU 0.0156s、3.9MB、唯一线程 `Wait/Executive`；OpenCode 自己的
  `opencode.log` 在这两轮期间零新增。GUI 轮进行中同时跑 headless `real_opencode_terminal`
  仍 2 passed 有真实 TUI 字节 ⇒ 阻塞点在子进程取得调度前，不在 PTY 后端。
- 详见 `tests/e2e/README.md` 新增的「2026-10-12 — OpenCode 终端生命周期」节。按 ADR-0005
  规则 2，阻塞只影响 readiness，不回转能力；0 credentials 同属 readiness。

### Task 6：门禁

| 命令                  | 结果                 |
| --------------------- | -------------------- |
| `pnpm format:check`   | 见下方「门禁实测」   |
| `pnpm rust:fmt:check` | 见下方「门禁实测」   |
| `pnpm lint`           | 见下方「门禁实测」   |
| `pnpm typecheck`      | 见下方「门禁实测」   |
| `pnpm test`           | 7 files / 135 passed |
| `pnpm rust:test`      | 见下方「门禁实测」   |
| `pnpm rust:clippy`    | 见下方「门禁实测」   |
| `pnpm python:test`    | 见下方「门禁实测」   |
| `pnpm build`          | 见下方「门禁实测」   |

### 门禁实测

| 命令                  | 结果                                                                                                                                                                                                                                                                                                                       |
| --------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `pnpm format:check`   | `All matched files use Prettier code style!`，exit 0                                                                                                                                                                                                                                                                       |
| `pnpm rust:fmt:check` | exit 0（`opencode.rs` 三处断言换行 `cargo fmt` 后通过）                                                                                                                                                                                                                                                                    |
| `pnpm lint`           | `✖ 4 problems (0 errors, 4 warnings)`（routes.tsx 2 + DashboardPage.tsx 2，均为既有 react-refresh warning），exit 0                                                                                                                                                                                                        |
| `pnpm typecheck`      | `tsc --noEmit`，exit 0                                                                                                                                                                                                                                                                                                     |
| `pnpm test`           | `7 passed (7)` / `135 passed (135)`，exit 0                                                                                                                                                                                                                                                                                |
| `pnpm rust:test`      | exit 0 全绿（`opencode_lifecycle` 4 passed、`real_opencode_terminal` 2 passed、`opencode_detection` 1 passed、`two_harness_inventory` 6 passed、`gemini_lifecycle` 4 passed、`gemini_pty_spike` 1 passed、`real_gemini_terminal` 2 passed、`two_harness_concurrency` 3 passed、`windows_pty_spike` 2 passed、Doc-tests 0） |
| `pnpm rust:clippy`    | `Finished dev profile`，exit 0                                                                                                                                                                                                                                                                                             |
| `pnpm python:test`    | `Ran 12 tests ... OK`，exit 0                                                                                                                                                                                                                                                                                              |
| `pnpm build`          | exit 0                                                                                                                                                                                                                                                                                                                     |
| `git diff --check`    | 无输出，exit 0                                                                                                                                                                                                                                                                                                             |

门禁期间抓到一个真实回归并已修复：`opencode_detection.rs` 仍断言能力矩阵全 false（能力翻转
后失败，`left: HarnessCapabilities { launch: true, terminal: true, … }` vs `right: … all
false`）。已对齐 `gemini_detection.rs` 的写法：断言 `launch` / `terminal` true 且其余七项
false，测试名同步为 `the_real_machine_detects_opencode_with_terminal_lifecycle_capabilities`，
实测 `1 passed`（输出 `capabilities=launch+terminal`，四个 XDG 根与 `debug paths` 一致）。
