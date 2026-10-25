# 回放内容全文搜索 B7 Spec

## Problem

ROADMAP B7：68+ 条会话转录能在 Replay 页逐条看（B6），但找不到「哪次会话里
讨论过 X」。需要全文搜索；同时 §12 隐私纪律要求支持排除项目 / 关闭内容索引。

## Locked Goal

Replay 页提供搜索框：输入关键字 → 跨已索引的 Claude 转录返回命中
（会话 + 消息片段）；索引是**显式、可撤回**的：全局开关（默认关）、
按 project_dir 排除；关闭后可一键清空已索引内容。

## Acceptance Criteria

- [ ] migration 0011：`replay_search_index`（FTS5 虚拟表，若 bundled SQLite
      不含 FTS5 则降级普通表 + LIKE，由测试判定并在本 spec 回填事实）+
      `replay_index_state`（session_key PK、size+mtime 指纹、indexed_at）+
      `replay_search_settings`（单行：enabled、excluded_dirs）。
- [ ] `replay::index`：`reindex(roots, settings)`——发现列表里跳过已索引指纹
      （增量）；排除目录与关闭状态不读内容；索引用 B6 的解析器（textParts +
      thinkingParts 分列或合并可定位）。**只读源文件，只写自己的表。**
- [ ] IPC：`replay_search_settings_get/set`（set 带 enabled=false 时清空索引）、
      `replay_search_reindex`（显式动作，不起自动后台任务）、
      `replay_search(query) -> Vec<命中>`（snippet + session 元数据）。
      查询参数不合法报错，不静默全量。
- [ ] 前端：Replay 页设置区（开关 + 排除目录 + 重建索引按钮 + 已索引计数）、
      搜索框 + 结果列表（点结果跳到对应会话回放）。关闭索引时搜索框不可用
      并说明原因。
- [ ] 测试：迁移（裸 Connection 升级 10→11）、增量（二次 reindex 不重复）、
      排除目录不读文件（以「未读」断言——size 指纹不写入）、关闭清空、
      契约测试、TS fixture 测试。
- [ ] 冷启动真路径：空库 → 生产代码启用 → reindex → 搜索命中。

## Non-goals

- 不自动后台索引（用户显式触发；自动 watch 是后续 watcher 阶段的事）。
- 不索引 sidechain 之外更深的元数据；不跨 harness（仅 claude，跟 B6 一致）。
- 不做分词器调优（FTS5 默认 unicode61；中文按字符可搜即可，v0.2 不承诺相关性排序）。

## Decision

索引落在主 SQLite（Rust Control Plane 所有，ADR-0004）但**独立成表**、
由设置门控；源转录永远只读。搜索走 SQL 聚合（ADR-0012 同款纪律：
查询路径不起进程、不读外部文件——只查索引）。

## Risks

- ~~FTS5 可用性未知~~ **已探明（2026-10-25，tests 探针实测后删除）**：
  bundled SQLite 带 FTS5；`tokenize='trigram'` 可用且中文/英文/混合子串均命中
  （'仓库里找到'/'所有 TODO' 命中、乱串 0），但 **<3 字符查询无 trigram 可匹配**——
  查询端对 <3 字符返回明确错误。默认 unicode61 对中文不可搜，不采用。
- 索引体积：只有 textParts/thinkingParts 入库，16 MiB 上限会传导。

## Verification

`cargo test --lib replay::` 全量 + 新迁移测试 + 契约 + `pnpm test` + 全量门。
