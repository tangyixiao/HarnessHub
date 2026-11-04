# Worktree 创建与状态 Implementation Plan

> Spec：`docs/specs/2026-10-25-worktree-add-design.md`。TDD。

## Task 1: `git::worktree` 模块

**Files** Create: `src-tauri/src/git/worktree.rs`；Modify: `src-tauri/src/git/mod.rs`、`src-tauri/src/git/runner.rs`（Limits 可传时长）

- `WorktreeEntry { path, head, branch, is_main, detached, bare }`（serde camelCase）
- `list` / `add`（-b 分支 / 已有分支复用；错误分类：已占用/冲突/不存在）
- 测试：隔离临时仓库三形态（主树/detached/linked）、畸形 porcelain、重复冲突。

## Task 2: Session 绑定 + IPC

**Files** Modify: `src-tauri/src/session/{service,store}.rs`、`src-tauri/src/commands.rs`、`src-tauri/src/lib.rs`

- `start_from_installation` 增加 `worktree_path: Option<&str>`；terminal.rs start 透传。
- `worktree_list` / `worktree_add` 命令（repo_root 校验已登记项目）；契约测试。

## Task 3: 前端（flash 子 agent）

**Files** Modify: `src/lib/ipc.ts`（+tests）、Terminal 启动器或 Projects 页

- worktree 列表展示 + 启动时选择（选中 → cwd + worktreePath）；创建对话框
  （路径 + 分支名 + 二次确认文案「将在磁盘创建目录」）。

## Task 4: 全量门 + 本机只读 smoke + PR
