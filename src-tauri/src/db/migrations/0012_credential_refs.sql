-- 0012_credential_refs —— Credential Store 的 SQLite 侧（ROADMAP E16）。
--
-- ADR-0012：真实 Secret 绝不进主 SQLite；本表只存**句柄元数据**。
-- 值在 OS Secret Store 抽象（credentials/store.rs，FileSecretStore 兜底）。
CREATE TABLE credential_refs (
    id          TEXT PRIMARY KEY,
    label       TEXT NOT NULL,
    harness_id  TEXT NOT NULL REFERENCES harnesses (id) ON DELETE CASCADE,
    store_kind  TEXT NOT NULL CHECK (store_kind IN ('file', 'os')),
    created_at  TEXT NOT NULL,
    UNIQUE (label, harness_id)
);
