# 会话窗口 Git 摘要 Implementation Plan

> Spec：`docs/specs/2026-10-24-session-git-summary-design.md`。TDD。

## Task 1: `git::window` 纯计算模块

**Files** Create: `src-tauri/src/git/window.rs`；Modify: `src-tauri/src/git/mod.rs`

- `WindowSummary { commits: u64, insertions: u64, deletions: u64 }`
- `window_summary(root: &Path, start: &str, end: &str) -> Option<WindowSummary>`
  （同 HEAD → Some(0,0,0)；解析失败 → None）
- 测试：真临时仓库（test_support::Repo）2 commits 窗口 / 同 HEAD / 坏输出容错。

## Task 2: 会话级编排 + IPC

**Files** Create: `src-tauri/src/git/window.rs` 内 `session_summary(conn, hub_session_id)`；Modify: `src-tauri/src/commands.rs`、`src-tauri/src/lib.rs`

- 读两条快照 → 都有 head → window_summary；否则 None。
- `session_git_summary` IPC（纯读）+ 契约测试（camelCase、null 语义）。

## Task 3: 前端

**Files** Modify: `src/lib/ipc.ts`、`src/features/sessions/SessionsPage.tsx`（+ tests）

- `getSessionGitSummary` + normalize；Git 区块加一行「N commits · +X / −Y」
  （有摘要才渲染）；R5 文案测试。

## Task 4: 全量质量门 + PR
