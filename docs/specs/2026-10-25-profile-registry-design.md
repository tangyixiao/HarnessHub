# ProfileAdapter + Profile Registry（ROADMAP E14）Spec

## Problem

ROADMAP E14 / 规划 §4.12-4.14：用户在不同 Provider / 账号 / 模型组合之间切换
要手动编辑各 Harness 的原生配置文件（`~/.claude/settings.json`、
`~/.codex/config.toml`、`~/.gemini/.env`）。Hub 需要统一 Profile 层：
**发现 → 校验 → （E15 才做投影）**。本阶段只做发现与只读展示，不写任何
外部配置（原子投影是 E15 的独立验收）。

## Locked Goal

Hub 能发现并展示每个 Harness 的「当前配置形态」：环境变量形式的 Provider
指向、模型偏好、base URL——只读解析现有配置文件，规范化为 `ProfileSummary`；
不写任何外部文件。用户在 Settings 页看到每个 harness 的配置现状。

## Acceptance Criteria

- [ ] `src-tauri/src/profile/` 新模块：
  - `mod.rs`：`ProfileSummary { harness_id, source_path, fields: BTreeMap<String,String>, parsed_at }`
    （camelCase DTO）；`ProfileParseError` 容错（文件不存在 → None；语法坏 → 错误）。
  - `claude.rs`：读 `~/.claude/settings.json` 的选定键（`ANTHROPIC_BASE_URL` /
    `ANTHROPIC_MODEL` / env 块内的键），**只读**。
  - `codex.rs`：读 `~/.codex/config.toml` 顶层 `model` / `model_provider` +
    `[model_providers.*]` 的 `base_url`（toml crate 已在依赖树），**只读**。
  - `gemini.rs`：读 `~/.gemini/.env` 的 `GEMINI_API_KEY` 是否存在（值脱敏为
    `set`/`unset`，**绝不把 key 值带出 Rust 层**）。
- [ ] `ProfileAdapter` trait：`fn discover(&self) -> Result<Option<ProfileSummary>>`（None = 配置文件不存在，不是错误）。
- [ ] IPC `profile_discover(harness_id) -> Option<ProfileSummary>`：纯读；未注册 harness → 错误。
- [ ] 契约测试（camelCase、脱敏字段、None 语义）+ 每个解析器各自的 fixture 测试（合法 / 缺文件 / 语法坏 / 键缺失）。
- [ ] 前端 Settings 页新增「Provider 配置现状」区（flash 子 agent）：harness 下拉 → 展示 fields（key=value 等宽），无配置显示空态。
- [ ] **安全红线**：任何 API key / token / secret 值不得跨 IPC——gemini 只报 `apiKey: "set" | "unset"`；claude/codex 的 env 块如有 `*KEY*` / `*TOKEN*` / `*SECRET*` 模式的键，值一律替换为 `<redacted>`。

## Non-goals

- **不写任何外部配置文件**（投影 + 原子写 + 备份恢复全部是 E15）。
- 不做多 Profile 切换 / 存储 / 比较（需要 E15 的投影基建先就位）。
- 不解析 MCP / Skills 配置（E18）。
- 不修改 harness capability（profiles 能力位在 E15 投影可用后翻 true）。

## Decision

只读发现先行：让用户先「看见」当前配置形态，再谈切换。解析器每个 harness
一个文件（同 harness adapter 模式），容错优先——外部配置文件是用户手编的，
坏语法必须给可读错误而不是崩溃。

## Risks

- 配置文件 schema 漂移 → 只读选定键，未知键忽略不报错；
- 泄密 → redact 规则在 DTO 序列化前执行，契约测试锁定。

## Verification

解析器 fixture 测试 + 契约 + 前端测试 + 全量门；真机只读 smoke。
