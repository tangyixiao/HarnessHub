# 只读会话回放 B6 Implementation Plan

> Spec：`docs/specs/2026-10-25-claude-replay-design.md`。TDD。

## Task 1: fixture + `replay::claude` 解析

**Files** Create: `fixtures/claude/replay/basic.jsonl`、`fixtures/claude/replay/README.md`、`src-tauri/src/replay/{mod,claude}.rs`

- `ReplayMessage { seq, role, sidechain, timestamp, text_parts, thinking_parts, model, unsupported_content_kinds }`
- `ReplaySession { session_id, source_path, messages, skipped_kind_counts, truncated: bool }`
- `read_session(path)`：16 MiB 上限；逐行 serde_json；坏行计入 `malformed_lines` 不中断。
- 测试：fixture 全形状（user 字符串/数组、assistant text+thinking、tool_use 计数、sidechain、未知类型 skipped）、超限、坏行。

## Task 2: 发现列表（有界预读）

- `list_sessions(roots)`：扫描 `*/projects/*/*.jsonl`，stat + 预读 256 KiB 取首条 user 预览（≤120 字符）与首末时间戳；按 mtime 倒序。
- 测试：临时目录假树（多个 project 目录、空文件、超大文件跳预览）。

## Task 3: capability + IPC + 注册

- claude_code adapter `replay: true`。
- `replay_list_sessions(harness_id) -> Vec<ReplaySessionSummary>`（非 claude/未安装 → 空）。
- `replay_read_session(harness_id, session_id) -> Option<ReplaySession>`：session_id
  必须命中 Task 2 的发现结果（路径白名单），否则 None——**IPC 不接受路径**。
- 契约测试（camelCase、枚举值）。

## Task 4: 前端（flash 子 agent）

- `src/lib/ipc.ts`：normalize + `listReplaySessions` / `readReplaySession`。
- 新 `src/features/replay/ReplayPage.tsx`（路由 /replay）：左列会话列表（harness
  capability 过滤）、右侧只读时间线（user/assistant 气泡、thinking 标注、sidechain
  徽标、skipped 计数脚注）。测试：列表渲染、时间线渲染、无能力不渲染入口。

## Task 5: 全量门 + 本机只读 smoke + PR
