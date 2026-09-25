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

session 行的 aggregate（`inputTokens` / `totalTokens` / `totalCost`）**不导入**，只用于校验。
校验分两级，依据是**实测**而不是直觉（真实 222 行 / 243 个 breakdown）：

```text
四类 token：Σ(breakdown) == 行值     222/222 行成立 → 不成立直接报错（说明我们读错了 ccusage）
行 totalTokens == 四类之和           221/222 行成立 → 唯一例外见决策十一
```

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

实测：真实 222 条 session × 243 个 modelBreakdown 上 **243/243 个键互不相同，0 碰撞**
（该断言已固化成真机 E2E 的一部分，可重复验证）；
`codex/gpt-5.6-sol` 与 `codex/gpt-5.6-terra` 必须落在不同的键上。

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

> **注意（2026-09-25 修订）**：「同一次数据加载」**不等于**「冻结快照」。写入方活跃时，
> 同一次调用内部先算的 `daily` 与后算的 `session` 也会不一致 —— 详见本决策后面的
> 「修订（2026-09-25）」一节。该节同时给出稳定窗口的适用边界。

若某个未来版本不再支持 `--sections`，退化为两次调用，但必须在 `usage_imports` 里分别记录
两次采集时间，并把对账结论明确标注为 **near-snapshot**（两次调用之间本地日志可能变化）。

### 对账口径（实测，必须能原样复述）

```text
totals == Σ(session rows)          逐项精确（input / output / cacheCreation / cacheRead / totalTokens / totalCost）
daily  != session                  差值 100% 落在 agent=claude 一个 agent 上
                                   codex / opencode / zcode 逐 token 相等（**仅当加载期间数据不变**，
                                   写入方活跃时不成立 —— 见「修订（2026-09-25）」）
                                   claude: daily 1,709,428 vs session 461,899（差 1,247,529 totalTokens）
totals.unpricedModels = ["GLM-5.3-Flash"]  → cost 总数是下界，不是全量
行 totalTokens vs 四类之和          221/222 相等；唯一例外是 opencode 的 +910（决策十一）
金额：独立舍入 + 求和 vs 整体舍入   实测 235 个计价 breakdown 上相差 +4 微单位（0.000004 USD）
```

所以 Harness Hub 的对账断言是**可证伪的**而不是「差不多」：

1. `Σ(事件 total) + 明示差额 == totals.totalTokens`（精确，不是近似）；
2. `Σ(事件 cost) - totals.cost` 的残差必须 ≤ ⌈计价 breakdown 数 / 2⌉ 微单位，
   并且**具体数值要打印出来**（每个事件独立舍入，量级上限是可证明的）；
3. 时间戳无法推导的事件必须**被单独计数并解释**，不允许混进差值里蒙过去；
4. 与 `daily` 的差异必须**归因到具体 agent**（当前版本：仅 claude），
   不允许出现「未知来源的差异」。

### 修订（2026-09-25）：一次 invocation ≠ 冻结快照

写 Task 8C（CI 修复）时实测到：**写入方活跃时**，同一次 ccusage 调用内部的 `daily` 与
`session` 就已经不一致，因为两个分段是两次读取，中间数据仍在追加。数据源是本机 codex 的
rollout 文件（`%USERPROFILE%\.codex\sessions\<date>\rollout-*.jsonl`，只追加）：

```text
同一份报告内部    codex daily - session = -166271 / -84090 / 0
                  （第三次恰好为 0，因为那一刻写入暂停 —— 差异正好等于两次读取之间的增量）
跨两次调用        totals.totalTokens 在 11.8s 内 +36920；另一次 87s 内 +968567
                  行数不变、其中 3 个 rollout 行被上游改写（21178283 → 21875738 等）
```

因此「同快照」的适用范围必须按下面写：

```text
成立        totals == Σ(session rows)             同一次加载内部的算术恒等式（原实测依然有效）
成立        daily vs session 差异逐 agent 归因    这是内部一致性检查，与数据是否在变无关
不成立      codex daily == session「逐 token 相等」  需要「加载期间数据不变」，即冻结快照
            （写入方活跃时它必然可能不成立；原始实测是在安静机器上得到的）
```

对**测试/对账**的边界（实现只落在测试侧，不碰生产代码）：

- 要比较 `daily` 与 `session` 的真机对账，取数必须落在**稳定窗口**：
  `--until <UTC 今天-2 天> -z UTC`。`-2 天` 保证任何时区下上界距现在都 ≥24 小时，
  `-z UTC` 让日期分组不依赖机器时区。**代价**：不覆盖进行中的那一天。
- 只需要「两次导入是同一份数据」（幂等）的测试**不加**这个上界：replay 同一份输出就够了；
  加上界反而会丢掉今天的覆盖，甚至让「源头每个 key 都在库里」的逐 key 检查空转通过。
- 想连进行中的那天一起覆盖，只有**冻结数据副本**一条路（复制数据目录并让 ccusage 指向副本），
  v0.1 不做。

落点：`src-tauri/tests/common/mod.rs` 的 `ReplaySections`（真实跑一次 + 按**完整命令**匹配后
replay；`--version` / detect 探针仍走真实 runner）与 `CaptureWindow`（`Full` / `StablePast`），
三个真机用例按各自需要选窗口。

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

## 决策十一：上游自己矛盾时，差额被**计数**而不是被摊派

实测（222 行里唯一的一处）：opencode 的 `ses_f40ea5cdbffeienAXgo4uTUBsL` 行
`totalTokens = 204906`，而它自己的四类之和是 `203996` —— 多出 **910**，
并且这两者的差**只**出现在行汇总上，模型明细之间是一致的。

处理规则：

```text
四类 token 与行不一致        → 报错（说明我们读错了）
行 totalTokens > 四类之和    → 以四类为准落库，差额记进 usage_imports.unattributed_tokens
行 totalTokens < 四类之和    → 报错（明细不可能比汇总多）
```

差额**绝不分摊给模型**（分摊就是编造），也**绝不静默丢弃**（那样对账会出现无法解释的差额）。
于是恒等式精确成立：`Σ(事件 total) + unattributed_tokens == 来源 totals.totalTokens`。

`unattributed_tokens` 由迁移 0007 加进 `usage_imports`（ADD COLUMN，老行补 0）。

## 决策十二：无法按模型拆分的信息不落库，也不分摊

`metadata.reasoningOutputTokens` 是 **session 级**、ccusage 不提供按模型拆分
（实测 243 个 breakdown 里没有任何逐模型推理 token 字段）。因此：

```text
行内只有一个模型  → reasoning_tokens = metadata.reasoningOutputTokens
行内有多个模型    → reasoning_tokens = NULL（没有依据分摊，宁可缺也不要编）
```

同理，`raw_payload` 列先留着但**不写**：需要的字段都已显式建模，
等到真有来源带我们没建模的字段时再写原文，而不是把整份报告复制进每一行。

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
- `usage_imports` 多了 `unattributed_tokens`（迁移 0007），用于精确对账。

## Evidence

- `fixtures/ccusage/README.md`：两轮真实取证（单模式 + `--sections` 单次调用）的原始结构。
- `src-tauri/tests/real_ccusage_import.rs`：真实机器上跑 `--sections` → 导入 →
  与同快照 `totals` 逐项对账 → 打印每一条差异的归因（含 910 与金额残差）。
  该用例按「修订（2026-09-25）」使用稳定窗口，因此对账覆盖的是已结束的日期。
- 迁移 0006 / 0007 的裸 `Connection` 迁移测试；`usage::key` 的碰撞测试。
- 单次调用内部 `daily` 与 `session` 不一致的实测（`-166271 / -84090 / 0`）与跨调用增量
  （11.8s / +36920）记录在「修订（2026-09-25）」一节，测试侧的固定与窗口选择见
  `src-tauri/tests/common/mod.rs`。

## Revisit Conditions

- ccusage 出现带 `sessionId` 的新 unified shape → 升 `key_version` 并新增 shape detector 分支。
- 接入 `provider_reported` token（Harness 原生 usage）→ 重新审视 Token Truth 择优查询，
  但本 ADR 的来源列不变。
- 需要非 USD 来源 → `currency_source` 已预留，但需要新的换算与取整契约。
- ccusage 的 `--until` 若支持**带时刻**的粒度（现在只有 `YYYY-MM-DD`），稳定窗口的代价
  （丢掉最近 ≥24 小时）可以压到几秒；届时应重估「冻结数据副本」那条路是否还需要。
