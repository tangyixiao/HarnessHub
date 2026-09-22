-- 0003 —— 结构性保证：Session 的 runtime_target_id 必须与它的 installation 一致。
--
-- 问题：migration 0002 之后，sessions 有两个**独立**的外键
--
--     installation_id   → harness_installations (id)
--     runtime_target_id → runtime_targets (id)
--
-- 但「两个外键各自合法」不等于「组合合法」：
--
--     installation_id   = 'codex@local'   （存在，合法）
--     runtime_target_id = 'wsl'           （存在，合法）
--     → 组合矛盾：codex@local 属于 local，不属于 wsl
--
-- 修法：给 harness_installations 建 (id, runtime_target_id) 唯一索引，
-- 然后让 sessions 用**复合外键**指向它。这样矛盾组合在数据库层就写不进来。
--
-- installation_id 可空（导入的历史会话可以没有安装）。SQLite 默认的
-- MATCH SIMPLE 语义下，复合外键只要有一个子列是 NULL 就视为满足约束，
-- 因此 NULL installation_id 依然合法。
--
-- 刻意不加 ON DELETE SET NULL：删除安装行时把 runtime_target_id 一并置空
-- 会违反它的 NOT NULL。安装行只会被 reconcile upsert，不会被删除。

CREATE UNIQUE INDEX uq_harness_installations_id_runtime
    ON harness_installations (id, runtime_target_id);

CREATE TABLE sessions_new (
    hub_session_id    TEXT PRIMARY KEY,
    source_session_id TEXT,
    harness_id        TEXT NOT NULL REFERENCES harnesses (id),
    installation_id   TEXT,
    project_id        TEXT REFERENCES projects (id) ON DELETE SET NULL,
    runtime_target_id TEXT NOT NULL REFERENCES runtime_targets (id),
    parent_session_id TEXT REFERENCES sessions_new (hub_session_id) ON DELETE SET NULL,
    status            TEXT NOT NULL CHECK (status IN ('created', 'running', 'exited', 'failed', 'unknown')),
    launch_mode       TEXT NOT NULL CHECK (launch_mode IN ('terminal', 'resume', 'imported')),
    cwd               TEXT,
    worktree_path     TEXT,
    started_at        TEXT NOT NULL,
    ended_at          TEXT,
    exit_code         INTEGER,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL,
    FOREIGN KEY (installation_id, runtime_target_id)
        REFERENCES harness_installations (id, runtime_target_id)
);

INSERT INTO sessions_new (
    hub_session_id, source_session_id, harness_id, installation_id, project_id,
    runtime_target_id, parent_session_id, status, launch_mode, cwd, worktree_path,
    started_at, ended_at, exit_code, created_at, updated_at
)
SELECT
    hub_session_id, source_session_id, harness_id, installation_id, project_id,
    runtime_target_id, parent_session_id, status, launch_mode, cwd, worktree_path,
    started_at, ended_at, exit_code, created_at, updated_at
FROM sessions;

DROP TABLE sessions;
ALTER TABLE sessions_new RENAME TO sessions;

CREATE INDEX idx_sessions_started_at ON sessions (started_at DESC);
CREATE INDEX idx_sessions_project ON sessions (project_id, started_at DESC);
CREATE INDEX idx_sessions_harness ON sessions (harness_id, started_at DESC);
CREATE INDEX idx_sessions_installation ON sessions (installation_id, started_at DESC);

CREATE UNIQUE INDEX uq_sessions_source
    ON sessions (harness_id, source_session_id)
    WHERE source_session_id IS NOT NULL;
