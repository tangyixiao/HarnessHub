# AGENTS.md

所有在本仓库工作的 Agent（人或 AI）必须遵守本文件。规格源文件是根目录的
`HARNESS_HUB_PROJECT_PLAN.md`；已稳定的事实见 `docs/CONTEXT.md`；
"去哪里找东西"见 `docs/CONTEXT-MAP.md`。

## 开工前

1. 读 `docs/CONTEXT.md`。
2. 读 `docs/CONTEXT-MAP.md`。
3. 找相关 ADR（`docs/adr/`），冲突时 ADR 优先于直觉。
4. 搜现有实现，**禁止凭感觉新建重复 abstraction**。
5. 非平凡功能必须有 spec（`docs/specs/`）与 plan（`docs/plans/`），先计划再写代码。

## 开发时

- **TDD**：先写失败的测试，确认它确实失败，再写最小实现，再重构。
- **小 Task**：一个 Task 形成独立可测试成果（例如"读取 Codex 版本"，而不是"实现 Codex 支持"）。
- **小 Commit**：一次提交只做一件事，提交信息用 `feat:` / `fix:` / `docs:` / `chore:` / `test:` / `refactor:`。
- 不跨无关模块重构。
- **不偷偷改变 schema**：改 SQLite schema 必须新增 migration 文件，禁止修改已提交的 migration。
- 不在没有证据时宣称完成。
- 外部 Harness 的日志与 Session 数据**只读**。

## 完成前

至少执行并通过：

```text
format        pnpm format:check && pnpm rust:fmt:check
lint          pnpm lint
typecheck     pnpm typecheck
unit tests    pnpm test && pnpm rust:test && pnpm python:test
build         pnpm build
clippy        pnpm rust:clippy
```

任一失败即未完成。报告结果时贴出真实输出，不要复述本文件的期望值。

## 禁止事项

- 禁止把真实 API Key / Token / Secret 写入仓库、SQLite 或日志。
- 禁止把 Prompt / Response / Session 原文发送到网络（local-first）。
- 禁止在 `src/features/*` 组件里直接调用 `invoke()`，必须走 `src/lib/ipc.ts`。
- 禁止让 Python sidecar 直接访问 SQLite：数据库归 Rust Control Plane 所有。
