-- 0014_prompt_artifacts —— Prompts Registry 的 SQLite 侧（ROADMAP E19）。
--
-- canonical markdown 存这里；投影到 <repo>/CLAUDE.md / AGENTS.md / GEMINI.md
-- 的渲染走 prompts::render（标记块语义，块外用户内容逐字节保留）。
-- project_id NULL = 全局 prompt（进每个项目的渲染）；role 字段 YAGNI 不做。
CREATE TABLE prompt_artifacts (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    content    TEXT NOT NULL,
    project_id TEXT REFERENCES projects (id) ON DELETE CASCADE,
    enabled    INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
