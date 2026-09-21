# Contributing

## 开发流程

本仓库使用 **GrillMe × Superpowers** 协议，不允许从"有个想法"直接跳到"写代码"：

```text
Idea → Grill Gate → Locked Spec / ADR → 设计 → 实施计划 → TDD → Review → 验证 → Merge
```

1. 非平凡改动先写 spec（`docs/specs/`）与 plan（`docs/plans/`）。
2. 用 `git worktree` 隔离特性开发。
3. TDD：RED → 确认失败 → GREEN → 确认通过 → REFACTOR → 再次验证。
4. 提交前跑 `pnpm verify` 与 `pnpm python:test`，把真实输出贴进 PR 描述。

## 提交规范

```text
feat:     新功能
fix:      缺陷修复
docs:     文档 / ADR / spec / plan
test:     测试
refactor: 不改变行为的重构
chore:    构建、依赖、工具链
```

一次提交只做一件事。数据库 schema 变更必须新增 migration 文件，**禁止修改已提交的 migration**。

## 代码约定

- TypeScript：严格模式，组件里禁止直接 `invoke()`，统一走 `src/lib/ipc.ts`。
- Rust：`rustfmt` + `clippy -D warnings` 必须干净；领域模块不得依赖 Tauri GUI 类型。
- Python：仅标准库起步；sidecar 不得访问 SQLite。
- 单文件职责单一；文件变大通常是职责过多的信号。

## 新增 Harness Adapter 的要求

"能检测到"不算支持。必须同时提供：

- `fixtures/<harness>/` 下的真实日志夹具（脱敏）。
- 一个 contract test 覆盖 detect / capabilities / usage 解析。
- 对应的能力矩阵更新。
