# 只读会话回放 B6（Claude Code 首个适配）Spec

## Problem

ROADMAP B6：Session Observability 的第一格——用户想回看某个外部 Harness 会话里
「说了什么、答了什么」，但 Harness Hub 目前只显示会话元数据（时间/状态/Git 快照），
不读会话正文。capability 矩阵里 replay 全 false。

## Locked Goal

Claude Code 的会话转录（`~/.claude/projects/**/*.jsonl`，只读）可以在
Harness Hub 内以只读时间线回放：列出可回放会话 → 点开看 user/assistant 消息流。
不写任何外部数据、不落库。

## Acceptance Criteria

- [ ] `src-tauri/src/replay/` 新模块：`claude::list_sessions(roots)`（stat + 有界
      预读 ≤ 256 KiB 取首条 user 消息预览与时间范围）与
      `claude::read_session(path)`（全文 ≤ 16 MiB，超限报错不猜）。
- [ ] 解析只认识已取证的行形状（type ∈ {user, assistant, …}；message.content
      字符串或数组；数组内 text/thinking 提取，tool_use 等保留为 unsupported 计数）；
      未知行类型计入 `skipped_kind_counts`，绝不 panic、绝不编造。
- [ ] `isSidechain=true` 的行带 sidechain 标记（为 B8 子代理树预留）。
- [ ] **IPC 不接受任意路径**：`replay_list_sessions(harness_id)` /
      `replay_read_session(harness_id, session_id)`——路径由后端从自己的发现结果
      解析，前端传不了文件系统路径（防任意文件读取）。
- [ ] 非 claude 的 harness：capability `replay=false` → 返回空列表（UI 不渲染入口）。
      claude 适配器 `replay` capability 翻 true（实现即能力）。
- [ ] fixture：`fixtures/claude/replay/`（synthetic，键名形状取自 2026-10-25 本机
      取证：type/parentUuid/isSidechain/message{role,content,model,usage}/uuid/
      timestamp/sessionId/cwd/version/gitBranch；内容全部合成）。
- [ ] Rust DTO camelCase 契约测试 + TS normalize fixture 测试 + UI 时间线渲染测试
      （user/assistant 气泡、thinking 折叠或标注、sidechain 徽标）。
- [ ] 冷启动/真机：对本机真实 `~/.claude/projects` 只读 smoke（不复制内容）。

## Non-goals

- 不写 turns/messages 表（那是 G24 Canonical Event 平面的决策点）。
- 不做 tool call 结果展开、subagent 树（B8）、全文搜索（B7）。
- 不解析 attachment / file-history-snapshot 内容（计入 skipped）。
- 不做 Codex/OpenCode replay（后续按同一 contract 逐个加）。

## Decision

回放「按需现读、不落库」：单会话文件有界（≤16 MiB），读一次渲染一次，
符合 §13 性能原则（不扫全量日志做索引）且避免 schema 过早冻结。
发现列表只 stat + 256 KiB 预读。

## Risks

- 上游格式漂移：未知类型/kv 一律 skipped 计数 + fixture 回归，不 panic。
- 大文件：硬上限 + 明确错误。

## Verification

`cargo test --lib replay::` + 契约 + `pnpm test` + 全量门 + 本机只读 smoke。
