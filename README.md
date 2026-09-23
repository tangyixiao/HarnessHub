# Harness Hub

> **Universal AI Development Control Plane** —— 统一管理、启动、观察、检索和复盘 AI Coding Harness 的本地优先桌面控制中心。
> 目标不是再造一个 Harness，而是把 Codex、Claude Code、Gemini CLI、OpenCode 等工具变成一个统一的工作环境。

- 规格源文件：[HARNESS_HUB_PROJECT_PLAN.md](./HARNESS_HUB_PROJECT_PLAN.md)
- 已稳定事实：[docs/CONTEXT.md](./docs/CONTEXT.md)
- 代码地图：[docs/CONTEXT-MAP.md](./docs/CONTEXT-MAP.md)
- 架构决策：[docs/adr/](./docs/adr/)
- 实施计划：[docs/plans/](./docs/plans/)

## 当前状态

Phase 1 Walking Skeleton —— **项目框架已就位**：Tauri 2 应用壳、React 前端、Rust Control Plane
模块骨架、SQLite 迁移执行器、Python sidecar 骨架全部可编译可测试。
业务链路（Harness 检测 / PTY 启动 / ccusage 导入 / Dashboard 数据）按 `docs/plans/` 逐步实现。

## 快速开始（≈10 分钟）

前置条件：

| 依赖             | 版本                             | 说明                                             |
| ---------------- | -------------------------------- | ------------------------------------------------ |
| Node.js          | ≥ 20                             | 前端与工具链                                     |
| pnpm             | 11.x                             | 包管理器（`corepack enable` 或 `npm i -g pnpm`） |
| Rust             | stable（含 `rustfmt`、`clippy`） | Control Plane                                    |
| MSVC Build Tools | VS 2022 或更新                   | Windows 上编译 Rust / bundled SQLite             |
| WebView2 Runtime | 任意近期版本                     | Windows 上运行 Tauri 窗口（Win11 通常已内置）    |
| uv               | 0.5+                             | 仅 Python sidecar 需要                           |

```bash
git clone <repo> harness-hub && cd harness-hub
pnpm install
pnpm verify          # lint + typecheck + unit tests + build + rust fmt/clippy/test
pnpm python:test     # Python sidecar 单元测试
pnpm tauri dev       # 启动桌面应用（会自动拉起 Vite dev server）
```

## 常用命令

| 命令               | 作用                                                      |
| ------------------ | --------------------------------------------------------- |
| `pnpm dev`         | 只启动前端（浏览器里可跑 UI，IPC 调用会降级为不可用状态） |
| `pnpm tauri dev`   | 启动完整桌面应用                                          |
| `pnpm test`        | 前端单元测试（Vitest）                                    |
| `pnpm rust:test`   | Rust 单元/集成测试                                        |
| `pnpm rust:clippy` | Rust 静态检查（`-D warnings`）                            |
| `pnpm python:test` | Python sidecar 测试                                       |
| `pnpm verify`      | 提交前完整验证                                            |

## 仓库结构（节选）

```text
src/                  React 前端（app 壳 / components / features / lib）
src-tauri/src/        Rust Control Plane（db / harness / usage / session / pty / …）
python/               Python AI Runtime sidecar（stdio JSON-RPC）
schemas/              HHAR / events / plugin-manifest 的 schema
fixtures/             外部 Harness 日志夹具（只读样本）
plugins/              插件 SDK 与示例
docs/                 CONTEXT / CONTEXT-MAP / adr / specs / plans
```

## 原则（摘要）

1. **Local-first**：Prompt / Response / Session 原文不上传；估算值必须标记 `estimated`。
2. **Evidence over claims**：没有测试与构建证据，不算完成。
3. **不重复造轮子**：先整合成熟项目，再复用，再借鉴重写接口层。
4. **HarnessAdapter 与 UsageAdapter 分离**：能启动 ≠ 能可靠解析 Usage。
5. **外部 Harness 数据只读**，配置投影必须原子写。
6. **Secret 不进主 SQLite**。

## License

MIT
