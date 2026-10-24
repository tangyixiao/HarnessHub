# Harness Hub 开发路线图（v0.1 收尾 → v0.2+）

> 本文件是 **未来开发阶段的执行顺序表**，在 `HARNESS_HUB_PROJECT_PLAN.md` §9 Roadmap 的基础上，
> 按当前真实进度（见 `docs/CONTEXT.md` §6 与 `docs/releases/`）细化成 31 个可独立验收的阶段。
>
> 规则：
> 1. 每个阶段开 PR、按 AGENTS.md 质量门验收；一个阶段可以拆多个 PR。
> 2. 每个阶段的非平凡改动仍需先写 spec（`docs/specs/`）与 plan（`docs/plans/`）。
> 3. 里程碑（A5、B9、E19、F23、G27、H31）各发一个 release。
> 4. 顺序不是绝对串行：A/D/E/F/G/H 之间相互独立的阶段可用子 agent 并行。

## 现状基线（2026-10-24）

已完成：Phase 0 研究（ADR-0001~0013、0018）、Walking Skeleton、
Codex / Claude Code / Gemini CLI / OpenCode 的 detect+launch+terminal、
ccusage 导入（幂等+对账）、Dashboard 只读投影、Project Registry、
Activity Timeline、插件宿主 v0.1（+加固）、内部只读 `git::capture`、
v0.1.0-alpha.4 Windows 安装包。

尚未完成（CONTEXT.md 明确记录）：Gemini Usage、OpenCode Usage、
CodeBuddy 生命周期、Git Session Snapshot 的 DB/Session/IPC/UI 接线、
Linux 打包、v0.1 Release Gate 其余各项。

---

## Track A — v0.1 收尾（目标：v0.1.0 正式发布）

### 阶段 1（A1）Git Session Snapshot 接线
- 目标：把内部 `git::capture` 接入 Session 生命周期，满足 Release Gate 的
  "Git Session Snapshot"。
- 范围：migration 0010 `git_snapshots`（(session, phase) 唯一、只增不改）、
  启动成功（mark_running 之后）与终态写入时各做一次有界快照（best-effort，
  失败不得影响会话状态机）、IPC `session_git_snapshots`、Sessions 页展示
  （用词遵守 R5：**Changes during session**，不得声称 AI 贡献）。
- 验收：v9→v10 迁移测试（裸 Connection + apply_until）；真仓库集成测试
  start/end 两行快照；非 repo cwd 不落行不报错；IPC 契约测试（camelCase）；
  TS fixture 测试；冷启动真路径用例。
- 依赖：当前分支已有的 `git::capture`。

### 阶段 2（A2）Usage 覆盖补齐 + CodeBuddy 生命周期
- 目标：Gemini CLI / OpenCode 的 usage 进入 Dashboard；CodeBuddy 补
  launch/terminal 或明确能力全 false 不动。
- 范围：验证 ccusage 对 gemini/opencode 数据源的覆盖（fixture + 真机）、
  Dashboard harness 维度出现对应数据；CodeBuddy 按 capability 纪律决定。
- 验收：fixture 回归 + 真机导入记录；能力矩阵如实。

### 阶段 3（A3）History 深化：筛选 + 会话窗口 Git 摘要
- 目标：History 页可按时间 / Harness / Project 过滤；会话详情展示
  start→end HEAD 与 `+lines/-lines`、commit 数（`git log HEAD_start..HEAD_end`
  有界查询）。
- 验收：SQLite 聚合查询纯读（ADR-0012）；R5 用词纪律测试。

### 阶段 4（A4）Linux 打包 + CI 发布流水线
- 目标：Linux 可安装运行（v0.1 gate）；PR CI 稳定。
- 范围：AppImage/deb 构建、GitHub Actions artifact、E2E 冒烟矩阵。
- 验收：Linux 安装包 + 冷启动冒烟证据。

### 阶段 5（A5）v0.1.0 正式发布
- 目标：§21 Release Gate 全项核对通过并发布。
- 范围：E2E 三主流程（添加项目→启动→Session 出现；导入→Dashboard 正确；
  重启→历史仍在）、README 10 分钟上手、崩溃不损坏数据库验证。
- 验收：Release Gate 清单逐项贴证据。

## Track B — Session Observability（目标：v0.2.0）

### 阶段 6（B6）只读 Session Replay
- 按 harness capability 门控（replay=true 才显示），读取外部 session 日志
  只读渲染时间线；capability 不支持绝不伪造入口。

### 阶段 7（B7）消息全文搜索
- SQLite FTS5 索引 turns/messages；支持 exclude project / 关闭内容索引
  只保留 metadata（local-first 隐私开关）。

### 阶段 8（B8）Tool Call / Subagent 树
- tool_calls 表接线 + 展示；subagent tree（parent_session_id）。

### 阶段 9（B9）Deep Link 到 Harness 原生 session
- capability-aware 外链；**v0.2.0 release**。

## Track C — Worktree / 并行（目标：v0.3.0）

### 阶段 10（C10）Worktree 创建与 Session 绑定
- worktree_path 落库、创建流程（借鉴 CCManager）、状态展示。

### 阶段 11（C11）Merge/Delete flow + 并行 Agent Session
- 安全合并/清理；多 harness 并行编排视图；**v0.3.0 release**。

## Track D — 更多 Harness（eval 门控，ADR-0022）

### 阶段 12（D12）Kimi / Qwen / Copilot CLI 适配
- 每个 harness：detect→launch→terminal→usage 逐级加，fixture+contract test
  先行；"能检测到"不算支持。

### 阶段 13（D13）ccusage 已支持源的 usage-only 接入
- Amp / Goose / Droid / Kilo 等：usage 数据进 Dashboard，生命周期不动。

## Track E — Configuration / Profile Plane（目标：v0.4.0）

### 阶段 14（E14）ProfileAdapter + Profile Registry
- trait 分离（ADR-0011 草案）；导入现有 Claude/Codex/Gemini 配置并识别
  当前 profile；Profile 校验。

### 阶段 15（E15）安全配置投影
- read→parse→validate→backup→render→validate→atomic replace→verify read-back；
  ownership 字段记账；恢复官方配置入口。

### 阶段 16（E16）Credential Store + Identity Registry
- OS keychain 抽象；SQLite 只存 credential_refs；多账号 identity 模型；
  opaque envelope 快照。

### 阶段 17（E17）Config Doctor
- 只读诊断（config parseable / credentials reachable / projection drift），
  Fix 必须显式。

### 阶段 18（E18）MCP Binding Registry + Harness 同步
- Registry→projection diff→preview→atomic sync；双向导入需提示 source-of-truth。

### 阶段 19（E19）Skills/Prompts Registry + System Tray
- Canonical→harness renderer（CLAUDE.md/AGENTS.md/GEMINI.md）；托盘切换
  走同一 ProfileAdapter+audit；**v0.4.0 release**。

## Track F — Unified AI Runtime（目标：v0.5.0）

### 阶段 20（F20）Python Sidecar 生命周期
- 启停/握手/能力协商/崩溃恢复；无 Python 功能时主功能不受影响。

### 阶段 21（F21）Provider Gateway + Tokenizer
- LiteLLM 统一通道 + OpenAI/Anthropic/Google 原生逃生舱；token 来源四级
  可信度标记（provider_reported>count_api>local_exact>estimated）。

### 阶段 22（F22）MCP Client/Server
- stdio + streamable HTTP client；Hub 作为 MCP Server 先开放只读工具，
  高风险工具显式授权。

### 阶段 23（F23）HF 模型资产 + 本地 runtime（feature-gated）
- 显式下载、revision pinning、磁盘占用提示；torch/transformers 不进默认
  安装；**v0.5.0 release**。

## Track G — Trace / 安全 / 可移植（目标：v0.6.0）

### 阶段 24（G24）Canonical Event v1 + Trace Builder
- events/traces/spans 落库，OTel GenAI 语义对齐；Codex/Claude 事件映射。

### 阶段 25（G25）Permission Engine
- namespace + policy 优先级 + ask/allow/deny；插件权限从 observed 走向
  可配置 enforce；UI 权限时间线。

### 阶段 26（G26）HHAR 导出/导入
- `hhar/1` schema、redaction、round-trip、gzip 流式。

### 阶段 27（G27）Eval / 回归门
- golden fixtures 全 adapter 覆盖、contract tests、promptfoo 可选套件、
  release regression gate；**v0.6.0 release**。

## Track H — Runtime / Analytics（目标：v0.7.0）

### 阶段 28（H28）WSL RuntimeTarget
- path mapping（canonical project identity）、远程 harness 检测、PTY 生命周期。

### 阶段 29（H29）SSH / Container Runtime
- RuntimeAdapter：probe/spawn/exec/read_file/watch；断线恢复、trace 连续性。

### 阶段 30（H30）Analytics
- 热力图、harness/model/provider 趋势、月度/年度 AI Coding Review
  （趋势优先，不做能力评分）。

### 阶段 31（H31）OTLP Exporter + v0.7.0
- OTLP/Phoenix/Langfuse 可选导出；独立工作不受 backend 影响；**v0.7.0 release**。

---

## 明确不做（延续 §3.2 / §22）

云端账号、团队 SaaS、手机端、IDE、Agent Marketplace、自研反向代理、
绝对准确账单承诺。YAGNI 条目在进入对应阶段前仍需 Grill Gate 重新确认。
