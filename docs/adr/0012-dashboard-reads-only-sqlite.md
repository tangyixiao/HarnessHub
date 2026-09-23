# ADR-0012 — Dashboard 是 SQLite 的只读投影，不是 ccusage 的前端

- **Status**: Accepted（2026-09-23）
- **Context**: Task 5 结束时，`usage_events` 里已经有可对账的明细，`usage_imports` 里已经有
  审计与 provenance。Task 6 要做的是「把已有事实显示出来」，因此最大的风险不是画不出来，
  而是**把三个不同职责混在一起**：
  ```text
  明细（usage_events）        = source of truth
  summary（聚合结果）         = projection，随时可由明细重算
  ccusage totals              = Task 5 的 reconciliation evidence，不是展示用的事实来源
  ```
  另一类风险是把「外部历史里的会话数」和「Harness Hub 管理的会话数」混成一个数字。
- **Related**: ADR-0003（local-first）、ADR-0005（capability vs readiness）、
  ADR-0011（usage 导入契约）。

## 决策一：渲染路径**禁止**任何外部调用

```text
usage_events / sessions / projects
        ↓ read-only SQL aggregation
   UsageSummary DTO
        ↓ Tauri IPC
   Dashboard（React）
```

mount / reload / 切换时间范围**全部是纯读**，只走 `usage_summary`。
`ccusage`、`npx`、`UsageSourceAdapter`、`UsageImporter` 都不允许出现在这条路径上：
它们会在一次页面渲染里下载包、起子进程、写数据库 —— 既慢又让「看」变成「改」。

数据更新的唯一入口是用户显式点击「刷新用量」，它调用 Task 5 的 `refresh_usage`，
**成功之后重新查询** SQLite。UI 不得自动触发它（包括 mount 时）。

可验证性：前端测试用「按命令名分派、遇到未预期命令就抛错」的桩，
因此只要 Dashboard 偷偷调用了别的命令，测试就会失败。

## 决策二：聚合必须从明细算

`SELECT ... FROM usage_events GROUP BY ...`，**不读** `usage_imports` 里任何总计字段，
也不读 ccusage 的 `totals`。理由：一旦 summary 从缓存总计算，三件事会立刻分不清 ——
对账失败时不知道该信谁；新增来源时无法合并；`daily`/`session` 双口径的坑会重演。

## 决策三：`usage_sessions` 与 `managed_sessions` 是两个概念，不许合并

```text
usage_sessions    = 时间窗内 usage_events 里 distinct (harness, source_session_id)
                    「ccusage 的历史里有多少个会话」
managed_sessions  = 时间窗内 sessions 表的行数（按 started_at）
                    「Harness Hub 自己启动/管理过多少个会话」
```

导入历史数据只会增加前者。把两者合成一个 `Sessions` 会让用户以为
「Harness Hub 跑过 222 个会话」，那是错的。UI 上必须分开显示。

## 决策四：`≥` 由后端判定，前端只负责格式化

```text
cost_microunits    = SUM(cost_microunits)（只累加有价格的记录；NULL 不参与求和）
cost_is_lower_bound = EXISTS(时间窗内任一 cost_microunits IS NULL 的记录)
missing_pricing_records = COUNT(那些记录)
```

前端只有两种渲染：`false → $12.34`、`true → ≥ $12.34`，外加一句
「部分记录缺少价格，实际成本可能更高」。
**已知价格为 0 且存在缺价格记录时，必须显示 `≥ $0.00` 而不是 `$0.00`**：
「0」和「至少 0 但可能更多」不是同一个事实。
前端不得自己遍历 rows 去猜下界（那是把判定逻辑复制两份）。

## 决策五：时间一律存 UTC，查询显式带时区

```text
usage_events.occurred_at / day   都是 UTC
UsageQuery { range, timezone_offset_minutes }
range ∈ { today, 7d, 30d, all }
timezone_offset_minutes = 加到 UTC 上得到本地时间的分钟数（UTC+8 → +480）
```

时间边界由**后端**从可注入的 `now` 计算（`[start, end)` 左闭右开），
timeline 的「本地日」用 SQL 的 `date(occurred_at, '<offset> minutes')`。
「Today」因此是**本地**的今天，而不是 UTC 的今天 —— 否则晚上 8 点之后使用记录会跑到第二天。

已知局限（明确记录，不假装支持）：固定偏移不建模 DST 切换，
切换当天的边界可能差 1 小时。要修就得引入 tz 数据库，属于后续版本。

## 决策六：没有时间戳的事件不被静默丢弃

Task 5 允许 `occurred_at IS NULL`（`occurred_at_source = 'unavailable'`）。这类事件：

```text
range = all        → 计入总量
range = 有界窗口   → 无法归入任何一天，**排除**出窗口
```

两种情况都返回 `timestampless_records`（窗口内）与 `excluded_timestampless`（被窗口排除掉的），
让「All 的总量 > 各窗口之和」永远有解释。

## `UsageSummary`（冻结）

```text
UsageSummary
├─ range { kind, startUtc?, endUtc?, timezoneOffsetMinutes, nowUtc }
├─ totals { inputTokens, outputTokens, cachedInputTokens, cacheCreationTokens, reasoningTokens, totalTokens }
├─ costMicrounits?      // 没有任何有价格的记录时为 null（不是 0）
├─ currency?            // 有金额时恒为 "USD"，与 ADR-0011 决策五一致
├─ costIsLowerBound, missingPricingRecords, timestamplessRecords, excludedTimestampless
├─ eventCount
├─ usageSessions, managedSessions
├─ byHarness[] / byModel[] / byProject[]   // { key, totalTokens, costMicrounits?, costIsLowerBound, eventCount }
└─ timeline[]                              // { day(本地日), totalTokens, costMicrounits?, costIsLowerBound, eventCount }
```

## 明确的非目标

年度报告、排行榜、Sankey、成本预测、跨设备聚合 —— 属于后续 Analytics，本 ADR 不做。
Dashboard 只显示「已经存在的事实」，不做推断。

## Consequences

- Dashboard 可以离线工作：ccusage 卸载了、断网了，数字照样在。
- 新增 Usage 来源（`provider_reported` 等）时，只要写进 `usage_events`，Dashboard 自动包含它，
  不需要改 Dashboard。
- `usage_summary` 变成跨 IPC 的稳定 DTO，因此必须有 Rust 序列化契约测试 + TS fixture 测试。
- 每次查询都重算聚合：v0.1 的数据量（十万级事件）下 SQL 聚合足够快；
  真出现性能问题再加物化视图，而不是先把 summary 缓存起来（那会重新引入「信谁」的问题）。

## Evidence

- `src-tauri/tests/real_dashboard_summary.rs`：真机导入后的 SQLite → summary →
  与**独立手写 SQL** 交叉验证 → 重启后一致 → 渲染路径未起进程。
- `src/features/dashboard/DashboardPage.test.tsx`：空库、有数据、`≥`、混合成本、
  切换范围只发 `usage_summary`、刷新用量才调 `refresh_usage`。
