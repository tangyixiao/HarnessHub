# Activity Timeline 设计（§20 第 15 步）

- 日期：2026-10-14
- 规格源：`HARNESS_HUB_PROJECT_PLAN.md` §20（v0.1 顺序：13 OpenCode → 14 Project registry → **15 Activity timeline** → 16 Git observer → 17 Packaging）
- 本步目标：把 `/activity` 从占位页变成**可用的时间线视图**，数据全部来自本机 SQLite。

## 目的与范围

### 本步做

1. 新增 Rust 只读模块 `src-tauri/src/activity/`，把 `usage_events` + `sessions` 两表折叠成一个
   **统一的活动事件流（Projection）**：
   - 每个事件带 `kind`（`usage` / `session`）、本地日 `day`、发生时间 `occurred_at`、`harness_id`、
     `project_id`、`total_tokens`、`cost_microunits` / `cost_is_lower_bound`、`duration_ms`（仅 session）。
   - 同时提供按**本地日**聚合的 `days`（Activity Calendar 的格子）与按**小时**聚合的 `hours`。
   - 提供三个筛选计数：`usage_events`、`managed_sessions`、`active_days`，以及 `max_day_tokens`
     / `max_hour_tokens`（前端画格子/条只需要一个最大值，不需要自己再扫一遍）。
   - 时间范围与 Dashboard 完全一致：`today` / `7d` / `30d` / `all`，边界由
     `timezone_offset_minutes` 决定，复用 `usage::summary::range_bounds` 与 `range_filter`
     的 SQL 习惯（`datetime(occurred_at) >= datetime(?N) AND < datetime(?N+1)`）。
2. 新增 1 个 IPC 命令 `activity_timeline(state, range, timezone_offset_minutes)`
   → `ActivityTimeline`（跨 IPC DTO，`#[serde(rename_all = "camelCase")]`）。
3. 前端 `src/lib/ipc.ts` 增加 `ActivityTimeline` / `ActivityEvent` / `ActivityDayBucket` /
   `ActivityHourBucket` 类型 + `normalizeActivityTimeline` + `getActivityTimeline`；
   `src/features/history/ActivityPage.tsx` 从 PlaceholderCard 变成：
   - 顶部范围切换（今天 / 7 天 / 30 天 / 全部）；
   - KPI 卡：事件数、会话数、活跃天数、tokens；
   - **Activity Calendar**：按本地日排布的格子（周一起始），颜色深浅按当日 tokens，
     悬停/聚焦显示日期与数值；点击某天 → 下方列表过滤到那一天；
   - **Session Timeline**：选中日（未选中则为整个窗口）的事件列表，倒序，
     usage 行显示 tokens/cost，session 行显示时长与状态。
4. 前端页面测试 + Rust 单测 + IPC 契约测试 + 真机/冷启动测试（见「验收」）。

### 本步不做

1. **不写 `git_events` / `file_events`**：这两张表自 0001 起存在，但至今没有任何 Rust 写入方，
   `src-tauri/src/git/mod.rs` 与 `src-tauri/src/watcher/mod.rs` 仍是 doc-comment 占位模块
   （前者明确写着「刻意不提供未经验证的公开 API」）。Git 快照属于第 16 步 Git observer。
   本步 UI 因此**不显示任何 Git 数据**，也不留假的 Git 区块。
2. **不新增 migration**：本步只读 `sessions` / `usage_events` 两张已有表，schema 无需变更。
3. **不做** Project History / Git Diff summary / Commit tracking / Harness-Model-Project 筛选器
   （Phase 3 其余条目），也不做 Tool usage、月度报告（Phase 11）。
4. **不推断项目归属**：`usage_events.project_id` 在 v0.1 恒为 NULL（`usage/importer.rs`
   「v0.1 不从 usage 推导项目归属」），因此 usage 事件的 project 只来自 `sessions.project_id`
   （通过 `hub_session_id` 关联）；关联不上就留空，**不猜**。
5. **不把 diff 归因为 AI**（CONTEXT.md 核心约束 7）：本步不展示任何 diff/代码变更数据，
   也就不存在错误归因；UI 文案明确写「时间线描述的是事件发生的时间，不代表代码作者」。

## 本机与官方事实

- `src-tauri/src/db/migrations/0001_init.sql:51-76`：`sessions(hub_session_id PK, source_session_id,
harness_id FK, project_id FK ON DELETE SET NULL, runtime_target_id FK, status CHECK IN
('running','exited','failed','unknown'), launch_mode CHECK IN ('terminal','resume','imported'),
cwd, worktree_path, started_at, ended_at, exit_code, created_at, updated_at)`；
  `idx_sessions_started_at`、`idx_sessions_project`、`idx_sessions_harness` 三个索引已存在。
- `src-tauri/src/db/migrations/0001_init.sql:134-161`（被 0006 重建、0007 扩列）：
  `usage_events` 现有 `harness`、`source_session_id`、`hub_session_id`、`project_id`、`model`、
  `total_tokens`、`cost_microunits`、`cost_source`、`occurred_at`、`occurred_at_source`、`day` 等列。
- `src-tauri/src/usage/summary.rs:155-193`：`range_bounds(connection, range, now_utc,
timezone_offset_minutes)` 把日期算术全部交给 SQLite `datetime(?1, ?2, 'start of day', ?3, ?4)`；
  `UsageRangeWindow{kind,start_utc,end_utc,timezone_offset_minutes,now_utc}` + `is_bounded()`。
- `src-tauri/src/usage/summary.rs:294-312`：`range_filter(window, first_index)` 产出
  `WHERE datetime(occurred_at) >= datetime(?N) AND datetime(occurred_at) < datetime(?N+1)`；
  文件里明确记录了「timeline 把时区修饰符放在 `?1`，范围参数必须从 `?2` 开始」的坑。
- `src-tauri/src/usage/summary.rs:395-435`：`timeline()` 用
  `strftime('%Y-%m-%d', occurred_at, ?1)` 做**本地日**分组，修饰符在 `?1`。
- `src-tauri/src/usage/importer.rs`：幂等 UPSERT（`ON CONFLICT(stable_source_key)`）；
  `project_id` 恒为 `None`；`hub_session_id` 只在 `(harness_id, source_session_id)` 唯一匹配时回填。
- `src-tauri/src/git/mod.rs`、`src-tauri/src/watcher/mod.rs`：均为 doc-comment 占位模块，无写入方。
- `src/app/routes.tsx:53`：`{ path: 'activity', element: <ActivityPage /> }` 已存在，本步不改路由。
- `docs/adr/0012-dashboard-reads-only-sqlite.md`：本步所有 IPC 都遵守「纯读 SQLite、不起进程、
  不写库」；参数不合法必须报错，不得退化成「全部时间」。

## 设计

### Rust 新模块 `src-tauri/src/activity/`

```
activity/
├── mod.rs        // ActivityEvent / ActivityTimeline / ActivityDayBucket / ActivityHourBucket
│                // + keys()（跨 IPC 契约测试用）+ #[cfg(test)] mod tests;
├── query.rs      // timeline(connection, window, limit) -> Result<ActivityTimeline>（纯读）
└── tests.rs      // 单测（见「验收」）
```

- `mod.rs` 只放 DTO 与 `keys()`，与 `project/mod.rs` 同形（`ProjectRecord::keys()` 是既有先例）。
- `query.rs` 是**唯一**的查询入口，签名只拿 `&Connection`（不起进程、不写库，ADR-0012）。
- 不新增 store：两张表的读取形状不同（events 聚合 + sessions 明细），一个 `query.rs`
  比「一个 store + 一个 service」更贴近真实复杂度；`session::store` / `project::store`
  的既有公开 API 不重复实现。

事件流构造（`query.rs` 的核心，一次两条 SQL）：

1. **usage 事件**（来自 `usage_events`，按 `occurred_at` 落在窗口内）：
   ```sql
   SELECT 'usage' AS kind, occurred_at, harness, hub_session_id, project_id,
          total_tokens, cost_microunits, cost_source
     FROM usage_events
    WHERE datetime(occurred_at) >= datetime(?1) AND datetime(occurred_at) < datetime(?2)
   ```
   - `kind = 'usage'`；`duration_ms = None`；`status = None`。
   - `cost_is_lower_bound` 由后端按 `cost_source` 判定（`estimated` → `true`），与 ADR-0012
     决策四「`≥` 由后端决定，前端只格式化」一致。
   - `project_id` 取 `sessions.project_id`：先按 `hub_session_id` join `sessions`，join 不上则为 NULL。
     （不按 `source_session_id` 猜 —— 那正是 importer 明确拒绝的事。）
2. **session 事件**（来自 `sessions`，按 `started_at` 落在窗口内）：

   ```sql
   SELECT 'session' AS kind, started_at AS occurred_at, hub_session_id, harness_id,
          project_id, status, launch_mode, cwd, ended_at
     FROM sessions
    WHERE datetime(started_at) >= datetime(?1) AND datetime(started_at) < datetime(?2)
   ```
   - `total_tokens = 0`、`cost_microunits = None`；`duration_ms` =
     `ended_at - started_at`（用 SQLite `strftime('%s')` 算秒再乘 1000；`ended_at` 为 NULL → None）。
   - 这是 **managed sessions**（Hub 自己启动的），与 usage 明细里的 `usage_sessions` 是两件事，
     ADR-0012 决策三要求永不合并 —— 所以两条 SQL 分开，前端分行展示、分别计数。

3. 聚合 `days`（本地日，`strftime('%Y-%m-%d', occurred_at, ?modifier)`）与 `hours`
   （`strftime('%H', occurred_at, ?modifier)`，`hour` 为 0..23 的整数）。
   `hours` 覆盖整个窗口（不是只有有事件的小时），这样前端画 24 格热力条不需要补零。

DTO（全部 `#[serde(rename_all = "camelCase")]`）：

```rust
pub struct ActivityTimeline {
    pub range: UsageRangeWindow,      // 复用，不重复定义
    pub events: Vec<ActivityEvent>,   // 倒序，长度受 limit 约束
    pub days: Vec<ActivityDayBucket>,  // 按本地日升序
    pub hours: Vec<ActivityHourBucket>,// 固定 24 项，按 hour 升序
    pub usage_event_count: u64,
    pub managed_session_count: u64,
    pub active_days: u64,
    pub total_tokens: u64,
    pub max_day_tokens: u64,
    pub max_hour_tokens: u64,
    pub has_more: bool,               // events 被 limit 截断时为 true
}
```

`ActivityEvent`：`kind`（`'usage'`|`'session'`）、`occurred_at`、`day`、`hour`、`harness_id`、
`project_id`、`source_session_id`、`model`（仅 usage）、`total_tokens`、`cost_microunits`、
`cost_is_lower_bound`、`status`（仅 session）、`launch_mode`（仅 session）、`duration_ms`
（仅 session）、`cwd`（仅 session）。

`limit`：命令层按 `DEFAULT_ACTIVITY_LIMIT = 200` / `MAX_ACTIVITY_LIMIT = 1000` 夹取
（沿用 `DEFAULT_SESSION_LIMIT` / `MAX_SESSION_LIMIT` 的写法）；`has_more` 让前端知道
「还有更多，别假装这是全部」。

### IPC 命令

```rust
#[tauri::command]
pub fn activity_timeline(
    state: State<'_, AppState>,
    range: String,
    timezone_offset_minutes: i32,
    limit: Option<u32>,
) -> Result<crate::activity::ActivityTimeline>
```

- 与 `usage_summary`（`src-tauri/src/commands.rs:330-346`）同形：锁 db → `parse_range` →
  `range_bounds(conn, range, &clock::now_rfc3339(), offset)` → `activity::query::timeline(...)`。
- `src-tauri/src/lib.rs` 的 `invoke_handler` 由 18 个命令变为 19 个（保持字母序）。

### 前端

- `src/lib/ipc.ts`：新增 `ActivityEventKind = 'usage' | 'session'`、
  `ActivityEvent` / `ActivityDayBucket` / `ActivityHourBucket` / `ActivityTimeline` 类型；
  `normalizeActivityEvent`（未知 `kind` → 丢弃该行；缺 `occurredAt` → 丢弃；数值用
  `asNumber`，可空字段用 `asStringOrNull`）；`normalizeActivityTimeline`；
  `localTimezoneOffsetMinutes` / `getUsageSummary` 已存在，`getActivityTimeline({range,
timezoneOffsetMinutes, limit})` 直接 `invokeCommand('activity_timeline', {...})`。
- `src/features/history/ActivityPage.tsx`：
  - 页面结构 = `PageHeader` + 范围切换 + 4 个 KPI 卡 + Activity Calendar + Session Timeline 列表；
  - Activity Calendar：`days` 铺成周网格（周一开头），格子用 `maxDayTokens` 归一化出 5 档深浅
    （与 Dashboard 的 `barWidth` 同思路：`max<=0 → 0`），格子带 `aria-label` 与 `title`，
    点击设置 `selectedDay`（再次点击取消）；
  - Session Timeline：`selectedDay === null ? events : events.filter(e => e.day === selectedDay)`，
    每条一行：时间（本地，`HH:mm`）+ kind 徽标 + harness + tokens/cost 或 duration/status；
  - 空态文案区分「这个时间范围内没有活动」与「数据库里还没有任何数据」；
  - 权限/隐私横幅固定显示一句：「时间线按事件发生时间排列，不代表代码由 AI 编写。」（约束 7）
  - 组件里**禁止** `invoke()`，全部走 `src/lib/ipc.ts`。

## 验收

1. **Rust 单测**（`src-tauri/src/activity/`）：
   - 冷启动纪律：至少一条用例从**真正空的 DB** 开始，只用生产代码（`UsageImporter` +
     `SessionService` / `SessionStore`）填充前置状态 —— 禁止 `seeded_db()` 之类的夹具
     替生产代码补 FK 前置（AGENTS.md「测试纪律」，历史上因此出过两次 FOREIGN KEY 事故）。
   - `range_filter` 的 `?N` 绑定：窗口参数从 `?1`（usage）/ `?1`（session）起始的行为被测试锁定，
     并且必须覆盖「tz 修饰符与范围参数同时存在」的组合，防止再犯 summary.rs:294-312 记录的坑。
   - 本地日边界：UTC+8 下 23:30Z 的事件必须落在第二天（沿用 summary.rs 测试的
     `NOW = 2026-09-22T23:30:00Z` / `UTC_PLUS_8 = 480` 习惯）。
   - `duration_ms`：`ended_at` 为 NULL 时为 `None`，非 NULL 时为精确毫秒数；
     负值/异常时间不得出现。
   - usage 事件 `project_id` 只在 `hub_session_id` 能 join 到 `sessions` 时非空
     （不按 `source_session_id` 猜）。
   - 多货币/缺价格：`cost_is_lower_bound` 由后端判定。
   - `has_more`：事件数超过 limit 时为 true，且 `events.len() <= limit`。
   - `hours` 恒为 24 项。
2. **IPC 契约测试**（`src-tauri/src/activity/` 或 `commands.rs::tests`）：序列化后键名与嵌套
   形状被锁死 —— `ActivityTimeline::keys()` 全量断言 + `events[0]` / `days[0]` / `hours[0]`
   的键名 + `assert!(json.get("some_snake_case").is_none())`（AGENTS.md「跨 IPC 的契约纪律」）。
3. **前端测试**（`src/features/history/ActivityPage.test.tsx` + `src/lib/ipc.test.ts`）：
   - 页面 stub 与 Dashboard 同形：任何**未预期**的命令名直接抛错
     （「Activity 不得调用 ${command}」）；挂载/切范围只调用 `activity_timeline`；
   - `normalizeActivityTimeline` 的 fixture test：缺字段/错 `kind`/非对象的安全默认值。
4. **真机测试**（`src-tauri/tests/real_activity_timeline.rs`）：
   - 用裸 `Connection` + `apply_pending` 建真库，经生产写入路径灌入 usage + session，
     再与一条**独立手写**的 SQL 逐项对账（含本地日归属、duration、`has_more`）；
   - 关闭重开后数字不变；
   - 证明查询路径**不起进程**：`timeline()` 只接受 `&Connection`。
5. **Gates**（全部真实输出贴到 `docs/plans/`）：`pnpm format:check` / `pnpm rust:fmt:check` /
   `pnpm lint` / `pnpm typecheck` / `pnpm test` / `pnpm rust:test` / `pnpm python:test` /
   `pnpm build` / `pnpm rust:clippy`。
6. **GUI 验收**：`pnpm tauri dev`（端口 3300 覆盖 + CDP 9222）实际打开 `/activity`，
   截图归档到 `D:\HarnessHub-E2E\`：空态、有数据态、范围切换、点击某天过滤。

## 来源与限制

- `HARNESS_HUB_PROJECT_PLAN.md` §20 顺序（:3726-3748）、§21 Release Gate（:3780-3781
  未勾选的 "Activity Timeline" / "Git Session Snapshot"）、Phase 3 History 未勾选项
  （:2760-2766）、Phase 11 Analytics（:2994-3000）。
- 产品定位 :77「… + Git Activity Timeline」；差异化能力 :440-497（维度枚举 + Git Activity v0.1
  范围 + 「git diff 不能证明 AI 编写」）。
- `docs/CONTEXT.md` 核心约束 7（Git diff ≠ AI 贡献）与 §6「尚未实现」列表。
- `docs/adr/0012-dashboard-reads-only-sqlite.md`（纯读、聚合自明细、usage vs managed 不合并、
  `≥` 后端判定、UTC + offset、timestampless 不静默丢弃）。
- `docs/adr/0011-usage-import-contracts.md`（usage 数据形状与幂等语义）。
- 限制：
  - 固定 `timezone_offset_minutes` 不建模 DST（ADR-0012 已知限制，本步沿用）。
  - 没有 Git 写入方，因此「Git Session Snapshot」仍为空 —— 属第 16 步。
  - usage 事件的项目归属只能来自 `sessions.project_id` 反查，覆盖不全；UI 不补全、不猜测。
