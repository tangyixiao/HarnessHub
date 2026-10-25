# Harness Hub — CONTEXT MAP

> 目的：让 Agent **不要每次重新探索整个仓库**。先在这里定位，再读具体文件。
> 若你移动了目录或重命名了模块，必须同步更新本文件。

## 按任务找入口

| 我要做的事                         | 去哪里                                                                                                                                                                    |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Harness 检测 / 版本 / 能力 / 启动  | `src-tauri/src/harness/`（`adapter.rs` = trait，`registry.rs` = 注册表，`adapters/` = 具体 Harness）                                                                      |
| Usage 发现 / 调用 / 归一化 / 导入  | `src-tauri/src/usage/`（`adapter.rs` = trait，`ccusage.rs` = 解析+归一化，`runner.rs` = 外部命令，`importer.rs` = 幂等落库，`cost.rs` = 金额边界，`key.rs` = stable key） |
| Usage 契约与取证                   | `docs/adr/0011-usage-import-contracts.md`、`fixtures/ccusage/README.md`                                                                                                   |
| Session 生命周期与索引             | `src-tauri/src/session/`                                                                                                                                                  |
| SQLite schema / 迁移               | `src-tauri/src/db/`（`migrations/` 下为 SQL，`migrations.rs` 为执行器）                                                                                                   |
| 进程与 PTY                         | `src-tauri/src/process/`、`src-tauri/src/pty/`（Windows 进程包含 = `pty/containment.rs`，Job Object；真机 Gate = `src-tauri/tests/process_tree_ownership.rs`）            |
| Git / Worktree 观察                | `src-tauri/src/git/`（生产 `capture`；真实仓库测试 `src-tauri/tests/git_snapshot.rs`）                                                                                    |
| Project Registry（repo root/登记） | `src-tauri/src/project/`、真机测试 `src-tauri/tests/project_registry.rs`                                                                                                  |
| 文件监听                           | `src-tauri/src/watcher/`                                                                                                                                                  |
| Python Sidecar 生命周期 / JSON-RPC | `src-tauri/src/sidecar/`（Rust 侧）、`python/harness_hub_runtime/`（Python 侧）                                                                                           |
| Trace / Canonical Event            | `src-tauri/src/trace/`、`schemas/events/`                                                                                                                                 |
| 权限与策略                         | `src-tauri/src/permissions/`                                                                                                                                              |
| 插件宿主                           | `src-tauri/src/plugins/`、`plugins/sdk/`                                                                                                                                  |
| RuntimeTarget 抽象                 | `src-tauri/src/runtime/`                                                                                                                                                  |
| HHAR 导出                          | `src-tauri/src/hhar/`、`schemas/hhar/`                                                                                                                                    |
| Eval / 回归                        | `src-tauri/src/eval/`、`evals/promptfoo/`                                                                                                                                 |
| Tauri IPC 命令                     | `src-tauri/src/commands.rs`                                                                                                                                               |
| 前端路由与应用壳                   | `src/app/`                                                                                                                                                                |
| Dashboard                          | `src/features/dashboard/`                                                                                                                                                 |
| Harness 列表页                     | `src/features/harnesses/`                                                                                                                                                 |
| 项目页                             | `src/features/projects/`                                                                                                                                                  |
| Session / Terminal                 | `src/features/sessions/`、`src/features/terminal/`                                                                                                                        |
| 历史 / Activity Timeline           | `src/features/history/`、`src-tauri/src/activity/`（`query.rs` = 统一活动流只读投影，真机测试 `src-tauri/tests/real_activity_timeline.rs`）                               |
| 设置                               | `src/features/settings/`                                                                                                                                                  |
| 前端调用 Rust                      | `src/lib/ipc.ts`（唯一 IPC 入口，不要在组件里直接 `invoke`）                                                                                                              |
| Adapter 测试夹具                   | `fixtures/<harness>/`、`docs/adr/`                                                                                                                                        |
| 规格 / 设计 / 计划                 | `docs/specs/`、`docs/plans/`、根目录 `HARNESS_HUB_PROJECT_PLAN.md`                                                                                                        |

## 层与依赖方向（不可反向依赖）

Gemini 检测入口：`src-tauri/src/harness/adapters/gemini_cli.rs`；组合根在 `lib.rs` 注入
`GEMINI_CLI_HOME`。共享 IPC fixture 在 `fixtures/gemini/`，真机检测测试为
`src-tauri/tests/gemini_detection.rs`；终端生命周期证据为 `gemini_pty_spike.rs`、
`real_gemini_terminal.rs`、`gemini_lifecycle.rs`。Terminal 按 capability 过滤检测阶段适配器。
CodeBuddy 检测入口：`src-tauri/src/harness/adapters/codebuddy.rs`（home 直发现，
无 env 覆盖）；fixture 在 `fixtures/codebuddy/`，真机测试 `codebuddy_detection.rs`。
Copilot CLI 检测入口：`src-tauri/src/harness/adapters/copilot.rs`（PATH `copilot` +
`~/.copilot`，仅检测，能力全 false）；fixture `fixtures/copilot/`，真机测试
`copilot_detection.rs`；spec `docs/specs/2026-10-25-copilot-detection-design.md`
（真机版本输出 `1.0.91.`，尾点由共享 `parse_version` 剥离）。
OpenCode 检测入口：`src-tauri/src/harness/adapters/opencode.rs`（按 XDG 规范解析
data / config / state / cache 四个根，组合根 `lib.rs` 读一次环境注入
`OpenCodeXdgRoots`，适配器本身不读环境）；fixture 在 `fixtures/opencode/`，
真机测试 `opencode_detection.rs`。刻意不 shell out 到 `opencode debug paths`。
OpenCode 终端生命周期：启动零默认参数（不选 model / agent），复用共享 `program_for`；
证据为 `opencode_pty_spike.rs`、`real_opencode_terminal.rs`、`opencode_lifecycle.rs`。
Project Registry：`src-tauri/src/project/{mod,git_root,service,store}.rs` ——
`GitRootProbe` trait（`SystemGitRootProbe` 用 `git -C <path> rev-parse --show-toplevel`，
组合根 `lib.rs` 注入 `AppState.git_probe`）；`ProjectStore` 按 `repo_root` UNIQUE
幂等 upsert + touch `last_opened_at`；IPC 命令 register_project / list_projects /
remove_project / get_project_sessions；前端 `src/features/projects/`、
`src/lib/ipc.ts` Project 段；真机测试 `src-tauri/tests/project_registry.rs`；
spec `docs/specs/2026-10-12-project-registry-design.md`。
Git 只读快照与 Session 接线：`src-tauri/src/git/{mod,snapshot,runner,status,store,service,window}.rs`
—— `git::capture(&Path)` 返回 root/git_dir/common_dir、完整 HEAD 和四类文件计数；
runner 限时限量执行系统 Git，status 按 porcelain v1 NUL 字节协议解析；
`git::store`（migration 0010 `git_snapshots`，(session, phase) 主键幂等）与
`git::service::spawn_capture`（后台线程三段式锁纪律）负责落库，
terminal 生命周期与 `finish_session` 触发补拍，IPC `session_git_snapshots` 纯读，
`git::window` 现算会话窗口摘要（IPC `session_git_summary`，不落库），
前端在 Sessions 页渲染（`normalizeSessionGitSnapshot` / `normalizeSessionGitSummary`，src/lib/ipc.ts）。
共享测试夹具 `git/test_support.rs`（隔离环境的真临时仓库）。
真实仓库矩阵与当前仓库只读 smoke 在 `src-tauri/tests/git_snapshot.rs`；
spec `docs/specs/2026-10-23-git-snapshot-capture-design.md` 与
`docs/specs/2026-10-24-git-session-snapshot-design.md`（A3 摘要见
`docs/specs/2026-10-24-session-git-summary-design.md`）。不保证跨查询原子快照。
Worktree 编排（C10-C11）：`src-tauri/src/git/worktree.rs`（list/add/status_counts/remove/merge_ff_only，
路径安全红线见 `docs/specs/2026-10-25-worktree-add-design.md` 与
`docs/specs/2026-10-25-worktree-merge-delete-design.md`）；IPC 五命令 +
`start_terminal_in_worktree`；前端 Projects 页 Worktrees 区。
只读回放与搜索（B6-B8）：`src-tauri/src/replay/{mod,claude,index}.rs` ——
claude 转录解析（16 MiB 上限、未知类型计数、tool_use/tool_result 摘要 ≤2 KiB、
sidechain 链分段）、FTS5 trigram 索引（migration 0011，指纹增量、默认关闭、
关闭即清空、<3 字符查询报错）、路径白名单（IPC 只收 sessionId）；
真机 smoke `src-tauri/tests/claude_replay_smoke.rs`；fixture `fixtures/claude/replay/`；
前端 `/replay` 路由 `src/features/replay/ReplayPage.tsx`（搜索/索引设置/工具卡/
子代理线程/恢复按钮）。恢复外部会话（B9）：`HarnessAdapter::build_resume_spec`

- `TerminalRuntime::start_resume` + IPC `resume_terminal`；spec 三份
  `docs/specs/2026-10-25-*.md`。仅 claude 开放（capability 门控）。
  Harness 身份色：`src/lib/harness-colors.ts`（同一 id 在 Dashboard / Sessions /
  Harnesses 三处永远同色，未知 id 中性灰）。
  插件宿主：`src-tauri/src/plugins/{manifest,rpc,adapters,host_api,registry,discovery,service,supervisor}.rs`
  —— manifest 解析校验与 capability 授权（唯一的「这方法能不能调」来源）、行分隔
  JSON-RPC codec、六类 Adapter trait（v0.1 只打通 usage 与 skill，其余 `Unsupported`
  返回 `-32001`）、受限 Host API（v0.1 只有 `host/log` 真实实现，7 个调用都写
  `permission_decisions` 观测行）、SQLite 持久化（迁移 0008/0009）、discovery 只碰文件系统、
  service 编排 refresh/import_usage、supervisor 负责 spawn/握手/超时/降级/懒重启；
  协议契约与 manifest schema 在 `plugins/sdk/`，两个示例在 `plugins/examples/`；
  IPC 命令 plugin_list / plugin_refresh；真机测试 `src-tauri/tests/real_plugins.rs`；
  spec `docs/specs/2026-10-16-plugins-host-design.md`，边界见 ADR-0018。

```text
src/features/*  →  src/components/*  →  src/lib/*
src/features/*  →  src/lib/ipc.ts  →  Tauri IPC  →  src-tauri/src/commands.rs
commands.rs     →  领域模块（harness / usage / session / db / …）
领域模块         →  db / error
任何 Rust 模块   →  不得依赖 Tauri GUI 类型（除 commands.rs 与 lib.rs 的 run()）
Python sidecar  →  不得直接写 SQLite（只有 Rust Control Plane 拥有数据库）
```

## 验证命令

```bash
pnpm lint           # ESLint
pnpm typecheck      # tsc -b
pnpm test           # Vitest（前端单元测试）
pnpm build          # 前端产物构建
pnpm rust:check     # cargo fmt --check + clippy -D warnings + cargo test
pnpm python:test    # Python sidecar 单元测试
pnpm verify         # 以上全部（提交前跑这个）
```
