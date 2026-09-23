# fixtures/ccusage

ccusage 的**真实输出结构**记录（Task 5 的取证结果，不靠猜）。
冻结后的契约见 `docs/adr/0011-usage-import-contracts.md`；本文件只保留**原始证据**。

## 取证环境（2026-09-22）

```text
本机 PATH 上没有 ccusage
npx --yes ccusage@20.0.24 --version  →  ccusage 20.0.24
```

PATH 上没有 ccusage 这件事本身就是 `unavailable` 分支的真实素材；同时它说明
`npx --yes ccusage@<pinned>` 是必要的 managed runner（但**不得**用 `@latest` 做生产默认，
理由见 ADR-0011 决策一）。

## `ccusage daily --json`

```jsonc
{
  "daily": [
    // 42 条
    {
      "agent": "all", // daily 模式下是聚合值
      "agents": [
        // ← 只有在 --by-agent 时出现
        { "agent": "claude", "inputTokens": 177789 /* …同 session 行结构… */ },
      ],
      "period": "2026-06-10", // ← 注意：字段名是 period，不是 date
      "inputTokens": 177789,
      "outputTokens": 15230,
      "cacheCreationTokens": 0,
      "cacheReadTokens": 1226752,
      "totalTokens": 1419771,
      "totalCost": 0.03258976559999999, // ← 浮点，可见二进制噪声
      "modelsUsed": ["deepseek-v4-flash"],
      "metadata": { "agents": ["claude"] }, // harness 维度藏在这里
      "modelBreakdowns": [
        {
          "modelName": "deepseek-v4-flash",
          "inputTokens": 177789,
          "outputTokens": 15230,
          "cacheCreationTokens": 0,
          "cacheReadTokens": 1226752,
          "cost": 0.03258976559999999,
        },
      ],
    },
  ],
  "totals": {
    "cacheCreationTokens": 0,
    "cacheReadTokens": 4653831168,
    "inputTokens": 91225213,
    "outputTokens": 12786904,
    "totalCost": 288.8240692563998,
    "totalTokens": 4757844195,
    "unpricedModels": ["GLM-5.3-Flash"], // ← 有模型没有价格：cost 之和是下界
  },
}
```

## `ccusage session --json`

```jsonc
{
  "session": [
    // 222 条
    {
      "agent": "codex", // session 模式下是具体 harness
      "period": "2026/08/30/rollout-2026-08-30T13-30-39-01a05125-febf-7780-90…",
      //         ↑ Codex 的 rollout 身份（与 ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl 对应）
      "inputTokens": 11867786,
      "outputTokens": 1675327,
      "cacheCreationTokens": 0,
      "cacheReadTokens": 817184768,
      "totalTokens": 830727881,
      "totalCost": 21.120615000000004,
      "modelsUsed": ["gpt-5.6-luna"],
      "metadata": {
        "lastActivity": "2026-09-02T11:26:42.287Z", // ← 唯一可靠的发生时间
        "reasoningOutputTokens": 540968, // ← 只在 session 模式下出现
      },
      "modelBreakdowns": [
        {
          "modelName": "gpt-5.6-luna",
          "inputTokens": 11867786,
          "outputTokens": 1675327,
          "cacheCreationTokens": 0,
          "cacheReadTokens": 817184768,
          "cost": 21.120615000000004,
        },
      ],
    },
  ],
  "totals": {/* …同 daily 的 totals 结构… */},
}
```

实测事实：

- **没有独立的 `sessionId` 字段**：session 模式下会话身份就是 `period`。
  Codex 形如 `YYYY/MM/DD/rollout-<ISO>-<uuid>`，Claude 是裸 UUID，OpenCode 是 `ses_…`，
  zcode 是 `sess_<uuid>` —— **不能假定 `period` 里一定有日期**。
- `metadata.lastActivity` 222/222 行都有；`metadata.reasoningOutputTokens` 出现在 208 行。
- 一个 session 行可以有多个 `modelBreakdowns`（实测 20 行如此）。
- 所有 token 字段都是整数；`modelBreakdowns[]` 的键是
  `cacheCreationTokens / cacheReadTokens / cost / inputTokens / modelName / outputTokens`
  （**没有** `totalTokens`；另有只在缺价格时出现的 `missingPricing`）。
- 算术恒等式（**修正过一次**：第一版写「Σbreakdown == 行汇总，222/222 成立」，
  那是没测全就下的结论，实测后不成立）：
  - 四类 token 的 `Σ(modelBreakdowns) == 行值`：**222/222 行成立**；
  - `四类之和 == totalTokens`：**221/222 行成立**，唯一例外是 opencode 的
    `ses_f40ea5cdbffeienAXgo4uTUBsL`（行 `totalTokens` 204906，四类之和 203996，多 **910**）。
    该行的模型明细之间是一致的，矛盾只在上游的行汇总里 —— 我们以四类为准落库，
    把 910 记进 `usage_imports.unattributed_tokens`（ADR-0011 决策十一）。
- 金额舍入残差：每个事件独立舍入到微单位再求和，与把 `totals.totalCost` 整体舍入相比，
  实测在 235 个计价 breakdown 上相差 **+4 微单位**（0.000004 USD）。上限是 ⌈n/2⌉ 微单位，
  可证明；对账时必须打印具体数值，不允许含糊过去。

## 第二轮取证：`--sections` 单次调用（20.0.24 支持）

```bash
ccusage session --sections daily --by-agent --json
```

```text
top-level keys: ["session", "daily", "totals"]   // 同一次数据加载，exit 0，264 KB
session: 222 行（结构与单独跑 session 完全一致）
daily:   42 行（每行多一个 agents[] 细分，agent = "all"）
totals:  同上的 totals 对象
```

`ccusage session --help` 原文：

```text
--sections <sections>   Emit multiple unified report sections from one load (daily, weekly, monthly, session)
--by-agent              Include per-agent JSON breakdowns in unified report rows
```

### 同快照对账（这才是「数字对得上」的证据）

```text
totals == Σ(session rows)   逐项精确相等（含 totalCost 288.8240692563998）

           inputTokens   outputTokens  cacheReadTokens  totalTokens
session      91225213       12786904      4653831168     4757844195
daily        91495409       12800013      4654795392     4759091724
totals       91225213       12786904      4653831168     4757844195   ← 等于 session

差值按 agent 归因（daily 的 agents[] vs session 的 agent）：
agent     daily totalTokens   session totalTokens   diff
claude          1709428              461899        +1247529   ← 差值 100% 在这里
codex        3857313549          3857313549              0
opencode         218444              218444              0
zcode         899850303           899850303              0
```

结论：`daily` 与 `session` 的口径差异**不是**未知噪声，而是完全落在 `claude` 一个 agent 上；
其余 agent 逐 token 相等。因此 reconciliation 可以用「逐 agent 归因」的方式断言，
不允许出现无法解释的残差。

### cost 是可缺的，且总数是下界

- `modelBreakdowns[]` 还有一个只在**缺价格**时出现的字段：

  ```text
  243 个 breakdown 中 8 个带 "missingPricing": true（全部是 GLM-5.3-Flash）
  这 8 个的 cost 都是 0；cost == 0 且没有 missingPricing 的条目：0 个
  行级（session 行）没有 missingPricing
  ```

  即「cost = 0」在 ccusage 里意味着**没有价格**，而不是「真的免费」：导入时必须落成
  `cost_microunits = NULL` + `cost_source = 'ccusage_missing_pricing'`，不能落 0。

- `totals.unpricedModels = ["GLM-5.3-Flash"]`，而 zcode 的 8 条 session 行合计
  899,850,303 tokens。因此 `Σ cost` 是**下界**（有模型没有价格），UI 与对账都必须能说出这一点。

### 失败路径

```text
ccusage session --nope --json   →  exit 2,  stdout/stderr 有 "Unknown session option '--nope'"
```

非零退出必须被记录成**失败的 import**（带 stderr 摘要），不得当成「没有数据」。

## 由此确定的 Task 5 设计（已冻结为 ADR-0011）

1. Runner：PATH → 用户配置 → pinned managed（`ccusage@20.0.24`）→ `unavailable`；**不用 `@latest`**。
2. 主导入源 = `session`，写入粒度 = `session.modelBreakdowns`（一行一个模型）；
   session aggregate 只做校验，`daily` 只做对账 —— 避免双计数。
3. `stable_source_key` = `sha256("ccusage\0v1\0" + report_kind + "\0" + agent + "\0" + period + "\0" + modelName)`，
   versioned；真实 222 行 × 243 个 breakdown 上 **243/243 个键互不相同（0 碰撞）**。
4. `token_source` / `cost_source` / `pricing_mode` 分离；ccusage 的 token **不得**标成 `provider_reported`。
5. `currency = 'USD'` + `currency_source = 'ccusage_contract'`；
   JSON 缺 currency 字段 ≠ 币种未知。
6. 金额走 `serde_json::RawValue` 原文 → ×1e6 → **round-half-even** → `i64 cost_microunits`；
   浮点噪声 fixture 锁死 `"0.03258976559999999" → 32590`。
7. 优先单次 `--sections` 调用；退化时在 `usage_imports` 记录两次采集时间并标注 near-snapshot。
8. `hub_session_id` 只在可证明对应时填，否则 `NULL`。
9. `occurred_at` 来自 `metadata.lastActivity`（`source_record`）；不可推导时为 `NULL`
   且 `occurred_at_source = 'unavailable'`，**不得**用导入时间冒充。
10. 显式 shape detect：非 unified v20.0.24 形状（例如只有 `sessionId`）报 `unsupported_shape`。

## 本目录的夹具

| 文件                     | 用途                                                                                       |
| ------------------------ | ------------------------------------------------------------------------------------------ |
| `session.json`           | 主路径：`session` + `totals`，4 个 agent、含多模型行、reasoning tokens、`missingPricing`   |
| `sections.json`          | 单次 `--sections daily --by-agent` 形状：`session` + `daily` + `totals`（子集自洽）        |
| `noisy-cost.json`        | 决策六的金额边界 fixture（浮点噪声、`e` 记法、half-even、`missingPricing`）                |
| `empty.json`             | 合法但零 session → 必须是成功的空导入，不是错误                                            |
| `unsupported-shape.json` | 合法 JSON 但只有 `sessionId`、没有 `period`/`modelBreakdowns` → 必须报 `unsupported_shape` |

截断 / 语法错误的 JSON 不放夹具文件（`prettier --check .` 会覆盖 `fixtures/**/*.json`，
放进来的话整个仓库的 format gate 会被一个故意损坏的文件卡住），改为在测试里内联字符串。

脱敏规则：`period` 里的 rollout UUID 换成合成 UUID（形状不变，`YYYY/MM/DD/rollout-<ISO>-<uuid>` 保留）；
删除任何绝对路径；模型名保留（它们是公开模型标识，且是 natural key 的一部分）。
