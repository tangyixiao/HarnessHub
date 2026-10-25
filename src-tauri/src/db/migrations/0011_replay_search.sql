-- 0011_replay_search —— 回放内容全文搜索（ROADMAP B7）。
--
-- 设计（docs/specs/2026-10-25-replay-search-design.md）：
--   * bundled SQLite 实测带 FTS5；tokenize='trigram' 支持中文/英文/混合子串
--     （<3 字符查询无 trigram，由查询端显式报错）。
--   * 隐私纪律（§12）：索引默认**关闭**（enabled=0），可按 project_dir 排除；
--     关闭时由应用层清空索引行。
--   * FTS5 虚拟表不能带外键/CHECK：与 replay_index_state 的一致性由
--     replay::index 在同一事务内维护（先删旧行再写新行）。

-- 单行设置表。
CREATE TABLE replay_search_settings (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    enabled       INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    -- 被排除的 project_dir 名单（JSON 字符串数组；空数组 = 不排除）。
    excluded_dirs TEXT NOT NULL DEFAULT '[]',
    updated_at    TEXT NOT NULL
);

INSERT INTO replay_search_settings (id, enabled, excluded_dirs, updated_at)
VALUES (1, 0, '[]', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'));

-- 增量索引状态：按 (harness, session) 记录指纹（size + mtime），未变化不重读。
CREATE TABLE replay_index_state (
    harness_id    TEXT NOT NULL,
    session_id    TEXT NOT NULL,
    project_dir   TEXT NOT NULL,
    source_path   TEXT NOT NULL,
    size_bytes    INTEGER NOT NULL CHECK (size_bytes >= 0),
    modified_ms   INTEGER NOT NULL,
    message_count INTEGER NOT NULL CHECK (message_count >= 0),
    indexed_at    TEXT NOT NULL,
    PRIMARY KEY (harness_id, session_id)
);

-- 全文索引：只有 text_content 参与 MATCH；其余列 UNINDEXED 用于回链与渲染。
CREATE VIRTUAL TABLE replay_messages USING fts5(
    harness_id UNINDEXED,
    session_id UNINDEXED,
    seq        UNINDEXED,
    role       UNINDEXED,
    text_content,
    tokenize = 'trigram'
);
