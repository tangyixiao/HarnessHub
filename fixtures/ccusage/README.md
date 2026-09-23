# fixtures/ccusage

ccusage 的**真实输出结构**记录（Task 5 的取证结果，不靠猜）。

## 取证环境（2026-09-22）

```text
本机 PATH 上没有 ccusage
npx --yes ccusage@latest --version  →  ccusage 20.0.24
```

因此 `CcusageAdapter` 必须同时支持「PATH 上的 ccusage」与「可配置的调用命令
（例如 `npx ccusage@latest`）」，并在两者都不可用时如实报告 `unavailable`。

## `ccusage daily --json`

```jsonc
{
  "daily": [                       // 42 条
    {
      "agent": "all",              // daily 模式下是聚合值
      "period": "2026-06-10",      // ← 注意：字段名是 period，不是 date
      "inputTokens": 177789,
      "outputTokens": 15230,
      "cacheCreationTokens": 0,
      "cacheReadTokens": 1226752,
      "totalTokens": 1419771,
      "totalCost": 0.03258976559999999,   // ← 浮点，可见二进制噪声
      "modelsUsed": ["deepseek-v4-flash"],
      "metadata": { "agents": ["claude"] },   // harness 维度藏在这里
      "modelBreakdowns": [
        {
          "modelName": "deepseek-v4-flash",
          "inputTokens": 177789,
          "outputTokens": 15230,
          "cacheCreationTokens": 0,
          "cacheReadTokens": 1226752,
          "cost": 0.03258976559999999
        }
      ]
    }
  ],
  "totals": { /* 存在，可用于同口径 reconciliation */ }
}
```

## `ccusage session --json`

```jsonc
{
  "session": [                     // 222 条
    {
      "agent": "codex",            // session 模式下是具体 harness
      "period": "2026/08/30/rollout-2026-08-30T13-30-39-01a05125-febf-7780-90…",
      //         ↑ Codex 的 rollout 身份（与 ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl 对应）
      "inputTokens": …, "outputTokens": …,
      "cacheCreationTokens": …, "cacheReadTokens": …, "totalTokens": …,
      "totalCost": …,
      "modelsUsed": [...], "metadata": {...}, "modelBreakdowns": [ { "modelName", "cost", … } ]
    }
  ],
  "totals": { /* 存在 */ }
}
```

**没有独立的 `sessionId` 字段**：session 模式下会话身份就是 `period`
（Codex 是 rollout 路径片段，其他 Harness 形态不同）。

## 由此确定的 Task 5 设计决定

1. **以 `session` 模式为主数据源**：它有 harness（`agent`）与会话身份（`period`），
   正是 `usage_events` 需要的粒度。`daily` 模式只用于**同口径 reconciliation**，
   两模式同时导入会重复计数。
2. **`stable_source_key`** = `ccusage` + `agent` + `period` + `modelName`
   （不额外掺入时间戳等脆弱字段；ccusage 没给更稳的 source id 时这才可用）。
3. **绝不伪造 hub session**：会话身份进 `source_session_id`；
   只有当本地已有可证明对应的 Harness Hub Session 时才填 `hub_session_id`，
   否则为 `NULL`。
4. **金额不走浮点**：ccusage 给的是带噪声的 float
   （`0.03258976559999999`，实测可见），在**边界**转成整数微单位
   （`cost_microunits`）并显式记录 `currency`；ccusage 输出里没有 currency 字段，
   这一点必须作为**显式假设**记录，而不是默认用户知道。
5. **`totals` 用于对账**：Harness Hub 只统计有时间戳/会话身份的记录，
   ccusage 的 `totals` 可能包含聚合行 —— 口径差异必须解释，不能"差不多就行"。
6. **provenance 必须可追**：`usage_events.import_id` +
   `usage_imports.source_version`（`ccusage 20.0.24`）+ `token_source`
   （沿用 ADR-0008 的优先级），让每个数字都能回答"从哪来"。

## 待补

- 脱敏后的**真实 fixture**（本目录下 `daily.json` / `session.json`，
  去掉路径与模型名中的可识别信息）—— 下一步实现时一起提交，并由它驱动归一化测试。
