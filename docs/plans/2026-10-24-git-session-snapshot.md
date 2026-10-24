# Git Session Snapshot 接线 Implementation Plan

> Spec：`docs/specs/2026-10-24-git-session-snapshot-design.md`。TDD，逐 Task 提交。

## File Map

### Task 1: migration 0010 `git_snapshots`
**Files** Create: `src-tauri/src/db/migrations/0010_git_snapshots.sql`；Modify: `src-tauri/src/db/migrations.rs`
- 失败测试：`migration_0010_upgrades_a_v9_database`（裸 Connection，apply_until(9) → 塞 v9 会话行 → apply_pending → 断言表存在、CHECK/主键约束生效、外键级联删除）。
- 最小实现：建表 + 注册迁移。

### Task 2: `git::store` + `git::service`
**Files** Create: `src-tauri/src/git/{store,service,test_support}.rs`；Modify: `src-tauri/src/git/mod.rs`、`src-tauri/src/git/snapshot.rs`（测试改用共享 fixture helper）
- `SnapshotPhase`（serde snake_case）、`GitSnapshotRecord`（serde camelCase）。
- `GitSnapshotStore::insert`（INSERT OR IGNORE 语义，返回是否新插入）、`list_by_session`（按 captured_at 升序）。
- `git::service::capture_for_session(conn, hub_session_id, phase)`：读 session.cwd → 无 cwd/非 repo/错误 → Ok(false)；Repository → 落行。
- 测试：真临时仓库（复用隔离的 fixture_git 环境）、重复插入幂等、非 repo 不落行、孤儿 session id 安全失败（FK）。

### Task 3: terminal.rs 生命周期接线
**Files** Modify: `src-tauri/src/terminal.rs`
- `capture_snapshot(&self, session_id, phase)`：短锁读 cwd → 丢锁跑 capture → 短锁写。
- start：mark_running 成功后调用（best-effort，忽略错误）。
- on_exit：`finish` 返回 true 后调用。
- 测试（FakePtyBackend + 真临时仓库）：start 后有 start 行；退出后有 end 行。

### Task 4: IPC + 前端
**Files** Modify: `src-tauri/src/commands.rs`、`src-tauri/src/lib.rs`（注册）、`src/lib/ipc.ts`、`src/features/sessions/SessionsPage.tsx`（+ 两个 test 文件）
- `session_git_snapshots(hub_session_id) -> Vec<GitSnapshotRecord>` 纯读。
- `finish_session` 更新成功后同款 best-effort capture。
- 契约测试（camelCase）+ TS normalize fixture 测试 + 页面渲染测试。

### Task 5: 冷启动真路径 + 全量质量门
- 空库 → ensure_local_target → reconcile → start_from_installation(cwd=repo) → mark_running → capture start → 行存在。
- `pnpm verify` 全绿后逐 Task 已提交的 commit 合并成 PR。
