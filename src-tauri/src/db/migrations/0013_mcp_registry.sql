-- 0013_mcp_registry —— MCP Binding Registry 的 SQLite 侧（ROADMAP E18）。
--
-- Registry 是 Harness Hub 的 source-of-truth（规划 §4.20）；投影到各 harness
-- 原生配置走 mcp::sync 的 diff → preview → atomic sync 管线。
--
-- env_keys 只存**变量名**（JSON 数组），值绝不入内（ADR 红线：Secret 不进主 SQLite，
-- 值属于用户手工配置或 E16 凭据层）。
CREATE TABLE mcp_servers (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    transport  TEXT NOT NULL CHECK (transport IN ('stdio', 'http')),
    command    TEXT,
    args_json  TEXT,
    url        TEXT,
    env_keys   TEXT NOT NULL DEFAULT '[]',
    enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    source     TEXT NOT NULL DEFAULT 'hub',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- 绑定：scope ∈ {global, harness}。scope_key 是可 UNIQUE 的派生键——
-- SQLite 的 UNIQUE 对 NULL 视为互异，直接 UNIQUE(server_id, scope, harness_id)
-- 拦不住重复的 global 绑定，所以用 'global' / 'harness:<id>' 落到具体值。
CREATE TABLE mcp_bindings (
    id         TEXT PRIMARY KEY,
    server_id  TEXT NOT NULL REFERENCES mcp_servers (id) ON DELETE CASCADE,
    scope      TEXT NOT NULL CHECK (scope IN ('global', 'harness')),
    harness_id TEXT REFERENCES harnesses (id) ON DELETE CASCADE,
    scope_key  TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (server_id, scope_key),
    CHECK (
        (scope = 'global' AND harness_id IS NULL AND scope_key = 'global')
        OR (
            scope = 'harness'
            AND harness_id IS NOT NULL
            AND scope_key = 'harness:' || harness_id
        )
    )
);
