# Harness Hub — CONTEXT

> 本文件只记录**已经稳定的事实**：产品定义、技术栈、统一术语、核心约束、已锁定接口。
> 有争议、未验证、待定的内容不要写在这里，写进 `docs/specs/` 或对应 ADR 的 Open Questions。
> 规格源文件：`HARNESS_HUB_PROJECT_PLAN.md`（仓库根目录，v3 规划）。

## 1. 产品定义

Harness Hub = **Universal AI Development Control Plane**：

> AI Coding Harness 的统一 Launcher + Session Manager + Observability + History + Usage Dashboard + Git Activity Timeline。

核心目标不是再造一个 Harness，而是把现有 Harness 变成一个统一的工作环境，并让它们的
数据与生命周期真正关联起来：

```text
Project → Session → Harness → Profile → Identity → Provider → Model → Token / Cost / Latency
```

## 2. 技术栈（已锁定）

| 层                 | 技术                                                                                                               |
| ------------------ | ------------------------------------------------------------------------------------------------------------------ |
| Desktop shell      | Tauri 2                                                                                                            |
| Frontend           | React + TypeScript + Vite + Tailwind CSS + shadcn/ui 风格组件                                                      |
| Control Plane      | Rust（tokio、PTY 抽象、notify 文件监听、Git、SQLite）                                                              |
| 数据库             | SQLite（主索引库，Rust 侧持有）                                                                                    |
| Usage Engine       | ccusage（JSON 输出，MIT）                                                                                          |
| AI Runtime Sidecar | Python 3.10+ / uv / LiteLLM / 官方 SDK / MCP SDK / HF / tiktoken / tokenizers / transformers；LangChain 仅可选插件 |
| 首选平台           | Windows、Linux；macOS 后续正式支持                                                                                 |

## 3. 统一术语

- **Harness**：外部 AI coding CLI/Agent（Codex、Claude Code、Gemini CLI、OpenCode…）。
- **HarnessAdapter**：负责 detect / version / capabilities / launch / resume / stop / state。
- **UsageAdapter**：负责 discover / import / watch / usage / sessions / models。与 HarnessAdapter **分离**。
- **Capability**：**Adapter 是否实现了**这项能力。静态事实，只随代码版本变化。
  逐项、可独立演进的矩阵，不是单个 boolean。未实现必须为 `false`。
- **Readiness**：**此刻**这台机器 / 这个 Profile / 这个 Session 能否使用该能力
  （ready / blocked / unknown）。动态、随环境变化，**尚未实现**。
  两者不可合并，见 `docs/adr/0005-capability-vs-readiness.md`：
  binary 缺失、auth 过期只影响 readiness，不得回写 capability。
- **hub_session_id**：Harness Hub 自己的全局 Session 标识。
- **source_session_id**：外部 Harness 的原始 Session ID。二者不可混用。
- **runtime_target_id**：Session/运行必须关联的运行目标（v0.1 只有 `local`，接口预留远程）。
- **HHAR**：Harness Hub Activity Record，长期用户活动数据的可移植导出格式。
- **Projection**：把统一配置投影为某个 Harness 的原生配置文件（原子写）。

## 4. 核心约束

1. **Local-first**：Prompt / Response / Code / Session Log 默认不上传；索引与搜索在本机完成；
   网络能力必须显式、可关闭、可解释；估算值必须标记 `estimated`。
2. **Evidence over claims**：没有测试 / 构建 / 验收证据，不得宣称完成。
3. **不重复造轮子**：先整合成熟项目 → 复用兼容 License 模块 → 借鉴架构重写接口层 → 最后才自写。
4. **外部 Harness 数据只读**：Harness Hub 不修改外部 Harness 的日志与 Session 数据。
5. **配置投影必须原子写**：写坏原生配置属于最高等级事故。
6. **Secret 不进主 SQLite**：只存 `credential_refs`（句柄），真实密钥交给 OS 密钥链。
7. **Git diff ≠ AI 贡献**：UI 不得把 Session 时间窗口内的 diff 归因为 AI 产出。
8. **每个 Harness 运行都必须关联 `runtime_target_id`**，Core API 不假定绝对本机路径或本机进程。

## 5. 已锁定接口（v0.1）

Rust Control Plane 的模块边界与规格第 8 节仓库结构一致，见 `docs/CONTEXT-MAP.md`。

已实现并冻结的接口（改动需要 ADR）：

- `harness::HarnessCapabilities` —— 能力矩阵结构体。
- `harness::HarnessAdapter` —— Harness 生命周期 trait。
- `harness::HarnessRegistry` —— 适配器注册表。
- `usage::UsageSourceAdapter` —— Usage 数据源 trait（`detect` / `version` / `capabilities` / `import`），
  取代旧脚手架 `usage::UsageAdapter`。
- `usage::{UsageSource, UsageImport, UsageEvent}` —— 跨 IPC 的 Usage DTO（契约见 ADR-0011）。
- `session::SessionRecord` —— Session 持久化模型（含 `hub_session_id` / `source_session_id` / `runtime_target_id`）。
- `db::Database` —— SQLite 连接 + 迁移执行器。

Rust 与 Python 之间默认使用 **stdio JSON-RPC**，不为桌面端常驻开放 localhost HTTP 端口。

## 6. 当前阶段

- Phase 0（Research / Grill）：进行中。ADR-0001 ~ ADR-0011 已落。
- Phase 1（Walking Skeleton）：框架已就位（Tauri + React + SQLite migrations + 模块骨架），
  业务链路（Codex detect / PTY / ccusage 导入 / Dashboard 数字）尚未实现。
- 实施计划见 `docs/plans/`。
