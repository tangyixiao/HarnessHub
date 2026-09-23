-- 0006: usage 导入契约（ADR-0011）
--
-- 为什么是「重建」而不是「加列」：
--   0001 的 `usage_events` 用的是 `dedupe_key`（朴素拼接键）、`cost_usd REAL`（浮点金额）、
--   `model_id` FK（模型必须先在 models 表里存在）。这三点与 ADR-0011 的
--   「versioned canonical key + 整数微单位 + 模型是来源给的自由文本」正面冲突，
--   而已提交的迁移不得修改（ADR-0004 第 3 条）。
--
-- 为什么重建是安全的：
--   旧 `usage_events` / `imports` 在**所有已发布版本里都没有写入方**（全仓 grep：没有任何
--   INSERT 路径），因此正常情况下它们必然是空表。为了不依赖「我以为它是空的」，
--   下面带一个守卫：一旦真的有行，整条迁移失败并回滚（版本不前进、数据不丢）。
--
-- 注意：`usage_events.harness` 刻意**不**做外键。ccusage 会报出本机尚未注册的 agent
--   （实测：claude / opencode / zcode），加 FK 会让整个导入因为一条外部数据而失败。
--   这是一张「外部事实」表，不是「本机状态」表。

CREATE TEMP TABLE migration_0006_guard (
    legacy_rows INTEGER NOT NULL CHECK (legacy_rows = 0)
);

INSERT INTO migration_0006_guard (legacy_rows)
SELECT (SELECT COUNT(*) FROM usage_events) + (SELECT COUNT(*) FROM imports);

DROP TABLE migration_0006_guard;

DROP TABLE usage_events;
DROP TABLE imports;

-- 一次导入的审计行。provenance 的关键：source_version + runner 必须能回答
-- 「这些数字是哪次调用、哪个版本产生的」。
CREATE TABLE usage_imports (
    id                    TEXT PRIMARY KEY,
    source                TEXT NOT NULL,                 -- 例如 "ccusage"
    source_version        TEXT,                          -- 例如 "ccusage 20.0.24"
    runner                TEXT,                          -- "path" | "configured" | "managed-npx"
    report_kind           TEXT,                          -- "session" | "daily"
    range_start           TEXT,                          -- RFC3339 UTC
    range_end             TEXT,
    status                TEXT NOT NULL CHECK (status IN ('running', 'succeeded', 'failed')),
    records_seen          INTEGER NOT NULL DEFAULT 0 CHECK (records_seen >= 0),
    records_inserted      INTEGER NOT NULL DEFAULT 0 CHECK (records_inserted >= 0),
    records_updated       INTEGER NOT NULL DEFAULT 0 CHECK (records_updated >= 0),
    records_skipped       INTEGER NOT NULL DEFAULT 0 CHECK (records_skipped >= 0),
    -- 无法推导发生时间、因而不能参与按天聚合的事件数（必须被显式解释，不许混进差值）。
    records_timestampless INTEGER NOT NULL DEFAULT 0 CHECK (records_timestampless >= 0),
    started_at            TEXT NOT NULL,
    completed_at          TEXT,
    error                 TEXT
);

CREATE INDEX idx_usage_imports_source_started ON usage_imports (source, started_at DESC);

-- 一行 = 一个 (会话, 模型)。session 行的 aggregate 不落库，只用于校验。
CREATE TABLE usage_events (
    id                    TEXT PRIMARY KEY,
    -- versioned canonical key（ADR-0011 决策三）：唯一约束放在数据库层，不靠应用层判断。
    stable_source_key     TEXT NOT NULL UNIQUE,
    key_version           INTEGER NOT NULL,
    source                TEXT NOT NULL,
    report_kind           TEXT NOT NULL,
    harness               TEXT NOT NULL,                 -- 外部报告的 agent，可能不在 harnesses 里
    source_session_id     TEXT NOT NULL,                 -- ccusage 的 period
    -- 只有可证明对应时才填；历史数据不得反向伪造 Harness Hub 会话。
    hub_session_id        TEXT REFERENCES sessions (hub_session_id) ON DELETE SET NULL,
    project_id            TEXT REFERENCES projects (id) ON DELETE SET NULL,
    model                 TEXT NOT NULL,
    -- ccusage 不报 provider（模型名是自由文本），列先留着：它属于 UsageEvent 的冻结概念，
    -- 第一个真的能报 provider 的来源落地时直接写，而不是那时再改 schema。
    provider              TEXT,
    input_tokens          INTEGER NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens         INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    cache_creation_tokens INTEGER NOT NULL DEFAULT 0 CHECK (cache_creation_tokens >= 0),
    cached_input_tokens   INTEGER CHECK (cached_input_tokens IS NULL OR cached_input_tokens >= 0),
    reasoning_tokens      INTEGER CHECK (reasoning_tokens IS NULL OR reasoning_tokens >= 0),
    total_tokens          INTEGER NOT NULL DEFAULT 0 CHECK (total_tokens >= 0),
    -- 金额永远是整数微单位（100 万分之一货币单位），浮点只允许存在于适配器边界。
    cost_microunits       INTEGER CHECK (cost_microunits IS NULL OR cost_microunits >= 0),
    currency              TEXT,
    currency_source       TEXT,
    -- Token Truth 是「来源」问题，不是「大小」问题：来源必须落库，见 ADR-0011 决策四。
    token_source          TEXT NOT NULL CHECK (token_source IN
                              ('provider_reported', 'provider_count_api', 'local_exact',
                               'ccusage_source_log', 'estimated')),
    cost_source           TEXT CHECK (cost_source IS NULL OR cost_source IN
                              ('ccusage_computed', 'ccusage_missing_pricing', 'native', 'estimated')),
    pricing_mode          TEXT CHECK (pricing_mode IS NULL OR pricing_mode IN
                              ('auto', 'calculate', 'display')),
    occurred_at           TEXT,
    occurred_at_source    TEXT NOT NULL CHECK (occurred_at_source IN
                              ('source_record', 'period', 'unavailable')),
    day                   TEXT,
    import_id             TEXT REFERENCES usage_imports (id) ON DELETE SET NULL,
    raw_payload           TEXT,
    imported_at           TEXT NOT NULL,

    -- 外部聚合器读来的 token 不得冒充 provider 直接上报（ADR-0011 决策四的数据库级守卫）。
    CHECK (NOT (source = 'ccusage' AND token_source = 'provider_reported')),
    -- 「有没有时间戳」和「时间戳从哪来」必须一致，不允许互相矛盾。
    CHECK ((occurred_at IS NULL) = (occurred_at_source = 'unavailable')),
    CHECK ((occurred_at IS NULL) = (day IS NULL)),
    CHECK ((currency IS NULL) = (currency_source IS NULL))
);

CREATE INDEX idx_usage_day ON usage_events (day);
CREATE INDEX idx_usage_harness_day ON usage_events (harness, day);
CREATE INDEX idx_usage_model_day ON usage_events (model, day);
CREATE INDEX idx_usage_project_day ON usage_events (project_id, day);
CREATE INDEX idx_usage_source_session ON usage_events (harness, source_session_id);
CREATE INDEX idx_usage_import ON usage_events (import_id);
