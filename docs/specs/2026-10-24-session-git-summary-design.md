# 会话窗口 Git 摘要（ROADMAP A3 前半）Spec

## Problem

A1 落了 start/end 两条快照（HEAD + 四类文件计数），但 §3.1 要求的
「Session 时间窗口内的 commit 数与 `+lines / -lines`」还没有：用户看不到
会话期间仓库前进了多少。

## Locked Goal

给定一条会话的 start/end 快照，当两端 HEAD 都存在时，用有界 git 查询得出
会话窗口摘要：commit 数、insertions、deletions；Sessions 页 Git 区块展示。
HEAD 未变（start==end）时摘要为「0 commit」，但仍展示快照计数。

## Acceptance Criteria

- [ ] 新模块 `git::window`（或 service 扩展）：`window_summary(worktree_root,
    start_head, end_head) -> Option<WindowSummary>`，内部用现有 runner 限时限量执行
      `git log --oneline <start>..<end>`（数 commit）与
      `git diff --shortstat <start> <end>`（+/-行数）。
- [ ] start_head == end_head → 直接返回零摘要，不起进程。
- [ ] 任一 head 缺失（unborn）或快照缺失 → `None`，UI 不显示该行，不报错。
- [ ] 解析 shortstat 必须容错：非预期输出 → None（不 panic、不猜）。
- [ ] IPC `session_git_summary(hub_session_id)` 纯读（ADR-0012：查询前不
      capture、不写库；摘要按需现算，v0.1 不缓存）。
- [ ] Rust DTO camelCase 契约测试；TS normalize fixture 测试。
- [ ] UI 文案继续遵守 R5：「会话期间的变更：N commits · +X / -Y」，
      不写「AI 写了」。
- [ ] 真仓库测试：start 后做 2 个 commit 再拍 end → 2 commits、行数正确；
      detached/无快照/同 HEAD 各有用例。

## Non-goals

- 不缓存摘要（每次查询现算，量级是两条 git 命令）。
- 不做 per-file diff 明细、不做 commit 列表展示（B 阶段 Replay 再说）。
- 不改 git_snapshots 表结构。

## Decision

摘要不落库：它由两个已落库的 head + 仓库当前状态推导，落库反而要处理
「仓库被 rebase 后摘要失真」的失效问题。R5 纪律不变。

## Risks

- 仓库消失/移动（快照后目录被删）→ runner 失败 → None，UI 如实不显示。

## Verification

`cargo test --lib git::` + 契约测试 + `pnpm test` + 全量质量门。
