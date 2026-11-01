# MCP Binding Registry + Harness 同步（ROADMAP E18）Spec

## Problem

规划 §4.20：**MCP Runtime（真正建立 MCP connection）与 MCP Binding（配置层决定
哪个 MCP Server 写进哪个 Harness 的配置）必须区分**。本阶段做后者：

```text
Registry（Hub SQLite，source-of-truth）
→ projection diff
→ preview
→ atomic sync
```

支持 bidirectional import（导入现有配置），但导入项标记 `imported:<harness>`，
且 UI 明确提示「导入后 Registry 是 source-of-truth」。

## Locked Goal

用户在 Hub 内登记 MCP Server（stdio / http），按 scope（global / harness）绑定，
对 claude / codex / gemini 各自执行「预览差异 → 确认 → 原子同步」；
同步走 E15 的 backup → temp → verify → rename 管线，坏结果绝不落盘。

## 目标文件与格式（真机取证）

| Harness | 目标文件                  | 形态                                                                  |
| ------- | ------------------------- | --------------------------------------------------------------------- |
| claude  | `~/.claude.json`          | 顶层 `mcpServers`：stdio `{command, args}`；http `{type:"http", url}` |
| codex   | `~/.codex/config.toml`    | `[mcp_servers.<name>]`：stdio `command`/`args`；http `url`            |
| gemini  | `~/.gemini/settings.json` | 顶层 `mcpServers`：stdio `{command, args}`；http `{httpUrl}`          |

（HTTP 形态逐源取证：gemini 用 `httpUrl`，codex 用 `url`，claude 用 `type:"http"+url`。）

- codex 用 `toml_edit` 做**保格式编辑**（其余部分逐字节保留，注释不丢）；
  claude/gemini 是 JSON（serde_json pretty，与 E15 同一取舍）。
- **Hub 只调和它管理的键**（command / args / url / type）：条目内其余键
  （`env` / `headers` / `timeout` 等用户手写的 secret 与调参）**原样保留**，
  导入时 env 只取**变量名**不取值。整条移除（不在 desired 里）在 preview
  显式列出、confirm 后才执行。
- 三个文件都可能是用户手编的重要文件：同步前自动备份、恢复可用。

## Acceptance Criteria

- [ ] migration 0013：`mcp_servers`（name UNIQUE；transport CHECK stdio|http；
      **env_keys 只存变量名**，值绝不入内）+ `mcp_bindings`
      （scope ∈ global|harness；`scope_key` 唯一约束防 NULL 重复；
      server 删除级联）。裸 Connection + `apply_until(12)` 升级用例。
- [ ] `src-tauri/src/mcp/registry.rs`：server upsert/list/remove + binding
      add/list/remove。校验：stdio 必须有 command；http 必须有 url；
      name 限 `[A-Za-z0-9._-]`；未知 harness 拒绝。
- [ ] `src-tauri/src/mcp/sync.rs`：
  - `desired(conn, harness)`：global ∪ harness 绑定中 enabled 的 server；
  - `preview`：**纯读**，返回 `McpSyncPreview { targetPath, additions, updates, removals }`
    （updates/removals 用摘要串，不含 env 值）；
  - `apply`：调和整段 mcpServers/`[mcp_servers]` → E15 管线落盘
    （目标文件不存在 = 明确报错「未初始化」）；
  - `import`：读现有条目，名字不在 Registry 的登记为 `imported:<harness>`，
    已存在的跳过。
- [ ] IPC：`mcp_server_list/upsert/remove`、`mcp_binding_list/add/remove`、
      `mcp_sync_preview/apply`、`mcp_import`（apply/import/写动作全部显式 confirm）。
- [ ] DTO 契约测试（camelCase）+ TS normalize/fixture 测试。
- [ ] Settings 页「MCP Servers」区：server 列表 + 新增表单 + 绑定勾选
      （global / 三个 harness）+ 每 harness 的 预览 → 确认 → 同步 / 导入。
- [ ] 同步语义：**整段调和**。不在 desired 里的现有条目出现在 removals 里，
      preview UI 逐条列出后用户确认才动手（这正是 import 存在的意义）。

## Non-goals

- 不做 project scope 绑定（规划模型里有，但投影目标 `<repo>/.mcp.json`
  属于用户仓库文件，安全语义单独评审后加）。
- 不做 profile scope（Profile 存储尚未实现）；不做 MCP Runtime 连接（F22）。
- 不做 opencode/codebuddy/copilot 的 MCP 投影（配置平面未建）。

## Decision

- `env_keys` 只是**元数据**（UI 展示 + Doctor 提示），同步不写 env 块——
  env 值是 secret，值进 harness 配置属于用户手工/凭据层（E16）的职责；
  Hub 绝不把 `<redacted>` 之类的占位写进真配置。
- 调和粒度 = 「条目级删除 + 管理键级更新」：不在 desired 的整条移除
  （preview 列出）；在 desired 的条目只覆盖 command/args/url/type，
  未知键一律保留。可预测、可预览、可恢复。
- toml_edit 保格式编辑 codex config.toml；JSON 侧 re-pretty（同 E15）。

## Verification

全矩阵单测 + migration 升级用例 + 契约 + 前端 fixture + 全量门。
