# Tool Call 展示 + Subagent 树（ROADMAP B8）Spec

## Problem

B6 回放把 tool_use 折叠成计数、把 sidechain 折叠成徽标；B7 搜索命中工具活跃的
会话后，用户想知道「这个会话到底跑了哪些工具、子代理结构是什么」。
`tool_calls` 表（migration 0001）闲置至今。

## Locked Goal

Replay 页消息流内展开 tool_use（工具名 + 输入摘要，**只读**）；
会话级「子代理」视图按 sidechain 分组渲染树形结构。
数据仍按需现读（不落库、不建索引），与 B6 同一纪律。

## Acceptance Criteria

- [ ] `replay::claude::read_session` 升级：`ReplayMessage` 增加
      `tool_calls: Vec<ReplayToolCall>`（name + input_json 摘要 ≤ 2 KiB/条，
      原样 JSON 字符串不解析语义）；tool_use 不再进 unsupported 计数。
- [ ] `ReplaySession` 增加 `subagent_threads: Vec<SubagentThread>`：
      按 isSidechain 行的 parentUuid 链分段（每段一条线程，含首末时间戳与消息
      seq 引用），主线消息照旧平铺。
- [ ] fixture 扩展（basic.jsonl 已有 tool_use 与 sidechain，补 tool_result 行形状
      ——本机取证有 `tool_result` content 类型，计入 tool_calls 的 result 关联）。
- [ ] DTO 契约测试更新（camelCase、嵌套形状）；TS normalize + 页面测试：
      工具卡展开、子代理树渲染、无工具/无子代理的会话不渲染空区块。
- [ ] capability 不变（replay 已覆盖；不开新能力位）。

## Non-goals

- 不解析工具语义（Grep/Bash 的参数含义）；tool_result 大输出截断为引用。
- 不做跨会话的工具用量统计（H 阶段 Analytics）。
- 不把 tool_calls 写库（0001 的表留给 G24 Canonical Event 决策）。

## Decision

继续「现读现渲染」。B6 已证明单文件 ≤16 MiB 有界读取在真实数据上
（68 会话、最大 ~5 MB）体验可接受；引入持久层反而要先回答
「tool_result 与消息的关联 schema」——那是 G24 的事。

## Risks

- tool_result 可能引用大 blob → 一律截断摘要 + 计数，绝不全文入库/入 UI。

## Verification

fixture 回归 + 契约 + `pnpm test` + 全量门 + 真机 smoke 扩展断言。
