-- 0001_init —— Harness Hub v0.1 核心 schema。
--
-- 设计约束（见 docs/adr/0004-sqlite-strategy.md）：
--   * 已提交的 migration 不得修改，只能新增。
--   * 外部 Harness 的数据只读，这里只保存索引与元数据。
--   * import 必须幂等：唯一约束在数据库层保证，而不是应用层判断。
--   * 时间统一用 RFC3339 UTC 字符串（SQLite 无原生时间类型）。
--   * Session 必须区分 hub_session_id 与外部 source_session_id。

-- 运行目标：v0.1 只有 local，接口为远程预留（ADR-0021）。
CREATE TABLE runtime_targets (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL CHECK (kind IN ('local')),
    display_name TEXT NOT NULL,
    base_path    TEXT,
    created_at   TEXT NOT NULL
);

CREATE TABLE harnesses (
    id                TEXT PRIMARY KEY,              -- 规范 id，例如 "codex"
    display_name      TEXT NOT NULL,
    installed         INTEGER NOT NULL DEFAULT 0 CHECK (installed IN (0, 1)),
    binary_path       TEXT,
    version           TEXT,
    -- HarnessCapabilities 的 JSON 快照：能力是可逐项灰度的矩阵，不是单个 boolean。
    capabilities_json TEXT NOT NULL DEFAULT '{}',
    data_paths_json   TEXT NOT NULL DEFAULT '[]',
    detected_at       TEXT,
    created_at        TEXT NOT NULL,
    updated_at        TEXT NOT NULL
);

CREATE TABLE projects (
    id             TEXT PRIMARY KEY,
    name           TEXT NOT NULL,
    repo_root      TEXT NOT NULL UNIQUE,
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL,
    last_opened_at TEXT
);

-- 项目级 Harness 设置；与全局默认分开，避免隐式覆盖。
CREATE TABLE project_harness_settings (
    project_id   TEXT NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    harness_id   TEXT NOT NULL REFERENCES harnesses (id) ON DELETE CASCADE,
    settings_json TEXT NOT NULL DEFAULT '{}',
    updated_at   TEXT NOT NULL,
    PRIMARY KEY (project_id, harness_id)
);

CREATE TABLE sessions (
    hub_session_id      TEXT PRIMARY KEY,
    source_session_id   TEXT,                        -- 外部 Harness 的原始 id
    harness_id          TEXT NOT NULL REFERENCES harnesses (id),
    project_id          TEXT REFERENCES projects (id) ON DELETE SET NULL,
    runtime_target_id   TEXT NOT NULL REFERENCES runtime_targets (id),
    parent_session_id   TEXT REFERENCES sessions (hub_session_id) ON DELETE SET NULL,
    status              TEXT NOT NULL CHECK (status IN ('running', 'exited', 'failed', 'unknown')),
    launch_mode         TEXT NOT NULL CHECK (launch_mode IN ('terminal', 'resume', 'imported')),
    cwd                 TEXT,
    worktree_path       TEXT,
    started_at          TEXT NOT NULL,
    ended_at            TEXT,
    exit_code           INTEGER,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);

CREATE INDEX idx_sessions_started_at ON sessions (started_at DESC);
CREATE INDEX idx_sessions_project ON sessions (project_id, started_at DESC);
CREATE INDEX idx_sessions_harness ON sessions (harness_id, started_at DESC);

-- 同一个 Harness 的同一个 source session 不能重复入库（import 幂等的基础）。
CREATE UNIQUE INDEX uq_sessions_source
    ON sessions (harness_id, source_session_id)
    WHERE source_session_id IS NOT NULL;

CREATE TABLE turns (
    id         TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions (hub_session_id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    ended_at   TEXT,
    UNIQUE (session_id, seq)
);

CREATE TABLE messages (
    id         TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions (hub_session_id) ON DELETE CASCADE,
    turn_id    TEXT REFERENCES turns (id) ON DELETE CASCADE,
    role       TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
    content    TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_messages_session ON messages (session_id, created_at);

CREATE TABLE models (
    id             TEXT PRIMARY KEY,                 -- 规范化的 "<provider>/<model>"
    provider       TEXT NOT NULL,
    display_name   TEXT NOT NULL,
    context_window INTEGER,
    created_at     TEXT NOT NULL
);

-- 导入与外部源文件：用于「可重复执行且不会重复计数」与增量扫描。
CREATE TABLE source_files (
    id              TEXT PRIMARY KEY,
    harness_id      TEXT NOT NULL REFERENCES harnesses (id) ON DELETE CASCADE,
    path            TEXT NOT NULL,
    kind            TEXT NOT NULL,
    size_bytes      INTEGER,
    modified_at     TEXT,
    content_hash    TEXT,
    last_scanned_at TEXT NOT NULL,
    UNIQUE (harness_id, path)
);

CREATE TABLE imports (
    id               TEXT PRIMARY KEY,
    source           TEXT NOT NULL,                  -- 例如 "ccusage"
    harness_id       TEXT REFERENCES harnesses (id) ON DELETE SET NULL,
    range_start      TEXT,
    range_end        TEXT,
    status           TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed')),
    records_seen     INTEGER NOT NULL DEFAULT 0,
    records_imported INTEGER NOT NULL DEFAULT 0,
    records_skipped  INTEGER NOT NULL DEFAULT 0,
    started_at       TEXT NOT NULL,
    finished_at      TEXT,
    error            TEXT
);

CREATE TABLE usage_events (
    id                    TEXT PRIMARY KEY,
    -- dedupe_key 唯一：import 重复执行时靠数据库去重，不靠应用层判断。
    dedupe_key            TEXT NOT NULL UNIQUE,
    session_id            TEXT REFERENCES sessions (hub_session_id) ON DELETE SET NULL,
    project_id            TEXT REFERENCES projects (id) ON DELETE SET NULL,
    harness_id            TEXT NOT NULL REFERENCES harnesses (id),
    model_id              TEXT REFERENCES models (id) ON DELETE SET NULL,
    -- ADR-0008 Token Truth Precedence：记录来源，便于按优先级择优。
    source                TEXT NOT NULL CHECK (source IN ('ccusage', 'native', 'estimated')),
    import_id             TEXT REFERENCES imports (id) ON DELETE SET NULL,
    source_file_id        TEXT REFERENCES source_files (id) ON DELETE SET NULL,
    occurred_at           TEXT NOT NULL,
    day                   TEXT NOT NULL,             -- YYYY-MM-DD（UTC），用于日历聚合
    input_tokens          INTEGER NOT NULL DEFAULT 0,
    output_tokens         INTEGER NOT NULL DEFAULT 0,
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens     INTEGER NOT NULL DEFAULT 0,
    total_tokens          INTEGER NOT NULL DEFAULT 0,
    cost_usd              REAL,
    cost_estimated        INTEGER NOT NULL DEFAULT 1 CHECK (cost_estimated IN (0, 1)),
    created_at            TEXT NOT NULL
);

CREATE INDEX idx_usage_day ON usage_events (day);
CREATE INDEX idx_usage_project_day ON usage_events (project_id, day);
CREATE INDEX idx_usage_harness_day ON usage_events (harness_id, day);
CREATE INDEX idx_usage_model_day ON usage_events (model_id, day);

CREATE TABLE tool_calls (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions (hub_session_id) ON DELETE CASCADE,
    turn_id     TEXT REFERENCES turns (id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    input_json  TEXT,
    output_ref  TEXT,                                -- 大输出落文件，库里只留引用
    status      TEXT NOT NULL CHECK (status IN ('pending', 'succeeded', 'failed')),
    started_at  TEXT NOT NULL,
    ended_at    TEXT
);

CREATE TABLE file_events (
    id          TEXT PRIMARY KEY,
    session_id  TEXT REFERENCES sessions (hub_session_id) ON DELETE CASCADE,
    turn_id     TEXT REFERENCES turns (id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    action      TEXT NOT NULL CHECK (action IN ('create', 'modify', 'delete', 'read')),
    occurred_at TEXT NOT NULL
);

-- Git 事件只描述「Session 时间窗口内发生了什么代码变化」，
-- 不能用来断言代码由 AI 编写（见 docs/CONTEXT.md 核心约束 7）。
CREATE TABLE git_events (
    id             TEXT PRIMARY KEY,
    session_id     TEXT REFERENCES sessions (hub_session_id) ON DELETE CASCADE,
    project_id     TEXT REFERENCES projects (id) ON DELETE CASCADE,
    kind           TEXT NOT NULL CHECK (kind IN ('head_start', 'head_end', 'commit', 'worktree')),
    commit_sha     TEXT,
    worktree_path  TEXT,
    files_changed  INTEGER,
    insertions     INTEGER,
    deletions      INTEGER,
    occurred_at    TEXT NOT NULL
);

CREATE INDEX idx_git_events_session ON git_events (session_id, occurred_at);
