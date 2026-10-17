-- 0008 —— permission_decisions：把「插件调用了哪些 Host API」记成一等数据。
--
-- 为什么要有这张表（而不是只写日志）：
--   v0.1 的权限边界是 **observe-only**（规格 §0 决策 1、§2）：不拒绝任何调用，只记录。
--   一旦只写 tracing 日志，问题就变成「哪些 API 真被用过」在重启后不可查询 ——
--   而 9.2 Permission Engine 的唯一可信输入，正是「历史上真实发生过的调用」。
--   因此观察数据必须落库（ADR-0020：权限是一等数据，不是 UI 开关）。
--
-- 为什么 plugin_id **刻意不做外键**（这是本条迁移最重要的决定）：
--   1. 这不是「本机当前状态」表，是「已经发生过的历史事实」表。插件被卸载/删除后，
--      它当年调用过什么必须照样查得到 —— 审计记录不随主体消失。加 FK 只有两种结局：
--      `ON DELETE CASCADE` 等于「卸载插件即销毁证据」，`RESTRICT` 等于「有审计记录就
--      永远删不掉插件」。两者都是错的。
--   2. 同一条思路在 0006 已经用过：`usage_events.harness` 也不做外键，因为 ccusage 会
--      报出本机尚未注册的 agent —— 外部/历史事实不该被本机状态表否决。
--   3. 插件 id 的**合法性**在 `manifest.rs` 校验（`^[a-z][a-z0-9-]*$`），不靠数据库兜。
--
-- 为什么 decision 的 CHECK 只有 'observed' 一个值：
--   v0.1 根本不会产生 allow/deny（结构层连拒绝路径都没有）。把 'allowed'/'denied' 预先
--   写进 CHECK，只会让人以为那些值已经有人写。9.2 扩展取值时必须**新增一条迁移**重建本表
--   —— SQLite 改 CHECK 只有 12-step 重建这一条路，这是有意留出的「必须显式做」的关口。
--
-- 为什么 id 用 AUTOINCREMENT 而不是裸 rowid：
--   AUTOINCREMENT 保证 id 单调且**绝不复用**。审计行一旦允许删除（例如按留存策略清理），
--   裸 rowid 会把被释放的 id 分给后来的记录，两条不同事件共用一个 id 是审计上的硬伤。
--
-- 为什么 args_json 是 NOT NULL（比 spec §2 的写法更严）：
--   每次调用都必然有参数形状，「没有参数」应写成 `{}`。允许 NULL 会让
--   「这次调用没有参数」与「这次没记下来」无法区分 —— 观察数据不允许这种歧义。
--   **长度不在数据库里约束**：512 字符是宿主侧截断的职责（Task 3 的 record_decision），
--   由 host_api 的单测锁住；在这里再写一个 CHECK 只会和「截断后追加省略号」的边界打架。
--
-- created_at 与全仓一致用 TEXT ISO8601（RFC3339 UTC），排序即字符串排序。

CREATE TABLE permission_decisions (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    -- 插件自报的 manifest id。**不是**外键，见文件头。
    plugin_id  TEXT NOT NULL,
    api        TEXT NOT NULL,                             -- 例如 "host/log"、"fs.read_scoped"
    decision   TEXT NOT NULL CHECK (decision IN ('observed')),
    reason     TEXT,                                      -- 为什么记这一条（可为空）
    args_json  TEXT NOT NULL,                             -- 已截断，宿主保证 <= 512 字符
    created_at TEXT NOT NULL
);

-- 唯一的查询形态：「某个插件最近调用了什么」（UI 与 9.2 都是这个方向）。
CREATE INDEX idx_permission_decisions_plugin_created
    ON permission_decisions (plugin_id, created_at);
