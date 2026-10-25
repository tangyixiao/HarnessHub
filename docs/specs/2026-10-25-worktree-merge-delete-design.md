# Worktree Merge/Delete + 并行编排（ROADMAP C11）Spec

## Problem

C10 只能创建 worktree；用完的 worktree 需要安全合并回主分支并清理；
多 worktree 并行会话缺少一个总览视图。这是 v0.3.0 的收尾项。

## Locked Goal

1. **删除（保守版）**：只允许删除「干净」的 worktree（无未提交修改），删除前
   用有界 `git status` 检查；有修改 → 明确报错并列出计数，绝不静默丢弃。
2. **合并（保守版）**：`git merge --ff-only <worktree_branch>` 到主分支的
   操作在**主树上**执行；非 fast-forward → 明确报错（建议用户手动处理），
   绝不做 rebase/force。合并前检查主树干净。
3. **并行编排视图**：Projects 页 worktree 区升级——每行显示「有无未提交修改」
   （有界 status 计数）与关联会话数；同一视图完成「创建 → 观察 → 合并 →
   清理」闭环。

## Acceptance Criteria

- [ ] `git::worktree` 扩展：- `status_counts(repo_root, worktree_path) -> Result<StatusCounts>`（复用
      `git -C <worktree> status --porcelain=v1 -z` 有界解析）；- `remove(repo_root, worktree_path) -> Result<()>`（干净才执行
      `git worktree remove`；主树/bare/不存在 → 明确错误）；- `merge_ff_only(repo_root, branch) -> Result<MergeOutcome>`（在主树上
      `git merge --ff-only`；主树脏 → 错误；非 ff → 错误含「非快进」）。
- [ ] IPC：`worktree_status(repoRoot, path)`（纯读）、`worktree_remove` /
      `worktree_merge`（显式写；前端 confirm 文案分别写明「删除目录」与
      「快进合并到主分支」）。repo_root 校验同 C10。
- [ ] 前端：worktree 行加「干净/有修改 N 项」徽标（惰性加载：展开时批量查）、
      「合并到主分支」「删除」按钮（均 confirm；失败原样展示）。
- [ ] 真临时仓库测试：干净删除、脏拒绝（计数正确）、ff 合并成功、非 ff 拒绝、
      主树脏拒绝、主树/bare 删除拒绝。
- [ ] 冷启动真路径：空库 → 登记 → 创建 worktree → 合并 → 删除全链路（IPC 层测试）。
- [ ] DTO 契约 + TS normalize + UI 测试（confirm 文案纪律）。

## Non-goals

- 不做非 ff 合并策略（squash/rebase）——保守第一版；
- 不做跨仓库编排、不做会话自动迁移到 worktree（Terminal 启动器选择留后续）；
- 不动 harness capability `worktree`（仍 false：C11 完成的是 Hub 自身能力，
  harness 在 worktree 中跑的验证需要真机 E2E，另行取证）。

## Decision

删除与合并都走「先检查后执行」两段式：检查用有界只读命令，执行用有界写命令；
两段之间不持锁（短事务纪律），竞态（检查后又被修改）由 git 自身拒绝兜底
（remove 对脏树、merge --ff-only 对落后分支天然安全）。

## Risks

- 用户在检查后立即改文件 → git 层拒绝，错误如实透传；
- 删除目录是破坏性操作 → confirm 文案 + 干净检查 + 已登记项目校验三重防线。

## Verification

真临时仓库全矩阵 + 契约 + `pnpm test` + 全量门。
