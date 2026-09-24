# e2e / 验收记录

本文件保存**真实验收输出**，而不是「应该可以」。没有记录的项一律视为未验收。

## 2026-09-21 — Phase 1 框架基线（Walking Skeleton 骨架）

环境：Windows（x86_64-pc-windows-msvc）、Node 26.9.0、pnpm 11.0.9、rustc 1.98.1、
Python 3.14.3（sidecar venv 由 uv 解析为 CPython 3.12.14）、WebView2 153.0.4234.48。

| 项                            | 命令                                              | 结果                                     |
| ----------------------------- | ------------------------------------------------- | ---------------------------------------- |
| 前端 lint / typecheck / build | `pnpm verify`                                     | 通过                                     |
| 前端单元测试                  | `pnpm test`                                       | `Test Files 2 passed`、`Tests 13 passed` |
| Rust 单元测试                 | `cargo test --manifest-path src-tauri/Cargo.toml` | `36 passed; 0 failed`                    |
| Rust 静态检查                 | `cargo clippy --all-targets -- -D warnings`       | 无警告                                   |
| Rust 格式                     | `cargo fmt --all -- --check`                      | 通过                                     |
| Python sidecar                | `pnpm python:test`                                | `Ran 12 tests ... OK`                    |
| 真实宿主检测冒烟              | `cargo test --test codex_detection`               | `1 passed`（真实 PATH + 真实子进程）     |

### 真实启动落库验收（已验收）

`cargo run --bin harness-hub` 在真实宿主上启动约 7 分钟后被外部超时终止（**未**发生 panic 或崩溃退出）。
由此产生的磁盘状态用 Python 标准库 `sqlite3` 独立读取，确认：

```text
库文件：%APPDATA%\dev.harnesshub.desktop\harness-hub.sqlite3（含 -wal / -shm）
tables(15): file_events, git_events, harnesses, imports, messages, models,
            project_harness_settings, projects, runtime_targets, schema_migrations,
            sessions, source_files, tool_calls, turns, usage_events
migrations: [(1, '0001_init')]
indexes(10): idx_git_events_session, idx_messages_session, idx_sessions_harness,
             idx_sessions_project, idx_sessions_started_at, idx_usage_day,
             idx_usage_harness_day, idx_usage_model_day, idx_usage_project_day,
             uq_sessions_source
journal_mode: wal
```

结论：`Tauri 启动 → Rust setup → Database::open → WAL → 迁移执行 → 磁盘 schema` 这条链路已成立。

### 明确**未**验收的项（不得当作已完成）

- 桌面窗口的**视觉**验收：未截图、未人工确认窗口内容与布局。只确认进程未崩溃。
- `pnpm tauri dev` 下的 Harness 启动、PTY 交互、ccusage 导入、Dashboard 数字：**未实现**，
  见 `docs/plans/2026-09-21-v0.1-walking-skeleton.md` Task 4 / 5 / 6。
- Linux 平台：本机未验证（CI 配置已就位但未在真实 runner 上跑过）。
- `.github/workflows/ci.yml`：本地无法执行，属未验证脚手架。

---

## 2026-09-21 — Task 2：Harness 能力矩阵接线到 IPC 与 UI（已验收）

### 验收方式

通过 WebView2 的远程调试端口（`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`）
用 CDP 驱动运行中的应用：`Page.navigate` 到 `#/harnesses`，再取 `document.body.innerText`
与 `Page.captureScreenshot`。

**该方式不触碰用户桌面**：不置顶窗口、不移动鼠标、不发送键盘事件、不截取整个屏幕。
（早先尝试过窗口置顶 + 模拟点击/按键，会抢占用户桌面，已废弃。）

### 实际检测结果（真实 detection，非 mock）

```text
location.href : http://localhost:1420/#/harnesses
Codex / Harness ID：codex / 已安装
版本          0.152.1
可执行文件    D:\npm-global\codex.cmd
数据目录（只读） C:\Users\tangy\.codex
能力矩阵      启动 ✓ 终端 ✓ 恢复 ✓ Usage — 回放 — 工具调用 — 子代理 — 实时状态 — Worktree —
侧边栏        Tauri 运行时已连接
```

对照命令：`D:\npm-global\codex.cmd --version` → `codex-cli 0.152.1`（一致）。

### 本次真机验证发现并修复的缺陷（重要）

第一次真机运行时 UI 显示 **版本未知**，`可执行文件` 为 `D:\npm-global\codex`：

- `candidate_paths` 把**无扩展名**排在首位（对 Unix 正确）。但 npm 在 Windows 上同时生成
  `codex`（POSIX bash 脚本）、`codex.cmd`、`codex.ps1`，无扩展名那份 **Windows 无法
  CreateProcess**，于是 `--version` 永远读不出来。
- 修复：Windows 上先枚举 `cmd / exe / bat / com / ps1`，无扩展名只作最后兜底；
  非 Windows 保持无扩展名优先。
- 回归锁：单元测试 `candidate_paths_puts_real_windows_executables_before_the_bash_shim`
  ＋ 集成测试 `prefers_a_real_windows_executable_over_the_bare_bash_shim`
  ＋ `detects_codex_without_lying_about_the_host` 中「选中可执行文件就必须读出版本」的断言。

单靠 FakeHostProbe 的单元测试**无法**发现该缺陷：假宿主只会回答被我配置过的那个路径。
这条缺陷是端到端启动应用才暴露出来的。

### 本轮验证命令与结果

| 命令                                | 结果                                                                                          |
| ----------------------------------- | --------------------------------------------------------------------------------------------- |
| `pnpm verify`                       | 退出码 0（lint / typecheck / 34 前端用例 / build / rust fmt / clippy / 43 单元 + 2 集成用例） |
| `pnpm python:test`                  | 退出码 0（`Ran 12 tests ... OK`）                                                             |
| `cargo test --test codex_detection` | `2 passed`（真实 PATH + 真实子进程）                                                          |

### 仍未验收

- 用户桌面上窗口的**视觉观感**（字体、间距等主观项）：只看过 CDP 页面截图与 innerText。
- 未安装 Harness 的机器上的 UI 表现：仅有 vitest 覆盖，没有真机。

---

## 2026-09-22 — Task 3：Clock / LocalRuntimeTarget / Session 编排（已验收）

### 验收方式

与 Task 2 相同：WebView2 CDP（`--remote-debugging-port=9222`）驱动运行中的应用，
`Page.navigate` / `Page.reload` + `Runtime.evaluate` + `Page.captureScreenshot`。
不置顶窗口、不动鼠标键盘、不截屏幕。

调用链路走**前端自己的 `ipc.ts`**（`await import('/src/lib/ipc.ts')`）而不是直调
`window.__TAURI_INTERNALS__.invoke`，因此同时覆盖了 `ipc.ts` 的参数整形与结果归一化。

### 端到端结果

```text
create_session({ harnessId: 'codex', cwd: 'D:/HarnessHub' })
  → { hubSessionId: "46d6b24c-…", runtimeTargetId: "local", status: "running",
      launchMode: "terminal", cwd: "D:/HarnessHub", startedAt: "2026-09-22T11:26:41Z",
      sourceSessionId: null, endedAt: null, exitCode: null }

Sessions 页（running）：codex / 46d6b24c-… / 运行中 / 开始时间 2026-09-22 11:26:41
                        / 结束时间 — / 工作目录 D:/HarnessHub
                        / 「仍在运行（尚未收到退出码） · 无外部 source session id」

finish_session(hubSessionId, 0) → true
list_sessions(limit 5)         → status "exited"、endedAt "2026-09-22T11:26:44Z"、exitCode 0

Sessions 页（reload 后）：已结束 / 结束时间 2026-09-22 11:26:44 / 退出码 0
```

**进程级重启后的持久化**（Release Gate 项「退出重启后 Session 历史仍存在」）：

```text
1. Stop-Process harness-hub        （不是页面刷新，是真正杀进程）
2. 用 Python 标准库读磁盘数据库：
   sessions(1): ('46d6b24c-…', 'codex', 'exited', 'terminal', 'D:/HarnessHub',
                 '2026-09-22T11:26:41Z', '2026-09-22T11:26:44Z', 0, 'local')
   harnesses:        [('codex', 'Codex', 1, '0.152.1', '2026-09-22T11:26:41Z')]
   runtime_targets:  [('local', 'local', '本机')]   ← utf-8 b'\xe6\x9c\xac\xe6\x9c\xba'
3. 重新 pnpm tauri dev → Sessions 页仍显示该会话（已结束 / 退出码 0）
```

### 本次真机验证发现并修复的缺陷（重要）

第一次 `create_session` 在真实应用里返回：

```text
数据库错误：FOREIGN KEY constraint failed
```

原因：`sessions.harness_id` 有指向 `harnesses(id)` 的外键，但**没有任何生产代码往
`harnesses` 表写过行** —— 检测结果一直只存在于内存注册表中。

- 单元测试**抓不到**：夹具 `seeded_db()` 会替调用方把 harness 行插好，
  正好掩盖了生产路径上缺失的那一步。
- 修复：新增 `harness/store.rs::upsert_harness()`，`create_session` 在落库前先按
  注册表的检测结果 upsert harness 行（未注册的 Harness 返回明确错误）。
- 回归锁：`upsert_harness` 的 4 个测试，其中
  `upserted_harness_satisfies_the_sessions_foreign_key` 直接复现原先失败的那条路径。

### 本轮验证命令与结果

| 命令               | 结果                                                                                          |
| ------------------ | --------------------------------------------------------------------------------------------- |
| `pnpm verify`      | 退出码 0（lint / typecheck / 55 前端用例 / build / rust fmt / clippy / 66 单元 + 2 集成用例） |
| `pnpm python:test` | 退出码 0（`Ran 12 tests ... OK`）                                                             |

### 仍未验收

- 会话的**实时刷新**：页面不会自动感知后台状态变化（缺「实时状态」能力）。
  本次是靠 `Page.reload` 才看到 exited 状态的 —— 这是预期行为，不是缺陷。
- 多 Harness：注册表目前只注册 Codex，其他 Harness 仍是占位。

---

## 2026-09-22 — Task 3 收口：两处语义修正（已验收）

review 结论：Task 3 架构方向通过，但必须先修两处语义，否则 Task 4 接 PTY 后更难理清。

### 修正 1：没有进程就不得是 `running`

新增 `created` 状态（ADR-0006）：

```text
create_session → created
Task 4 spawn 成功 → running
spawn 失败        → failed
进程退出          → exited（exit≠0 为 failed；拿不到退出码为 unknown）
```

### 修正 2：Harness 落库走显式同步路径

`list_harnesses` 保持**纯读**；写入只经过 `harness::inventory::reconcile_harnesses`，
调用点只有：应用启动、IPC `refresh_harnesses`（用户点按钮）、
`create_session` 的 invariant guard（ADR-0007）。同时把 `harnesses` 拆成
「定义」与「安装」两张表，为 WSL / SSH / Container 预留。

### 真实数据库上的 v1 → v2 迁移（最有价值的一段验证）

被迁移的是**上一版应用真实创建的开发库**（不是构造出来的夹具）。

迁移前：

```text
schema_migrations: [(1, '0001_init')]
harnesses 列: id, display_name, installed, binary_path, version,
              capabilities_json, data_paths_json, detected_at, created_at, updated_at
harnesses 行: ('codex', 'Codex', 1, 'D:\npm-global\codex.cmd', '0.152.1', '2026-09-22T11:26:41Z')
sessions  行: ('46d6b24c-…', 'exited', 'D:/HarnessHub', …, exit_code 0)
无 harness_installations 表
```

迁移后（应用启动时执行）：

```text
schema_migrations: [(1, '0001_init'), (2, '0002_session_created_and_harness_installations')]
harnesses 列: id, display_name, created_at, updated_at          ← 只剩定义
sessions 有 installation_id: True
harnesses（定义）:      [('codex', 'Codex')]
harness_installations: [('codex@local', 'codex', 'local', 'D:\npm-global\codex.cmd',
                         '0.152.1', 'available',
                         first_detected_at='2026-09-22T11:26:41Z',   ← 保留旧 detected_at
                         last_seen_at='2026-09-22T11:43:08Z')]      ← 启动同步刷新
session 老数据: ('46d6b24c-…', 'exited', 'codex@local', 'local', …, 0)  ← installation_id 已回填
runtime_targets: [('local', 'local', '本机')]                    ← 未被迁移污染
journal_mode: wal
PRAGMA foreign_key_check: 0 条违规                                ← 表重建没留下悬空引用
```

### created 语义的端到端验证

```text
listHarnesses()      → codex / installed / 0.152.1 / D:\npm-global\codex.cmd
                       / 全部 capability 为 false（纯读，未写库）
refreshHarnesses()   → { harnesses: 1, installations: 1 }        ← 显式写路径
createSession(...)   → status: "created", installationId: "codex@local"
                       （不是 running —— 此时没有任何进程）
finishSession(该会话) → false                                     ← 拒绝结束未启动的会话
Sessions 页          → 「已创建 · 尚未启动」/「尚未启动（没有进程）· 安装：codex@local」
                       页面中不出现「仍在运行」
Sessions 页（老数据）→ 「已结束」/「退出码 0」/「安装：codex@local」
```

### 本轮验证命令与结果

| 命令               | 结果                                                                                          |
| ------------------ | --------------------------------------------------------------------------------------------- |
| `pnpm verify`      | 退出码 0（lint / typecheck / 65 前端用例 / build / rust fmt / clippy / 86 单元 + 2 集成用例） |
| `pnpm python:test` | 退出码 0（`Ran 12 tests ... OK`）                                                             |

### 新增的测试纪律（已写入 AGENTS.md）

涉及 FK / migration / registry bootstrap 的行为，必须至少有一组**从真正空库开始、
只调用生产代码填充前置状态**的用例；验证迁移必须用**裸 Connection** 控制版本
（`Database::open_*` 会一次跑到最新，只能验证最终 schema，验证不了老数据搬运）。

### 仍未验收

- 上述 `running` 路径（`mark_running`）在真机上还没有触发点 —— 它属于 Task 4，
  目前只有单元测试覆盖 `created → running → exited` 全链路。
- 多 runtime target（WSL / SSH）只有单元测试，没有真实环境。

---

## Task 5：ccusage 使用量导入（真机 E2E，2026-09-22）

### 自动化证据

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test real_ccusage_import -- --nocapture`

实际输出（真机、真实调用 `npx --yes ccusage@20.0.24 session --sections daily --by-agent --json`）：

```text
runner：ManagedNpx → D:\nodejs\npx.cmd --yes ccusage@20.0.24
金额：Σ事件 - 来源 = 4 微单位（235 个计价事件，上界 118）
daily - session = 1247529；逐 agent 归因：["claude: daily 1709428 vs session 461899（差 1247529）"]
对账通过：事件 243 条 / total 4757843285 / 无法归因 910 / 金额残差 4 微单位 / 未定价 8 条
test real_ccusage_import_reconciles_against_its_own_totals ... ok
（28.11s）
```

### 这些数字说明了什么

| 断言                                     | 结果                                                                            |
| ---------------------------------------- | ------------------------------------------------------------------------------- |
| `Σ(事件 total) + 无法归因 == totals`     | 4,757,843,285 + 910 == 4,757,844,195（精确，不是近似）                          |
| 四类 token 逐项等于 `totals`             | input / output / cacheCreation / cacheRead 全等                                 |
| 金额残差 ≤ ⌈计价事件/2⌉ 微单位           | 实测 +4 微单位（0.000004 USD），上界 118 —— 来自每个事件独立舍入                |
| `daily` 与 `session` 的差异逐 agent 归因 | 1,247,529 全部落在 `claude`；`codex` 两个口径**逐 token 相等**                  |
| 幂等                                     | 第二次 `inserted = 0`、`skipped = 243`，库内汇总一分不变                        |
| 重启持久化                               | 关掉 SQLite 再打开，汇总一字不差；两次导入留下两条 `succeeded` 审计行           |
| 不伪造 hub session                       | 冷启动库里 `hub_session_id IS NOT NULL` 计数为 0                                |
| natural key 无碰撞                       | 243 条事件的 `stable_source_key` 两两不同                                       |
| `missingPricing` 的 0 元                 | 落成 `cost_microunits = NULL` + `cost_source = ccusage_missing_pricing`（8 条） |

### E2E 抓到的真实缺陷（已修）

`resolve_runner` 早期把 managed runner 的命令写成裸名字 `npx`：Windows 上实际是
`npx.cmd`，`Command::new("npx")` 直接 `Io(NotFound)`。现在执行**探测到的绝对路径**，
并补了一条单测锁住这一点（`managed_runner_executes_the_resolved_absolute_path`）。

### 仍未验收（本轮范围内）

- **没有 GUI 验收**：Task 5 只到 IPC（`get_usage_sources` / `refresh_usage`），
  使用量面板属于 Task 6，因此本轮的 E2E 全部是 Rust 侧的。
- `npx` 首次调用会下载包（本机已缓存），因此耗时约 28s；UI 侧必须有「进行中」状态，
  这件事属于 Task 6。

---

## Task 6：Dashboard 只读投影（真机数据链 E2E，2026-09-23）

### 自动化证据

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test real_dashboard_summary -- --nocapture`

实际输出（真机导入 Task 5 的数据之后）：

```text
Dashboard 对账通过：事件 243 / tokens 4757843285 / 已知成本 Some(288824073) 微单位 / 下界 true /
  usage sessions 222 / managed sessions 0 / 今天 0 条（30 天 204 条）
```

| 断言                                    | 结果                                                          |
| --------------------------------------- | ------------------------------------------------------------- |
| `summary()` 与**独立手写 SQL** 逐项一致 | token / 已知成本 / 事件数 / usage sessions 全等               |
| 各维度之和 == 全局                      | harness、model 分组之和都等于全局 token                       |
| `timeline 之和 + 无时间戳事件 == 全局`  | 无时间戳的事件进不了任何一天，因此必须单独计数                |
| 关库重开                                | `summary` 全字段相等（数字完全由 SQLite 恢复）                |
| 时间窗随偏移变化                        | UTC+8 与 UTC 的「今天」不是同一个 UTC 窗口                    |
| `usage_sessions` ≠ `managed_sessions`   | 222 vs 0 —— 外部历史有 222 个会话，Harness Hub 一个都没启动过 |
| `cost_is_lower_bound`                   | 8 条 `missingPricing` 记录 → true，金额因此是下界             |

### 「渲染路径不起进程」怎么证明的

两层证据，都不是靠嘴说：

1. **结构上**：`summary(&Connection)` 的签名里没有 runner / executor / adapter，
   编译期就不可能启动外部命令；
2. **行为上**：`DashboardPage.test.tsx` 的桩**按命令名分派、遇到未预期命令直接抛错**，
   并断言 mount 与切换范围期间只出现 `usage_summary`（外加 `app_info` / `db_health`），
   `refresh_usage` 只在用户点「刷新用量」之后才出现。

### 仍未验收

- **没有做 GUI 截图级验收**：本轮证据全部来自 Rust 真机 E2E 与前端组件测试
  （jsdom + 真实 IPC 契约形状）。没有像 Task 4 那样启动 `pnpm tauri dev` 并抓取真实窗口，
  因此「应用里看到的数字」这一点尚未在真实 WebView 中复核。
- 固定时区偏移不建模 DST（ADR-0012 决策五已记录该局限）。

---

## Task 6 Release Gate：真实 GUI 验收（2026-09-23）

方法：真实 `pnpm tauri dev`（WebView2 + CDP 9222），用 DevTools Protocol 读取 Dashboard 的
真实渲染文本、点击真实按钮，并用 Python 的 `sqlite3`（**与 Rust 不同工具链**）独立读同一份
应用数据库做交叉核对。**没有为验收增加任何生产 debug API。**

### 启动即完成真实迁移（5 → 7）

应用数据库启动前 `schema_version = 5`（`usage_events` 还是旧形状、0 行），启动后：

```text
migrations: 0001 … 0005, 0006_usage_imports_and_events, 0007_usage_import_unattributed_tokens
schema_version 7    usage_events 0    sessions 13
GUI: Schema 版本 7 / Managed Sessions 13 / Tokens 0 / Known Cost —
```

0006 的「旧表非空则拒绝执行」守卫在真实库上正常通过（旧表确实是空的）。

### GUI 与 SQL 逐项一致（点一次「刷新用量」之后）

点击刷新后 `usage_imports` 恰好 **1 行**（`succeeded`, inserted 243, skipped 0），
应用数据库：`243` 事件 / `4,757,843,285` tokens / `288,824,073` 微单位 / `8` 条缺价格 /
`usage_sessions 222` / `managed_sessions 13`。

| 范围  | GUI Tokens        | GUI 成本      | GUI Usage Sessions | GUI Managed | GUI 事件数 | SQL 侧                                 |
| ----- | ----------------- | ------------- | ------------------ | ----------- | ---------- | -------------------------------------- |
| 今天  | 0                 | —             | 0                  | 3           | 0          | 空状态，符合预期（E2E 预测 Today = 0） |
| 7 天  | 126,998,986       | ≥ $14.41      | 24                 | 13          | 27         | 子集                                   |
| 30 天 | 4,641,236,542     | ≥ $227.59     | 186                | 13          | 204        | 与 Rust E2E 的「30 天 204 条」一致     |
| 全部  | **4,757,843,285** | **≥ $288.82** | **222**            | 13          | **243**    | **与 SQL 基准逐项相同**                |

要点逐条核对：

1. **成本带 `≥`**：全部范围显示 `≥ $288.82` 与「部分记录缺少价格（8 条），实际成本可能更高」，
   **没有**显示成精确金额；`288,824,073` 微单位 ≈ $288.824 → `≥ $288.82` 语义一致。
2. **两个会话 KPI 独立**：`Usage Sessions 222` 与 `Managed Sessions 13` 是两个独立卡片，
   各自的说明文字也写明了区别，未合并。
3. **breakdown 正常**：Harness 分布 codex 3,741,061,325 / zcode 899,850,303（无价格显示 `—`）/
   opencode / claude；Model 分布按明细列出 8 个模型；Project 分布显示「尚未把用量关联到项目
   （不推断、不伪造归属）」。
4. **切换范围真实重查**：四个范围的数字如上表，`今天` 正确落入空状态。
5. **不起进程**：切换 4 个范围期间（轮询 20 次）与刷新页面期间（轮询 12 次），
   `ccusage@` / `ccusage\src\cli.js` / `ccusage session` 进程命中 **0 次**。
6. **重启持久化**：强制关闭应用 → 确认无 ccusage/npx 残留、`harness-hub` 进程数 0、
   数据库仍是 243/…/imports 1 → 重新启动（**未点刷新**）→ 四个范围的数字与关闭前**逐字相同**
   （30 天 4,641,236,542 / 186 / 204；全部 4,757,843,285 / 222 / 243），
   期间 ccusage 命中 **0 次**，`usage_imports` 仍是 **1 行**（证明确实没有隐式 import）。
7. **刷新是唯一写入口**：点击刷新时进程审计抓到
   `npx.cmd --yes ccusage@20.0.24 --version` 与
   `npx.cmd --yes ccusage@20.0.24 session --sections daily --by-agent --json`
   → `…\ccusage\src\cli.js session --sections daily --by-agent --json`（**没有 `latest`**），
   成功后 UI 显示「已读取 243 条记录（新增 243，更新 0，未变 0）」并重新查询 SQLite。

### 真实 GUI 暴露并修掉的缺陷

`<CardDescription>` 里写了 markdown 风格的 `按**本地日**…`，JSX 不解析 markdown，
于是用户看到的是字面星号。已改成 `<span className="font-medium">本地日</span>`，
并**重新启动真实应用确认**渲染为「按本地日（时区 480 分钟）聚合。」且页面不再出现 `**`。

### 验收环境事故（非产品缺陷）

验收过程中在 `pnpm tauri dev` 运行期间执行 `prettier --write`，prettier 在
`src/features/dashboard/` 下创建临时目录，Vite 文件监听在它上面 `EBUSY` 崩溃（dev server 退出）。
这是工具链竞争，不是 HarnessHub 的缺陷；但**开发时不要边跑 dev server 边格式化源码**。

### 与基线不一致的一处，必须解释

用户给的基线里 `managed_sessions = 0`，真实应用数据库是 **13**。原因：基线来自 Task 5/6 的
**临时库 E2E**（全新空库，Harness Hub 一个会话都没启动过）；应用数据库里有 Task 3/4 真实启动过的
13 条会话。两个数字都正确，恰好说明 `usage_sessions`（222，来自外部历史）与
`managed_sessions`（13，Harness Hub 自己管理的）确实是两个不同的事实。

---

## Task 7B GUI 取证轮（2026-09-23，真实 WebView2 + CDP）

方法：真实 `pnpm tauri dev`，用真实 UI 路径驱动（导航链接 → selector → 启动按钮 → xterm 输入 →
结束会话按钮），不做 hash 注入、不加任何 debug API。证据形式是**提交前/提交后分界**
（答案 `585987` 从未被输入过，因此它的出现只能来自 Claude 输出）。

### 已经拿到的（真机输出）

```text
selector options: [{value:"codex@local",label:"Codex"},{value:"claude@local",label:"Claude Code"}]
picked        : claude@local          ← 真实选中第二个 Harness
start         : 真实点击「启动」按钮
after start   : 屏幕 545 字符，selector disabled=true（运行中禁止切换）
before submit : answerPresent=false   promptPresent=false   ← 基线干净
after kill    : 「已结束 · 用户主动结束 · 退出码 1」
DB            : status=exited  termination_reason=user_killed  exit_code=1  pid=40780
running(claude)=0   running(all)=0    claude 残留进程=0
```

**GUI kill 的终态语义完全正确**（`user_killed` + reaper 观察到的真实退出码 1；非零退出码按约定
不算失败，只记录事实），且**零 ghost running、无孤儿进程**。

### 未完成：算术往返（因此 `terminal` 仍不能翻 true）

Claude 首屏**不是聊天界面**，而是目录信任确认：

```text
Accessing workspace: C:\Users\tangy
Quick safety check: Is this a project you created or one you trust? …
```

因此我发去的 prompt 没有进入输入行（`promptPresent=false`）；回车把这个确认界面关掉之后
才出现聊天界面（`> ` 加 `? for shortcuts`），但那段文字已被对话吃掉。

按约定这**不写成测试绕过真实行为**，而是如实记录该 TUI 状态，下一轮用正常 GUI 交互
（在确认界面选择「信任并继续」）进入可输入状态后再提交 `314159 + 271828` 并只检查
基线之后的新输出。

需要用户确认的副作用：选择「信任」会**持久写入 Claude 的用户配置**（`~/.claude.json` 的
信任列表），且本次 cwd 是用户主目录 `C:\Users\tangy`。这是真实产品行为，但属于对用户机器
的持久配置改动，因此先停下确认，而不是替用户点下去。

---

## Task 7B 最终 GUI 验收（2026-09-23，纯 UI 路径，`RESULT: PASS`）

启动参数：Terminal Launcher → `Claude Code` → cwd = `D:\HarnessHub-E2E\claude-terminal`（仓库外专用目录）。
唯一允许的持久副作用：`~/.claude.json` 记录**该窄目录**的 trust（未信任用户主目录）。

```text
form          : picked=claude@local  cwd=D:\HarnessHub-E2E\claude-terminal
firstScreen   : workspaceShown=true  homeShown=false
                运行中 · Claude Code · cwd D:\HarnessHub-E2E\claude-terminal · pid 45684 · session d0646772…
                "Accessing workspace: D:\HarnessHub-E2E\claude-terminal"
                "Quick safety check: Is this a project you created or one you trust? … > 1. Yes, I tru…"
chatReady     : sawShortcuts=true（真实按键通过 trust 后进入聊天界面）
baseline      : answerPresent=false（干净边界）
typed         : promptPresent=true
afterSubmit   : answerPresent=true  grewBy=435   ← 585987 只出现在提交之后的新输出里
afterKill     : 已结束 · 用户主动结束 · 退出码 1
DB            : status=exited  termination_reason=user_killed  exit_code=1  pid=45684
running(claude)=0   running(all)=0   pid 45684 存活=false   claude 残留进程=0
```

这条证据同时覆盖了：

1. **cwd 的完整链路**：UI → IPC → `LaunchSpec` → PTY，由 Claude 自己的 TUI 显示出来验证（不是我们自说自话）；
2. **多阶段交互式 TUI**：trust 确认页 → 真实键盘选择 → TUI 状态转换 → 聊天界面；
3. **真正的双向 Terminal**：xterm 输入 → PTY → Claude → 原始输出 → Channel → xterm，且答案 `585987` 从未被输入过，因此「提交前没有 / 提交后有」排除了输入回显假阳性；
4. **GUI kill** 的终态语义与零孤儿。

据此（且仅据此）翻转 `Claude: launch=true, terminal=true`；`resume` 等仍为 false，并由
`the_capability_matrix_is_exactly_what_has_been_accepted` 锁死。

---

## Task 7B.1：Claude 剩余 lifecycle 取证（2026-09-23，headless 真机）

每个场景**新开一个 Session**，因此 DB 链互不污染；S3/S4 调用的是**生产收敛函数**
`SessionService::reconcile_orphans`（启动时与 `RunEvent::Exit` 走的就是它）。

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test claude_lifecycle -- --nocapture --test-threads=1`

```text
S1 user_killed : exit_code=Some(1)                     ← 非零退出码按约定只记事实
S2 natural_exit: exit_code=Some(0)                     ← 发 /exit，未调用 kill_terminal
S3 host_shutdown: status=Unknown + host_shutdown + running=0
S4 lost        : status=Unknown + lost + running=0，pid=45860（PID 是否存活不影响状态）
                 且二次收敛为 0（幂等）
4 passed
```

### 仍未取证的 7B.1 项

**真实 GUI resize/reflow**：`terminal=true` 的验收契约里包含这一条，目前只在 Codex 侧
观察过（Task 4：窗口 maximize/restore → viewport 1280↔1707、xterm screen 972↔1401），
Claude 侧尚未做同样的观察。下一轮补：窗口尺寸变化 → WebView viewport → xterm layout →
Claude TUI reflow（不新增 debug API）。

### 7B.1 最后一块：Claude 的真实 GUI resize/reflow（2026-09-23）

形式沿用 Task 4（真实窗口尺寸变化 → viewport → xterm layout → TUI 重绘），不读精确
cols/rows、不加 debug API。触发方式：`ShowWindow(SW_MAXIMIZE/SW_RESTORE)`（Task 4 已确认
`SetWindowPos` 不会触发 WebView2 reflow，ShowWindow 才会）。

```text
before: innerWidth 1280  container 992x480  screen 972x475  rule 136  text 1766
after : innerWidth 1707  container 1419x480 screen 1401x475 rule 196  text 1964
changed: innerWidth=true container=true screen=true ruleWidth=true text=true
RESULT: PASS
xterm 首行: ╭─── Claude Code v2.1.126 ───…
```

`container/screen` 宽度与 TUI 里那条横线的长度（136 → 196）同时变化，说明**不只是容器变大，
Claude 自己也按新宽度重绘了**。至此 `terminal=true` 的验收契约（真实 TUI + 双向交互 + resize）
全部补齐，7B/7B.1 彻底关账。

---

## Task 7D：Cross-Harness Concurrency（2026-09-23/24）

7D 要证明的不是「Codex / Claude 各自能跑」（7A–7C 已各自取证），而是**同时跑时互不污染**。
分三层取证，避免一次真机测试承担太多责任：

```text
7D-A-i  确定性并发隔离矩阵（fake PTY backend，进 CI）
7D-A-ii 真实 PTY 无头并发（真 codex + 真 claude，两个不同 cwd）
7D-B    真实 GUI 双会话（真实 WebView2 + CDP；同一 App 实例里同时持有两条会话）
```

### 7D-A-i 确定性矩阵

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib terminal::concurrency_tests`

```text
running 7 tests ... test result: ok. 7 passed; 0 failed
```

断言（全部在**生产的** `TerminalRuntime` 上，fake 只替换 PTY 传输层）：两个 session 同时
running、session_id / pid / installation / cwd 各自独立且不同、DB 两行；输出/输入/resize
隔离（fake 精确记录 `(session_id, cols, rows)`）；kill 两个方向都只影响目标会话且另一条
仍能 write / resize / **继续产出**；两条会话退出码与终态各自落库互不覆盖；两条 running 一次
收敛（`converge == 2`，不是只处理第一行）且二次收敛为 0。

> 这条矩阵的第一个 RED 值得留档：最初用 fake 的**共享**输出队列起两条会话时，
> `codex stream` 里出现了 `CLAUDE_ONLY_27182` —— 一个不区分会话的传输层会让「隔离」测试
> 说谎。修复点在夹具（生产 `PortablePtyBackend` 本来就按 session_id 存），断言自始至终没变。

### 7D-A-ii 真实 PTY 无头并发

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test two_harness_concurrency -- --nocapture --test-threads=1`

```text
running 3 tests ... test result: ok. 3 passed; 0 failed; finished in 21.56s

codex : installed=true  binary=D:\npm-global\codex.cmd  version=0.152.1
claude: installed=true  binary=D:\npm-global\claude.cmd version=2.1.126
[1] 同时运行: codex pid=38560 claude pid=36008        ← 两个真实 child 同时存在
[2] 首屏 bytes: codex=270 claude=61 （DSR 应答: codex=1 claude=1）
[3] marker 回显: codex=true claude=false
[4] resize codex → codex 重绘=true，claude 仍 running 且进程存活
[5] kill claude → codex 仍 running，继续产出=true（bytes 36801 → 44875）
[6] 终态: claude exit=Some(1) codex exit=Some(1) running=0
[反向] kill codex → claude 仍 running（重绘=true，bytes 1127 → 3075，pid 38768 存活）
[ghost] 两条 running 都收敛为 unknown/lost（converge == 2，二次收敛 == 0）
```

进程观测刻意**独立于 PTY**：用 `tasklist` 查 PID，而不是只问 portable-pty 的 child 状态。
marker 隔离的**反向**断言是硬的（对方 stream 一定不含我的 marker）；正向（自己的 marker 回显）
受被测 TUI 行为影响，只如实记录 —— 这一次 Codex 回显、Claude 没回显（它停在**未信任目录**的
trust 确认页，headless 不替它做信任决定）。

### 7D-B 真实 GUI 双会话（`RESULT: PASS`）

方法：真实 `pnpm tauri dev`（`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`），
用真实 UI 路径驱动（导航链接 → selector → cwd 输入 → 启动 / 结束会话按钮）：
不置顶窗口、不动鼠标键盘、不做 hash 注入、不加任何 debug API。
数据库交叉核对用 **Python 标准库 `sqlite3`**（与 Rust 不同工具链）读应用库快照
（WAL：必须连 `-wal` / `-shm` 一起复制）。

```text
# 1) GUI 自己启动的真实会话
picked    : codex@local  cwd=D:\HarnessHub-E2E\codex-concurrent
running   : 运行中 · Codex · cwd D:\HarnessHub-E2E\codex-concurrent · pid 23788 · session 6de9354d…
xterm 屏幕: 「gpt-6-luna max · D:\HarnessHub-E2E\codex-concurrent · Workspace · Context 100% left …」
            ← Codex 自己的 TUI 把 cwd 显示出来，UI → IPC → LaunchSpec → PTY 全部走通
DB(Python): status=running pid=23788 cwd=D:\HarnessHub-E2E\codex-concurrent

# 2) 与此同时无头侧也在跑真实的 Codex + Claude（同一生产 TerminalRuntime 类）
headless  : codex pid=28168  claude pid=44280
同时存活  : pid 23788 ALIVE / 28168 ALIVE / 44280 ALIVE   ← 三个真实 child 同一时刻都在
headless  : test result: ok. 3 passed; finished in 24.83s（含两个 kill 方向与两条 ghost 收敛）
GUI       : 全程仍显示「运行中 · Codex … pid 23788」

# 3) GUI 结束会话（真实按钮）
after kill: 已结束 · 用户主动结束 · 退出码 1
            pid 23788 dead；DB: status=exited termination_reason=user_killed exit_code=1 ended_at=…；running=0

# 4) 同一 App 实例里同时持有两个 Harness 的会话
A=codex  pid 11964 session 73b78cc5… cwd codex-concurrent
B=claude pid 12480 session 57665d03… cwd claude-terminal
（Terminal 页只持有一个 current session；导航离开**不会** kill 旧会话 —— 这是既有产品语义）
DB: running = 2，两行分别是 codex / claude，各自 pid 与 cwd 正确；两个 child 同时存活
GUI 屏幕: Claude 聊天界面（`╰────…` 边框 + `> ` + `? for shortcuts`）

# 5) 两个方向的 kill 隔离（都在真实 GUI + 真实 App 库上）
kill Claude(B) → pid 12480 dead；DB: claude exited/user_killed/exit 1；codex(11964) 仍 running 且进程存活
B2=claude pid 39056 session 89988a0e…（重新启动）；C=codex pid 31656 session 700e27e1…（新页面启动）
kill Codex(C)  → pid 31656 dead；DB: codex exited/user_killed/exit 1；claude(39056) 与 codex(11964) 仍 running

# 6) 强杀 Harness Hub → 重启（两条 running 都是 ghost）
before     : running = 2（codex 73b78cc5… + claude 89988a0e…），两个 child 同时存活
Stop-Process -Name harness-hub -Force
after kill : 库里仍是 running = 2（真实 ghost 残留）；两个 child 进程随 ConPTY 一起消失
restart    : 启动收敛：2 条遗留 running 会话被标记为 lost
             两条旧 running → status=unknown + termination_reason=lost + ended_at 写入，running = 0
             已 user_killed 的历史行**未被碰**（终态不被收敛覆盖）

# 7) Dashboard（同一个真实应用实例，未点刷新用量）
Managed Sessions 22 == SQL 侧 sessions 总数 22
Harness 分布: codex 3,741,061,325 / zcode 899,850,303 / opencode 217,534 / claude 107,380
```

**本轮开始时还顺手拿到一条真实孤儿证据**：这次应用的第一次启动打印
`启动收敛：1 条遗留 running 会话被标记为 lost`，被收敛的是**上一个应用实例**遗留的
claude 会话（`c0811428…`，cwd `claude-terminal`，13:57:49Z 开始）—— 不是构造出来的行。

没有新增持久副作用：headless 用的两个 `*-concurrent` 目录**没有**写进 `~/.claude.json`
的信任列表（只有 7B 已同意的 `claude-terminal` 那一条）。

### 7D 发现的两个问题（都不在 7D 范围内修，但必须记账）

1. **产品 kill 只终止直接子进程**（harness-agnostic）。
   证据：无头用例在产品 kill 之后仍然查到 shim 的 `node.exe` 子进程在跑
   （`[finding] … 12764 的 node 子进程仍存活：[39036]`，Codex / Claude 两侧都出现）。
   影响：`user_killed` 终态正确，但机器上会留一个 Harness node 进程。
   测试自己用 `taskkill /F /T` 定点清理（只杀自己记录过的父 PID 的子树，绝不安杀所有 node）。
   建议后续：用 Job Object（或 Windows 上等价的树终止）保证「会话结束 = 进程树结束」。
2. **`PortablePtyBackend::write` 持锁写**（`sessions` mutex 覆盖 `write_all` + `flush`）。
   观测：7D-A-ii 的第一版实现让测试**静默挂死 15+ 分钟**，测试进程 CPU ≈ 0，
   两个 Harness 的 shim 都还活着（说明 kill / reap 从未拿到锁）。触发条件是测试按
   「历史上出现过多少个 `ESC[6n`」**无限重放** DSR 应答，把 Claude 的 ConPTY 输入缓冲区灌满，
   写入阻塞 → 整个 backend 被这把锁冻住 → **连 kill 都进不去**。
   机制结论来自「观测 + 读代码」，没有拿到线程栈，因此按假设记录。
   对生产的含义：某条会话的子进程一旦停止读 stdin，整个 runtime（含「结束会话」这个唯一的
   恢复手段）都可能冻住。7D 未改产品行为；测试侧改为**输入只从独立线程写** + DSR 应答
   每会话上限 3 次，之后同一场景连续多次通过。
   建议后续单独起 Task：写入不得持全局会话锁（每会话锁 / 输入队列 / 非阻塞）。

### 7D 明确**未**证明的事

- 无头真实进程下**正向** marker 回显不稳定（Claude 停在 trust 确认页），因此只有反向
  「对方 marker 不出现」是硬断言；正向由确定性矩阵（7D-A-i）硬断言兜住。
- 真实 PTY 上**不读精确 cols/rows**（无 readout）：`(session_id, cols, rows)` 的精确断言
  在 fake backend 那一层。
- 没有新增 multi-tab / split terminal，因此同一 App 实例里只有**当前**会话可由 UI 操作
  （第二条会话靠导航离开后仍存活来实现，见 [4]）。
- `hub_session_id` 与 ccusage 裸 UUID 的关联仍然不可证明，7D 不碰（ADR-0011 决策八）。

---

## Task 8A：Runtime 输入路径 — 跨会话锁下不得有阻塞 I/O（2026-09-24）

### 要修的东西（7D 发现的第 2 条）

```text
PortablePtyBackend::write（旧）
  lock sessions            ← 全局 map 锁
  session.writer.write_all ← 子进程不读 stdin 时**永久阻塞**
  unlock
```

一条会话的阻塞写会把 `resize` / `kill` / `try_wait`（reaper）全部冻住 —— 连「结束会话」
这个唯一的恢复手段都进不去（7D 真机实测：15+ 分钟静默挂死，测试进程 CPU ≈ 0，
两个 Harness 的 shim 都还活着）。

冻结的不变量（spec：`docs/specs/2026-09-24-task8a-runtime-input-path-design.md`）：

```text
INV-1  任何可能阻塞的 OS I/O 不得发生在跨会话锁持有期间
INV-2  write_terminal 不得阻塞在子进程 stdin 上
INV-3  kill / try_wait / resize / take_reader 不得依赖 writer 的锁
INV-4  全局 sessions 锁只用于「查找 → clone Arc / 插入 / 删除」
```

### 确定性矩阵（fake backend，每次提交都跑）

Run: `cargo test --manifest-path src-tauri/Cargo.toml --lib pty::input_tests`

```text
running 12 tests ... test result: ok. 12 passed; 0 failed（0.1s 量级，全部靠 Condvar 同步，无 sleep 猜时序）
```

| 用例                              | 锁死的事实                                                                             |
| --------------------------------- | -------------------------------------------------------------------------------------- |
| 单批 > 容量（默认 & 注入 8 字节） | 空队列也整批拒绝                                                                       |
| in-flight 记账                    | worker 阻塞在写里时 `pending_bytes == 8`；容量不可能被「pop 后重填」绕过               |
| 无 partial enqueue                | 被拒批次一个字节都不落地；放行后 backend 只收到被接受的批次                            |
| FIFO                              | 三批按序写入                                                                           |
| A 阻塞写不影响 B                  | B 的 write/resize/kill **以及 A 自己的 resize/kill/try_wait** 全部 < 2s 返回           |
| shutdown 不 join                  | worker park 在阻塞写里时 `forget` 仍立即返回（硬期限锁死）                             |
| worker 失败语义                   | 输入侧永久失败 + 丢弃未发送 + `pending_bytes` 归零；**不发事件、不写终态**；控制面照常 |
| 错误优先级                        | closed → failure → capacity（worker 先失败、随后 kill 关闭时报 `InputClosed`）         |
| kill / forget 交互                | kill 成功才关输入侧；kill 失败不关；forget 之后写入被拒                                |
| reaper 释放                       | 退出后释放 handle + 调 `backend.forget`，且**顺序**是「先写终态、再回收资源」          |
| E1 reader EOF                     | EOF **不**触发资源回收；handle 还在，`is_running` 仍为 true                            |

两个 RED 值得留档（都是先看到失败才改的实现）：

```text
1) 队列第一版在 pop 时就减 pending_bytes
   → 16 字节容量实测能塞进约 2 倍（in-flight 没被计入）→ 改成只在 backend.write 返回后结算
2) 失败态第一版把 failure 检查放在 closed 之前
   → 显式 kill 之后仍报 worker 失败 → 按 spec §4.4.1 调成 closed 优先
```

### 真机验收（synthetic child 永不读 stdin）

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test runtime_input_backpressure -- --nocapture`

```text
test a_real_child_that_never_reads_stdin_does_not_freeze_the_runtime ... ok
test the_last_output_is_not_truncated_when_the_reaper_releases_the_session ... ok
test result: ok. 2 passed; 0 failed; finished in 6.58s（连续多次同结果）

[R2] A 背压写入 返回耗时 = 378.4µs      ← A 的 writer 此刻真的 park 在 OS write 里
[R2] B write   返回耗时 = 264.4µs
[R2] B resize  返回耗时 = 280.3µs
[R2] B kill    返回耗时 = 442.4µs
[R2] A resize  返回耗时 = 233.6µs      ← 卡住的那条会话自己也能被 resize/kill
[R2] A kill    返回耗时 = 370.7µs
[R2] A try_wait 返回耗时 = 175.4µs
```

```text
R1  4 MiB 批次被接受 → 之后写入得到 InputBackpressure；
    2 秒后 pending_bytes 仍是 4 MiB —— 这同时**自证** child 真的不读 stdin
R2  上表：六个控制调用全部在约 160–450 微秒内返回（旧实现是 15+ 分钟挂死）
R3  kill 之后写入得到 InputClosed；synthetic child 全部清理干净
R4  自己退出的 child（打印唯一 marker 后退出）→ reaper 走完「真实退出 → 终态 →
    有界等待 reader → 释放」→ marker 完整出现在收集到的输出里（尾部不截断）
```

### 写这条测试时被机器教的三件事（都已写进测试注释）

```text
1. 容量必须**大于 ConPTY 的输入缓冲区**：64 KiB 会被 OS 一次性吞掉，write_all 根本不阻塞，
   那样就没在测「park 在 OS 写里」。实测取 4 MiB 才能稳定造出阻塞写。
2. `cmd /c echo` 在 ConPTY 下**不会退出**（GetExitCodeProcess 与 tasklist 都确认它 15 秒后仍活着），
   所以「自然退出」用例改用绝对路径的 Windows PowerShell（CreateProcess 不搜 PATH）。
3. 短命 console child（cmd / PowerShell）和 Claude 一样，**先发 ESC[6n 并等待应答**
   （实测：只收到 4 字节 ESC[6n 就永远不动）。R4 因此需要 test-only DSR responder；
   应答本身走 manager.write，也就是新的队列 —— 顺带把输入路径端到端过了一遍。
```

### 8A 的承诺边界（写进 spec §7，不许含糊）

```text
已证明：A 的 writer 永久 park 时，整台 Runtime、B 会话、以及 A 自己的 kill/resize/reap
        都不冻结（确定性 + 真机两层）。
未证明：回收那条被 OS syscall 卡死的 writer 线程本身 —— 这属于 Task 8B 的
        process-tree ownership（Job Object / 受控 tree termination）。
```

### 回归与 Gate

```text
7D 确定性矩阵       7 passed（新队列路径 + 新锁结构下仍全绿）
7D 真机并发         3 passed
claude_lifecycle    4 passed（user_killed / natural_exit / host_shutdown / lost）
real_codex_terminal 2 passed
pnpm verify         退出码 0：Rust 单测 276 passed、全部集成套件、前端 124 passed、
                    build、clippy -D warnings、格式检查；act() 警告 0
pnpm python:test    Ran 12 tests ... OK
Core diff 审计      只落在 pty/*、error.rs、测试与文档；pty/ 与 error.rs 里 codex/claude
                    只出现在 #[cfg(test)] 测试数据中；commands.rs / src/lib / src/features
                    零改动（无新 IPC DTO、UI 无新状态）
```

---

## Task 8B：进程树所有权 / 受控终止（2026-09-24）

契约：`docs/specs/2026-09-24-task8b-process-tree-ownership-design.md`（§3 冻结语义、
§6 已知限制、§7 测试设计）；实施：`docs/plans/2026-09-24-task8b-process-tree-ownership.md`；
平台保证等级：`docs/adr/0013-windows-process-containment-establishment.md`。

### 要解决的问题

7D 的遗留问题（8A 只解决了其中一半）：`.cmd` 的**直接子进程**是 `cmd.exe`（shim 解释器），
真正干活的是它的后代（`node.exe` / `codex.exe` / `claude.exe`）。8A 让「阻塞写不再冻结
Runtime」，但没有解决「**Session 拥有的进程树还可能活着**」。

冻结语义：**Session 拥有的是进程树，不是一个 PID**。`kill_terminal(session_id)` 终止整棵
被拥有的树；containment 在进程变成 `running` **之前**建立，失败即 `launch_failed`。

### spike 证据（真机，`D:\HarnessHub-E2E\spike-8b\`，throwaway，代码不入库）

```text
s1  job.terminate → synthetic 三层树 3/3 全灭
s2  session-scoped：只杀 A 的 job，B（same_exe=true）3/3 存活；第一版先起后 assign 丢了 race，
    只杀死 root —— 这就是「立即 assign + 定点补扫」的来历（ADR-0013 如实记为**非** race-free）
s3  真实 codex.cmd：cmd.exe → node.exe → codex.exe（job.active_processes = 3）
s4  真实 claude.cmd：cmd.exe → claude.exe（2 节点）
s6  宿主 `taskkill /F` 强杀 → 它拥有的 3 个进程全部被 OS 回收（KILL_ON_JOB_CLOSE）
s7  5/5 轮 assign 成功（本机、同用户、同完整性级别）
```

**s5 三档对照（负面结论，定义了 8B 的两条独立路径，载荷 64 MiB）**：

```text
shim-only(child.kill)              动作后：shim dead / ping ALIVE，写仍然 park（15s 未返回）
whole-tree(job.terminate)          动作后：树全 dead，**写仍然 park**（15s 未返回）
whole-tree + drop master           动作后：树全 dead，写返回 Err(管道已结束。 (os error 109))，约 0.9s
```

结论：**进程包含 ≠ 传输关闭**。「进程还活着」由 `terminate_tree` 修；
「阻塞 writer 醒不过来」只能由 `close_transport`（关 PTY master）修。两者都在 `forget` 里。

### 确定性矩阵（fake backend，每次提交都跑）

```text
T1  未建立 containment 不得进入 running：spawn 失败 → failed + launch_failed + 无 pid，
    错误信息带具体 os error（不静默降级成「没有 containment 也在跑」）
T2  kill 走的是 terminate_tree（断言 backend 收到 terminate_tree 调用），kill 成功后输入侧关闭
T3  terminate_tree 失败 → 不关闭输入侧、不写终态（沿用 8A 形状）
T4  forget 先关输入侧（8A 顺序不变）
T5  reaper 顺序不变：终态 →（有界等待 reader）→ shutdown_input → 移除 handle → backend.forget
T6  没有 containment 的会话 terminate_tree 必须报明确错误，**不得**降级成 direct-child kill
```

### 真机单测（`pty::portable_pty_backend` / `pty::containment`）

```text
containment::a_job_contains_the_child_and_its_descendants                    ok
containment::closing_the_last_handle_kills_the_contained_tree                ok
containment::assign_reports_the_win32_error_for_an_unopenable_process        ok
portable_pty_backend::forget_actively_closes_the_transport_so_a_parked_write_returns  ok
portable_pty_backend::forget_releases_the_job_even_while_a_writer_holds_the_arc        ok
portable_pty_backend::release_after_a_natural_root_exit_still_reaps_living_descendants ok
```

### 真机 Gate（`src-tauri/tests/process_tree_ownership.rs`，8 条）

Run: `cargo test --manifest-path src-tauri/Cargo.toml --test process_tree_ownership -- --nocapture`

```text
[R2/R3] hub-codex tree = [4984, 50716, 3408]        ← cmd.exe → node.exe → codex.exe
[R2/R3] hub-claude tree = [22444, 46684]            ← cmd.exe → claude.exe
[R4] A=[55728, 48380, 51140] B=[58084, 51800, 50032] C=[56308, 50936]
[R6] tree = [23208, 55408]
[R1] tree = [57544, 46572, 14672]
[R7] production runtime tree = [41864, 52356, 12584]（DSR 应答 1 次）
[R4/raw] A=[36824, 51388, 42824] B=[30312, 52820, 29360] C=[56324, 25912]
[R1/raw] tree = [53980, 58212, 43356]
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 51.12s
```

```text
R1      synthetic cmd → cmd → ping：kill → 3 个 PID 全部消失
R2/R3   真实 codex（3 层）/ claude（2 层）：kill → 全树消失（支撑证据；缺 binary 时明确跳过）
R4      Codex A + Codex B + Claude C 同时运行 → kill A → A 全死、B/C 的**活节点**全活
        （A/B 同一个可执行文件：executable-scoped 的错误清理也骗不过这条）
R5      writer park 在 OS 写里（pending_bytes 持续等于整批 64 MiB）→ kill → 树全死 →
        reaper 走完终态 + forget → handle 被回收（有界 15s 观察，不 join writer）
R6      不先 kill、直接 forget：残留在 Job 里的后代被收敛（措辞边界见下）
R7      生产 TerminalRuntime 路径：真实 codex 树 ≥ 3 层 + kill 后全树消失
R1/raw  只用 PortablePtyBackend + terminate_tree（**没有任何 forget 兜底**）→ 全树消失
R4/raw  同上，A/B/C 三条会话 → terminate_tree(A) → A 全死、B/C 的活节点全活
```

### 变异验证（本 Task 最有价值的一段：证明断言真的在测东西）

```text
A) terminate_tree 退回 direct-child kill
   R1/R2/R3/R4（走 PtyManager::kill）**照样全绿** —— kill 之后 reaper 会 forget，
   残留后代会死在「关 console + 释放 Job 句柄」那一步。
   所以它们只是**产品级**结论；判别性证据是 R1/raw 与 R4/raw：
     R1/raw 失败：terminate_tree 必须终结整棵树（这里没有任何 forget 兜底），
                  仍在跑：[50500, 30040]
     R4/raw 失败：A 的全树必须死，仍在跑：[55388, 49372]
B) forget 里去掉 close_transport()
   portable_pty_backend::forget_actively_closes_the_transport_so_a_parked_write_returns
   在 13s 内失败于「forget 之后 parked write 必须返回」
C) forget 里改成就地取走但不释放 Job 句柄（mem::forget 泄漏）
   「后代被回收」断言**照样通过**（ClosePseudoConsole 自己会带走挂在同一 console 上的后代），
   但「containment 必须已被显式取走」断言失败 —— 这条写进了 spec 已知限制 L6
D) 关掉 fake 的 containment 检查
   pty::input_tests::a_session_without_containment_is_failed_not_running 与
   terminal::tests::start_fails_when_containment_cannot_be_established 双双失败
```

### 8B 过程中被机器教的三件事（都已写进测试注释）

```text
1. headless 测试必须自带终端模拟器：conhost 因 PSUEDOCONSOLE_INHERIT_CURSOR 会发 ESC[6n 并等应答，
   **不应答时 pseudoconsole 卡在初始化**，连 ClosePseudoConsole 都收不干净 ——
   表现为「master 已 drop、reader 拿不到 EOF、parked 写永不返回」。
   第一版测试没有应答，于是把这个夹具缺陷误判成「产品没关传输」。
2. 等树长出来的循环里必须**持续** pump DSR；漏掉 pump 时三条真实 TUI 全卡在首屏，
   症状是「树只有 root」，与 session-scoped 失败长得完全不一样。
3. session-scoped 断言要比较「terminate(A) 之前还活着的节点」，不能比较「与初始快照等长」：
   后者会把真实 TUI 自己退出的节点算成 A 的误杀（全量套件里踩到过一次）。
```

### 8B 的承诺边界（写进 spec §6 L1–L7，不许含糊）

```text
已证明：containment 在 running 之前建立（失败 = launch_failed + 具体 Win32 error）；
        terminate_tree 只终止该会话拥有的树（合成 + 真实 codex/claude 两层证据）；
        forget 显式关闭传输与唯一 Job 句柄（parked writer 持有 Arc 时也必须关）；
        root 自然退出后残留后代仍被收敛。
未证明：分配前 race 的数学消除（L1，ADR-0013 记为 best-effort 补扫）；
        宿主异常死亡以外的「任何宿主都必然 assign 成功」（R7 只证本机测试进程这条生产路径）；
        非 Windows 平台的 containment（L5）。
```

### 回归与 Gate

```text
pty::（含 8A 12 条 + 8B 6 条）      30 passed
terminal::（含 7D 确定性矩阵）      15 passed
process_tree_ownership（8B 真机）   8 passed（连续 4 轮同结果）
7D 真机并发                         3 passed
claude_lifecycle                    4 passed
runtime_input_backpressure          2 passed
pnpm verify                         退出码 0：Rust 单测 287 passed、全部集成套件、
                                    前端 124 passed（6 files）、build、clippy -D warnings、
                                    rustfmt --check
pnpm python:test                    Ran 12 tests in 0.001s ... OK
Core diff 审计（alpha.3..HEAD）     只落在 pty/*、terminal.rs（测试）、Cargo.toml、tests/、docs/；
                                    pty/ 里 codex/claude 只出现在测试数据与注释；
                                    commands.rs / src/lib / src/features 零改动
```

### 验收结论（2026-09-24）

验收人结论（原文）：

```text
Task 8B 的本地交付我认可。7 条 review 修正都有对应实现和证据，尤其 R1/raw、R4/raw
已能区分真正的整树终止与 reaper 兜底；分配前竞态等限制也记录清楚了。
我核对到工作区干净、alpha.3 之后是 30 个提交，tag 未移动。
你报告的测试结果作为本次验收证据；我这边重跑时被本机 Rust 工具链组件下载问题挡住，
测试没有启动，因此不冒称独立复验通过。远端仍未推送，网络恢复后再推送 main 和 tag。
```

必须跟着一起读的边界：

```text
1. 本次验收的证据是**实现者报告的真机输出**（上面每一段），验收人的独立重跑**未执行**
   （本机 rustup 组件下载受阻，cargo test 没有启动）。所以这是「本地交付已验收」，
   **不是**「第三方独立复验通过」。
2. 远端未推送：alpha.3..HEAD 的提交（验收人核对时为 30 个；写入本条验收记录后为 31 个）
   与 v0.1.0-alpha.3（annotated tag，6787aa1 → d1ff944，**未移动**）都只在本地。
   网络恢复后执行 `git push origin main --follow-tags`（tag 不重建、不改指向）。
3. spec §6 L1–L7 的已知限制随本次验收一并接受；其中 L1（分配前 race 只能 best-effort
   压到最小）是**能力边界**，不是待办缺陷。
```
