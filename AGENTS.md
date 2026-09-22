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

### 测试纪律：冷启动必须至少有一条真路径

涉及 **FK / migration / registry bootstrap** 的行为，测试里必须保留至少一组
**从真正空数据库开始、只调用生产代码填充前置状态**的用例。

不允许让 `seeded_db()` 之类的夹具替生产代码补前置状态 —— 真实事故已经发生过两次：

```text
seeded fixture（夹具插好 harness 行）
  → 单元测试全绿
  → production cold start 爆 FOREIGN KEY constraint failed
```

同理，验证迁移必须用**裸 Connection** 控制版本（`apply_until`），
因为 `Database::open_*` 会一次性把迁移跑到最新，那样只能验证最终 schema、
验证不了「老数据被正确搬运」。

### 跨 IPC 的契约纪律

同一类事故已经发生两次（`HarnessCapabilities` 的 snake_case、`PtyEvent` 的
`rename_all_fields`），因此固定成规则：

- **任何跨 Tauri IPC 的 Rust DTO / enum，必须有 JSON serialization contract test**
  锁定实际输出的键名与嵌套形状（注意 `rename_all` 只改变体名，结构变体字段要用
  `rename_all_fields`）。
- **任何 TS normalize / decoder，必须有对应的 fixture test**，用同一形状的 payload
  锁住解析行为与安全默认值。
- 新增 Provider / MCP / Trace Event 等 DTO 时同样适用：不靠「看起来应该是
  camelCase」猜。

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
