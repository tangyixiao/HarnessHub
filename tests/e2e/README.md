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
