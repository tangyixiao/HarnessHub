# ADR-0011 — Usage 导入契约：外部优先、粒度明确、幂等、金额不走近浮点

- **Status**: Accepted（2026-09-22）
- **Context**: Task 5 要把 `ccusage → discover/invoke → JSON → normalize → idempotent import → SQLite`
  变成可信数据管道。取证结果见 `fixtures/ccusage/README.md`（ccusage 20.0.24 真实输出，
  不是文档推测）。取证推翻了早期计划里的三处假设：字段是 `period` 而不是 `date`；
  session 模式**没有** `sessionId`；cost 是带二进制噪声的 float。
  本 ADR 冻结七条契约，实现必须逐条可测。
- **Related**: ADR-0002（Adapter 分离）、ADR-0003（Local-first）、ADR-0004（SQLite 演进）、
  ADR-0008（LaunchSpec 边界，本次只是被旧文档误引为 Token 优先级来源）。

## 决策一：Runner 解析顺序固定，生产默认**不**用 `@latest`

```text
1. PATH 上的 ccusage
2. 用户显式配置的 executable + args
3. managed runner：固定版本（当前 pin 到 ccusage@20.0.24）
4. 都不可用 → source status = unavailable（正常状态，不是崩溃）
```

`npx --yes ccusage@latest` **只用于取证**。把它放进生产管道等于让 schema 在未来某天
无声变化后再由我们的解析器去猜。自动升级是独立的 upgrade flow，不混进数据管道。

可观测性要求：一次 import 必须记录**实际用的是哪个 runner**（`runner` 字段）与
`source_version`，否则「数字对不上」时无法定位是数据变了还是 runner 变了。

## 决策二：导入粒度 = `session.modelBreakdowns`，session aggregate 只做校验

`session` 模式是主数据源（它有 `agent` = harness，有 `period` = 外部会话身份）。
但写进 `usage_events` 的**一行 = 一个 `modelBreakdowns[]` 条目**，这样一行对应一个明确模型：

```text
session row (agent=codex, period=2026/08/20/rollout-…)
├─ modelBreakdowns[0] modelName=gpt-5.6-sol   → UsageEvent #1
└─ modelBreakdowns[1] modelName=gpt-5.6-terra → UsageEvent #2
```

session 行的 aggregate（`inputTokens` / `totalTokens` / `totalCost`）**不导入**，只用于
校验：`Σ breakdown == row aggregate`（222/222 行实测成立，多模型行包含在内）。

`daily` 分段**永不导入**：它和 `session` 是同一批数据的不同聚合口径，两者都导入必然双计数。
`daily` 只作为 reconciliation 的第二个视角。

## 决策三：stable key 是 versioned canonical hash，不是字符串拼接

```text
key_version = 1
stable_source_key = sha256( "ccusage\0v1\0" + report_kind + "\0" + agent + "\0" + period + "\0" + modelName )
```

理由：`agent` / `period` / `modelName` 都可能含分隔符（`period` 里就有 `/`），
朴素拼接会产生歧义从而产生**伪碰撞**；先长度无关地规范化再 hash，并显式带 `key_version`
与 `report_kind`。以后 ccusage 增加新的身份维度时**升 version 并新增一列语义**，
不允许悄悄改算法（那会让历史行的 key 全部漂移）。

实测：真实 222 条 session × 其全部 modelBreakdowns 上**无碰撞**；
`codex/gpt-5.6-sol` 与 `codex/gpt-5.6-terra` 必须落在不同的 key 上。

## 决策四：`token_source` 与 `cost_source` / `pricing_mode` 分离

| 列             | ccusage 导入时的值                      | 含义                                                     |
| -------------- | --------------------------------------- | -------------------------------------------------------- |
| `token_source` | `ccusage_source_log`                    | token 数来自「外部聚合器读取 Harness 本地日志」          |
| `cost_source`  | `ccusage_computed`                      | 金额由 ccusage 侧算出，不是 Harness 或 Provider 直接给的 |
| `pricing_mode` | `auto`（默认）/ `calculate` / `display` | 对应 ccusage 的三种 cost mode                            |

**ccusage 导入的 token 不得标成 `provider_reported`。** 它是外部聚合器读取本地日志的结果，
与「Harness Hub 自己从 Provider 响应里拿到 usage」不是一回事。Token Truth 优先级
（`provider_reported > provider_count_api > local_exact > estimated`）只有在两者都存在时才有意义，
当时刻区分来源，所以来源必须存成列而不是注释。

## 决策五：currency 明确为 `USD`，依据来自 ccusage 契约

ccusage 的统一 JSON 行里**没有** currency 字段，但它的 cost 在文档、CLI 表头（`Cost (USD)`）
与 README 中都明确是 USD。因此：

```text
currency        = 'USD'
currency_source = 'ccusage_contract'
```

「JSON 里没有 currency 字段」**不等于**「币种未知」。币种随源契约确定，并连同
`currency_source` 一起落库 —— 将来接入非 USD 的来源时，读者能立刻认出哪一行是**假设**而不是**数据**。

## 决策六：金额在边界转成整数微单位，规则固定、可测

```text
JSON 原始十进制文本（serde_json RawValue，取到的是 21.120615000000004 这样的原文）
  → × 1_000_000
  → round-to-nearest, ties-to-even
  → i64  →  usage_events.cost_microunits INTEGER
```

- **不允许** `f64 as i64`，也不允许先把原文变成 f64 再乘 —— 那正是
  `0.03258976559999999` 这类噪声变成不可预测结果的路径。
- 浮点噪声 fixture 必须锁死：`"0.03258976559999999"` → `32590`。
- cost 可缺，并且 ccusage 用**显式字段**告诉你它缺：

  ```text
  modelBreakdowns[].missingPricing === true   → cost_microunits = NULL, cost_source = 'ccusage_missing_pricing'
  否则 breakdown.cost 存在                      → 走上面的十进制转换
  ```

  实测：243 个 breakdown 里 8 个 `missingPricing: true`（全部是 `GLM-5.3-Flash`），
  它们的 `cost` 都是 `0`，而**没有任何** `cost == 0 && missingPricing 缺失` 的条目 ——
  也就是说「cost 0」在 ccusage 里等于「没有价格」，不等于「真的免费」。
  把 `missingPricing` 的 0 当 0 落库会静默抹掉这个事实，因此必须落成 `NULL`。
  `Σ cost` 因此是**下界**（`totals.unpricedModels = ["GLM-5.3-Flash"]`），
  UI 与对账都必须能说出这一点。`cost` 字段本身缺失或为 `null` 时同样落 `NULL`（防御性）。

## 决策七：优先单次 invocation，`totals` 是同快照真值

实测 `ccusage session --sections daily --by-agent --json`（20.0.24）**可用**，一次调用返回
`session` + `daily` + `totals`，且**三个分段来自同一次数据加载**：

```text
一次 invocation
├─ session  → 真正导入
├─ daily    → reconciliation 的第二个视角（不导入）
└─ totals   → 同快照真值
```

若某个未来版本不再支持 `--sections`，退化为两次调用，但必须在 `usage_imports` 里分别记录
两次采集时间，并把对账结论明确标注为 **near-snapshot**（两次调用之间本地日志可能变化）。

### 对账口径（实测，必须能原样复述）

```text
totals == Σ(session rows)          逐项精确（input / output / cacheCreation / cacheRead / totalTokens / totalCost）
daily  != session                  差值 100% 落在 agent=claude 一个 agent 上
                                   codex / opencode / zcode 逐 token 相等
                                   claude: daily 1,709,428 vs session 461,899（差 1,247,529 totalTokens）
totals.unpricedModels = ["GLM-5.3-Flash"]  → cost 总数是下界，不是全量
```

所以 Harness Hub 的对账断言是**可证伪的**而不是「差不多」：

1. `Σ(本次导入的 usage_events) == totals`（同快照、同口径，逐项相等）；
2. 时间戳无法推导的事件必须**被单独计数并解释**，不允许混进差值里蒙过去；
3. 与 `daily` 的差异必须**归因到具体 agent**（当前版本：仅 claude），
   不允许出现「未知来源的差异」。

## 决策八：绝不伪造 hub session

`source_session_id = period`；`hub_session_id` 只有在**可证明**对应（同一 harness +
同一 `source_session_id` 已在 `sessions` 表中存在）时才填，否则 `NULL`。
「本地日志里有一条 2026/08/30 的 codex rollout」不等于「Harness Hub 运行过这个会话」，
时间窗口相近也不是证据。历史数据不得反向生成假的 Harness Hub 会话。

## 决策九：时间戳来源必须落库，不允许用导入时间冒充发生时间

session 行的 `period` 不都带日期（claude 是裸 UUID，opencode 是 `ses_…`），所以：

```text
occurred_at_source = 'source_record'  ← metadata.lastActivity（实测 222/222 行都有）
                   | 'period'         ← 仅当能从 period 里可靠解析出日期
                   | 'unavailable'    ← occurred_at = NULL
```

`day` 由 `occurred_at` 推导；`occurred_at` 为 NULL 时 `day` 也为 NULL。
**不得**用 `imported_at` 填 `occurred_at`：那会把历史数据搬到今天，污染按天聚合。

## 决策十：schema shape 显式识别，不做「神秘解析失败」

第一版只支持取证过的 **unified v20.0.24 session shape**（行内有 `period` + `modelBreakdowns`）。
ccusage 同时存在 focused 文档里展示的 `sessionId` 语义，因此 adapter 必须先做 shape detect：

```text
有 session 数组 + 行含 period + modelBreakdowns  → supported（v1）
其他形状（例如只含 sessionId 的行）              → unsupported_shape，错误信息说明期望形状
```

不允许出现「`sessionId` 被当空气、数字静默变少」这种失败方式。

## Alternatives

- **直接导入 session aggregate**：少一层循环，但一个 session 用多个模型时就丢失了模型维度，
  而 Dashboard 的核心诉求恰恰是 Model Breakdown。拒绝。
- **直接用字符串拼 key**：实现最简单，但 `period` 含 `/`、模型名未来可能含分隔符，
  歧义碰撞是不可检测的静默错误。拒绝。
- **`cost_usd REAL`**：与「金额不用浮点」冲突，且会让 `ORDER BY cost` 出现脏比较。
  拒绝（迁移 0006 重建 `usage_events`，理由见该迁移头部注释）。
- **为每个 Harness 自写 token parser**：与 ADR-0002 / ADR-0003 的外部优先原则冲突。拒绝。

## Consequences

- `usage_events` 的旧形状（`dedupe_key` / `cost_usd` / `cost_estimated` / `model_id` FK）
  被迁移 0006 重建为 `stable_source_key` / `cost_microunits` / `currency` / `currency_source`
  / `token_source` / `cost_source` / `pricing_mode` / `occurred_at_source`。
  旧表在**所有已发布版本中都没有写入方**，迁移内含「非空则拒绝执行」的守卫。
- ccusage 不再是「Usage 的唯一真值」而是「一个来源 + 一份可对账的快照」；
  Dashboard（Task 6）只读 Harness Hub SQLite，**不得**在渲染时调用 ccusage。
- 新增 `serde_json` 的 `raw_value` feature 与 `sha2` / `hex` 直接依赖（均已在依赖图中）。

## Evidence

- `fixtures/ccusage/README.md`：两轮真实取证（单模式 + `--sections` 单次调用）的原始结构。
- `src-tauri/tests/real_ccusage_import.rs`：真实机器上跑 `--sections` 单次调用 → 导入 →
  与同快照 `totals` 逐项对账 → 打印每一条差异的归因。
- 迁移 0006 的裸 `Connection` 迁移测试；`usage::key` 的碰撞测试。

## Revisit Conditions

- ccusage 出现带 `sessionId` 的新 unified shape → 升 `key_version` 并新增 shape detector 分支。
- 接入 `provider_reported` token（Harness 原生 usage）→ 重新审视 Token Truth 择优查询，
  但本 ADR 的来源列不变。
- 需要非 USD 来源 → `currency_source` 已预留，但需要新的换算与取整契约。
