# Harness Hub — CONTEXT MAP

> 目的：让 Agent **不要每次重新探索整个仓库**。先在这里定位，再读具体文件。
> 若你移动了目录或重命名了模块，必须同步更新本文件。

## 按任务找入口

| 我要做的事                         | 去哪里                                                                                               |
| ---------------------------------- | ---------------------------------------------------------------------------------------------------- |
| Harness 检测 / 版本 / 能力 / 启动  | `src-tauri/src/harness/`（`adapter.rs` = trait，`registry.rs` = 注册表，`adapters/` = 具体 Harness） |
| Usage 发现 / 导入 / 归一化         | `src-tauri/src/usage/`                                                                               |
| Session 生命周期与索引             | `src-tauri/src/session/`                                                                             |
| SQLite schema / 迁移               | `src-tauri/src/db/`（`migrations/` 下为 SQL，`migrations.rs` 为执行器）                              |
| 进程与 PTY                         | `src-tauri/src/process/`、`src-tauri/src/pty/`                                                       |
| Git / Worktree 观察                | `src-tauri/src/git/`                                                                                 |
| 文件监听                           | `src-tauri/src/watcher/`                                                                             |
| Python Sidecar 生命周期 / JSON-RPC | `src-tauri/src/sidecar/`（Rust 侧）、`python/harness_hub_runtime/`（Python 侧）                      |
| Trace / Canonical Event            | `src-tauri/src/trace/`、`schemas/events/`                                                            |
| 权限与策略                         | `src-tauri/src/permissions/`                                                                         |
| 插件宿主                           | `src-tauri/src/plugins/`、`plugins/sdk/`                                                             |
| RuntimeTarget 抽象                 | `src-tauri/src/runtime/`                                                                             |
| HHAR 导出                          | `src-tauri/src/hhar/`、`schemas/hhar/`                                                               |
| Eval / 回归                        | `src-tauri/src/eval/`、`evals/promptfoo/`                                                            |
| Tauri IPC 命令                     | `src-tauri/src/commands.rs`                                                                          |
| 前端路由与应用壳                   | `src/app/`                                                                                           |
| Dashboard                          | `src/features/dashboard/`                                                                            |
| Harness 列表页                     | `src/features/harnesses/`                                                                            |
| 项目页                             | `src/features/projects/`                                                                             |
| Session / Terminal                 | `src/features/sessions/`、`src/features/terminal/`                                                   |
| 历史 / Activity Timeline           | `src/features/history/`                                                                              |
| 设置                               | `src/features/settings/`                                                                             |
| 前端调用 Rust                      | `src/lib/ipc.ts`（唯一 IPC 入口，不要在组件里直接 `invoke`）                                         |
| Adapter 测试夹具                   | `fixtures/<harness>/`、`docs/adr/`                                                                   |
| 规格 / 设计 / 计划                 | `docs/specs/`、`docs/plans/`、根目录 `HARNESS_HUB_PROJECT_PLAN.md`                                   |

## 层与依赖方向（不可反向依赖）

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
