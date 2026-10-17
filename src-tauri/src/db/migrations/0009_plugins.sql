-- 0009 —— plugins：已登记插件的本机状态。
--
-- 这张表回答的问题是「本机现在装了哪些插件、每个现在是什么状态」，
-- 它与 0008 的 permission_decisions 正好相反：那边是不可变的历史事实，
-- 这边是**可被重写**的当前状态（每次 refresh 都会覆盖）。两者不共享生命周期。
--
-- 为什么不把 manifest 全量存下来（只存能力快照）：
--   manifest.toml 是磁盘上的唯一事实源，扫描时随时可重读。落库只需要「UI 不必再去
--   解析 TOML 就能渲染列表」所需的最少字段 + capabilities 的规范化快照。
--   把 manifest 原文存一份会让「磁盘改了但库里没改」成为可能，多一份可漂移的副本。
--
-- 为什么 path 是 TEXT 而不是外键/规范化：
--   插件目录可以整体挪动（用户自选 HARNESS_HUB_PLUGINS_DIR）。path 只是「上次扫描到它的
--   位置」，用来诊断与重新发现，不是身份 —— 身份永远是 id。
--
-- 为什么 skills_cache_json 允许 NULL：
--   NULL = 「还没拉过」；'[]' = 「拉过，插件声明零个 skill」。两者语义不同，
--   不能用一个默认值糊过去（v0.1 只在注册时刷新，不做热更新）。
--   capabilities_json 则是 NOT NULL：没有能力声明的插件不会走到写表这一步。
--
-- 为什么 status 用 CHECK 锁死 'ready' / 'degraded'：
--   这两个值直接决定宿主还敢不敢 spawn 它，属于安全状态而不是展示字段。
--   拼错的 status 必须在写入点就炸，而不是等到 supervisor 里变成「未知状态按什么处理」。
--   registered_at 由 upsert 的 ON CONFLICT 分支刻意**不**更新：首次登记时间才是审计关心的，
--   后续 refresh 只改可变字段（见 registry.rs 的 upsert_plugin）。

CREATE TABLE plugins (
    id                  TEXT PRIMARY KEY,
    name                TEXT NOT NULL,
    version             TEXT NOT NULL,
    api_version         TEXT NOT NULL,
    path                TEXT NOT NULL,
    capabilities_json   TEXT NOT NULL,
    skills_cache_json   TEXT,
    status              TEXT NOT NULL CHECK (status IN ('ready', 'degraded')),
    registered_at       TEXT NOT NULL
);
