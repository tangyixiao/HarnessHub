# OpenCode CLI 检测接入实施计划

规格：[opencode-detection-design](../specs/2026-10-12-opencode-detection-design.md)。
基线：`ff012c8`（PR #9 合并后），分支 `feat/opencode-detection`。

## Task 1：RED —— 注册与契约断言先行

- `two_harness_inventory.rs`：生产注册/冷启动断言从四 Harness 改为五
  （顺序 codex、claude、gemini、codebuddy、opencode），矩阵追加 opencode 全
  false 断言，保留原有真机与首次发现时间检查。
- 运行确认失败（无适配器）。

## Task 2：适配器与 fixture（RED → GREEN）

- `fixtures/opencode/`：version.txt（真实输出 `1.18.23`）+ summary.json
  （全 false capabilities，与 Rust 序列化同形）+ README.md。
- `opencode.rs`：先加单测（detect / 未知版本 / 四个根存在与缺失 / XDG 覆盖
  不回退默认 / home 缺失 / 全 false 能力 / launch 未实现 / fixture 契约），
  确认编译失败后补最小实现。
- 组合根 `lib.rs` 注册第五个适配器（四个 XDG 变量在组合根读取一次）。
- `ipc.test.ts` 消费同一 fixture（normalize 契约）。
- Expected：Rust/TS 全绿。

## Task 3：真机检测

- 新增 `opencode_detection.rs` 真机只读检测（未安装显式 SKIP），断言报告的
  数据目录真实存在。
- 本机真机验证 `opencode --version`、`opencode.exe` 选中与四个根发现。

## Task 4：门禁、文档与提交

- 同步 `CONTEXT.md`、`CONTEXT-MAP.md`、`lib.rs` 注释、`adapters/mod.rs`、
  `fixtures/README.md`（顺带补齐 codebuddy/gemini 目录清单与过时末句）。
- 全部完成门命令 + `git diff --check`；真实 GUI 验证 Harnesses 页五行 +
  Terminal 无 OpenCode 入口。
- 一件事一提交（spec/plan + feat + docs）；不推送限制解除后按用户流程 PR。

## 已知风险

1. `--version` 输出 `1.18.23` 为纯数字串，复用现有解析即可。
2. OpenCode 是本机唯一走 Scoop 原生 exe 的 Harness：`candidate_paths` 的
   `.exe` 分支命中；若将来出现 `.cmd` shim，枚举顺序仍优先真实可执行扩展名。
3. XDG 覆盖只影响**报告哪些目录**，不影响 installed/version；覆盖存在时
   按「不回退默认」处理，与 Gemini 的既有纪律一致。
4. 真机测试在本机可跑（已安装）；CI 无 opencode 时必须显式 SKIP。

## 执行记录

### Task 1 RED（实测）

`cargo test --test two_harness_inventory` → **4 failed / 2 passed**：

```text
left: ["codex", "claude", "gemini", "codebuddy"]
right: ["codex", "claude", "gemini", "codebuddy", "opencode"]
left: ["claude", "codebuddy", "codex", "gemini"]
right: ["claude", "codebuddy", "codex", "gemini", "opencode"]
panicked at tests\two_harness_inventory.rs:291:14: 必须已注册
```

仍绿：`the_real_machine_detects_both_harnesses`、`a_second_harness_requires_no_migration`。

### Task 2 GREEN（实测）

- `cargo test --lib harness::adapters::opencode` → **11 passed / 0 failed**。
- `cargo test --test two_harness_inventory` → **6 passed / 0 failed**。
- `pnpm test` → **135 passed**（原 134，新增 OpenCode fixture 契约 1 条）。
- 变异测试（两次都被抓，证明断言承重）：
  1. 忽略 XDG 覆盖 → 3 failed（`an_explicit_xdg_root_is_listed_even_when_home_is_unknown`
     给出 `left: [] right: ["D:/xdg-data\\opencode"]`）；
  2. 在收集链尾加 `.rev()` 反转目录顺序 → 3 failed
     （`reports_only_existing_xdg_data_directories_in_a_stable_order`）。

### Task 3 真机（实测）

`cargo test --test opencode_detection` → **1 passed**：

```text
OpenCode: installed=true binary=D:\Scoop\shims\opencode.exe version=1.18.23
data=["C:\\Users\\tangy\\.local\\share\\opencode", "C:\\Users\\tangy\\.config\\opencode",
      "C:\\Users\\tangy\\.local\\state\\opencode", "C:\\Users\\tangy\\.cache\\opencode"]
capabilities=all false
```

四个根与 `opencode debug paths` 完全吻合，顺序 data → config → state → cache。

### 门禁

| 命令                                | 结果                            |
| ----------------------------------- | ------------------------------- |
| `pnpm format:check`                 | 通过                            |
| `pnpm rust:fmt:check`               | 通过                            |
| `pnpm lint`                         | 0 errors（4 条既有 warnings）   |
| `pnpm typecheck`                    | 通过                            |
| `pnpm test`                         | 135 passed                      |
| `pnpm rust:test`                    | 全绿（`gemini_pty_spike` 见下） |
| `pnpm rust:clippy`（`-D warnings`） | 通过                            |
| `pnpm python:test`                  | 12 tests OK                     |
| `pnpm build`                        | 通过                            |
| `git diff --check`                  | 通过                            |

### Task 4 真实 GUI（实测，2026-10-12）

本机 `vite` 默认端口 1420 已落入 Windows 保留区间（`netsh interface ipv4 show
excludedportrange` 显示 1355–1454），`pnpm tauri dev` 因此报
`Error: listen EACCES: permission denied ::1:1420`。按 `tests/e2e/README.md` 既有
办法改用 `--config D:\HarnessHub-E2E\7b2-tauri-override.json`（端口 3300）+
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` 启动当前源码
构建，用 CDP 驱动真实页面：

```text
Harnesses 页（点「重新检测并同步」后）：
  Codex 0.156.1 / Claude Code 2.1.126 / Gemini CLI 0.63.0 / CodeBuddy 2.161.0
  OpenCode  1.18.23  可执行文件 D:\Scoop\shims\opencode.exe
            数据目录 C:\Users\tangy\.local\share\opencode、C:\Users\tangy\.config\opencode、
                     C:\Users\tangy\.local\state\opencode、C:\Users\tangy\.cache\opencode
            能力矩阵九项全部为「—」（未支持）

Terminal 页「启动的 Harness」下拉：["Codex","Claude Code","Gemini CLI"]
  → 不包含 CodeBuddy 与 OpenCode，检测阶段 Harness 不出现启动入口 ✓
```

同一 GUI 会话写库后的只读快照（`harness_installations`）：

```text
claude@local    2.1.126   first_detected_at=2026-09-23T12:37:44Z  last_seen_at=2026-10-12T13:06:17Z
codex@local     0.156.1   first_detected_at=2026-09-22T11:26:41Z
gemini@local    0.63.0    first_detected_at=2026-10-11T00:56:19Z
codebuddy@local 2.161.0   first_detected_at=2026-10-11T06:03:43Z
opencode@local  1.18.23   first_detected_at=2026-10-12T12:54:32Z  last_seen_at=2026-10-12T13:06:17Z
             data_paths = 四个 XDG 根（顺序 data、config、state、cache）
```

`harnesses` 定义表为 claude / codebuddy / codex / gemini / opencode 五行。
既有四个安装的 `first_detected_at` 未被改写，说明新增 Harness 是追加而不是重建。

### 基线既有 flake：`gemini_pty_spike.rs`（与本次改动无关）

`pnpm rust:test` 会在 `real_gemini_in_a_real_pty_reports_what_it_needs` 上非确定性失败。
**已在未改动的基线 `ff012c8` 上复现**（独立 worktree
`D:\HarnessHub-baseline-ff012c8`，3 次运行 1 次失败），证据：

- 失败签名不固定：改动分支上分别落在 `line 270`（bracketed paste）与 `line 282`（Ctrl+C×2 退出）。
- 退化形态一致：某个观察槽只有 **94 bytes**、`dsr=1 bracketed_paste=0`
  （`cmd.exe` 横幅输出后 node 从未启动），同一测试的其它槽却正常
  （8687 / 11025 / 13184 bytes，`bracketed_paste=1`，`exited=Some(0)`）。
- 基线 3 次运行里，94-byte 退化**分别出现在不同观察槽**（[5]、[4]、[2]）——
  同一代码在不同槽位随机退化，说明是启动时序而非固定逻辑缺陷。
- 归属：与 `tests/e2e/README.md` 已记录的「shim `cmd.exe` 停在 node 启动前」
  空白终端现象同型；本机 Gemini CLI 仍为 0.63.0，非上游版本变化。
- 本 diff 不触及任何 Gemini 代码路径。

结论：该失败是**环境/时序 flake**，不由本任务引入；不通过在测试里放宽断言来掩盖。
