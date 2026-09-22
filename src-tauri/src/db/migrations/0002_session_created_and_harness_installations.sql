-- 0002 —— Session 增加 created 状态；Harness 拆成「定义」与「安装」。
--
-- 两个语义修正（review 结论）：
--   1. create_session 只登记会话、不启动进程，因此新会话必须是 'created' 而不是
--      'running'。SQLite 无法修改 CHECK 约束，只能重建表。
--   2. 原 harnesses 把「Harness 类型」与「某台机器上的安装」混在一张表里。
--      远程运行时（WSL / SSH / Container）会让同一个 Codex 在多台机器上有不同安装，
--      因此拆成：
--         harnesses            = Harness 定义 / 注册表身份
--         harness_installations = 某个 runtime_target 上检测到的真实安装
--
-- 本迁移会重建表，因此执行器会在**事务外**临时关闭外键（见 apply_until），
-- 并在结束后重新打开外键 + 跑 PRAGMA foreign_key_check。

-- ---------------------------------------------------------------------------
-- 1) harnesses → 只保留「定义」
-- ---------------------------------------------------------------------------
CREATE TABLE harnesses_new (
    id           TEXT PRIMARY KEY,
    display_name TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    updated_at   TEXT NOT NULL
);

INSERT INTO harnesses_new (id, display_name, created_at, updated_at)
SELECT id, display_name, created_at, updated_at FROM harnesses;

-- ---------------------------------------------------------------------------
-- 2) 旧的安装信息要挂到 local runtime target 下，先确保它存在。
--    只在「确实有旧 harness 行要搬运」时才引导 —— 全新数据库不该被迁移塞进业务数据
--    （那是应用启动时 ensure_local_target 的职责）。
-- ---------------------------------------------------------------------------
INSERT INTO runtime_targets (id, kind, display_name, created_at)
SELECT 'local', 'local', '本机', strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE EXISTS (SELECT 1 FROM harnesses)
  AND NOT EXISTS (SELECT 1 FROM runtime_targets WHERE id = 'local');

-- ---------------------------------------------------------------------------
-- 3) 新的「安装」表
-- ---------------------------------------------------------------------------
CREATE TABLE harness_installations (
    -- 复合稳定 id：同一 Harness 在同一 runtime target 上只有一个安装。
    id                TEXT PRIMARY KEY,
    harness_id        TEXT NOT NULL REFERENCES harnesses (id) ON DELETE CASCADE,
    runtime_target_id TEXT NOT NULL REFERENCES runtime_targets (id) ON DELETE CASCADE,
    binary_path       TEXT,
    version           TEXT,
    capabilities_json TEXT NOT NULL DEFAULT '{}',
    data_paths_json   TEXT NOT NULL DEFAULT '[]',
    availability      TEXT NOT NULL CHECK (availability IN ('available', 'unavailable', 'unknown')),
    first_detected_at TEXT NOT NULL,
    last_seen_at      TEXT NOT NULL,
    UNIQUE (harness_id, runtime_target_id)
);

-- ---------------------------------------------------------------------------
-- 4) 把旧 harnesses 行里的安装信息搬过来（不丢数据）
-- ---------------------------------------------------------------------------
INSERT INTO harness_installations (
    id, harness_id, runtime_target_id, binary_path, version,
    capabilities_json, data_paths_json, availability, first_detected_at, last_seen_at
)
SELECT
    harnesses.id || '@local',
    harnesses.id,
    'local',
    harnesses.binary_path,
    harnesses.version,
    harnesses.capabilities_json,
    harnesses.data_paths_json,
    CASE WHEN harnesses.installed = 1 THEN 'available' ELSE 'unavailable' END,
    COALESCE(harnesses.detected_at, harnesses.updated_at),
    COALESCE(harnesses.detected_at, harnesses.updated_at)
FROM harnesses;

-- ---------------------------------------------------------------------------
-- 5) 换掉旧的 harnesses 表
-- ---------------------------------------------------------------------------
DROP TABLE harnesses;
ALTER TABLE harnesses_new RENAME TO harnesses;

-- ---------------------------------------------------------------------------
-- 6) 重建 sessions：status 增加 'created'，并关联到具体安装
--    旧数据里 status = 'running' 的行其实都没有进程（PTY 当时还没实现），
--    因此诚实地降级为 'created'。
-- ---------------------------------------------------------------------------
CREATE TABLE sessions_new (
    hub_session_id    TEXT PRIMARY KEY,
    source_session_id TEXT,
    harness_id        TEXT NOT NULL REFERENCES harnesses (id),
    installation_id   TEXT REFERENCES harness_installations (id) ON DELETE SET NULL,
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
    updated_at        TEXT NOT NULL
);

INSERT INTO sessions_new (
    hub_session_id, source_session_id, harness_id, installation_id, project_id,
    runtime_target_id, parent_session_id, status, launch_mode, cwd, worktree_path,
    started_at, ended_at, exit_code, created_at, updated_at
)
SELECT
    sessions.hub_session_id,
    sessions.source_session_id,
    sessions.harness_id,
    (SELECT installations.id
       FROM harness_installations AS installations
      WHERE installations.harness_id = sessions.harness_id
        AND installations.runtime_target_id = sessions.runtime_target_id),
    sessions.project_id,
    sessions.runtime_target_id,
    sessions.parent_session_id,
    CASE WHEN sessions.status = 'running' THEN 'created' ELSE sessions.status END,
    sessions.launch_mode,
    sessions.cwd,
    sessions.worktree_path,
    sessions.started_at,
    sessions.ended_at,
    sessions.exit_code,
    sessions.created_at,
    sessions.updated_at
FROM sessions;

DROP TABLE sessions;
ALTER TABLE sessions_new RENAME TO sessions;

CREATE INDEX idx_sessions_started_at ON sessions (started_at DESC);
CREATE INDEX idx_sessions_project ON sessions (project_id, started_at DESC);
CREATE INDEX idx_sessions_harness ON sessions (harness_id, started_at DESC);
CREATE INDEX idx_sessions_installation ON sessions (installation_id, started_at DESC);

-- 同一个 Harness 的同一个 source session 不能重复入库（import 幂等的基础）。
CREATE UNIQUE INDEX uq_sessions_source
    ON sessions (harness_id, source_session_id)
    WHERE source_session_id IS NOT NULL;

CREATE INDEX idx_harness_installations_harness
    ON harness_installations (harness_id, runtime_target_id);
