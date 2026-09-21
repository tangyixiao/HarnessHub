# Harness Hub 项目规划

> 一个统一管理、启动、观察、检索和复盘 AI Coding Harness 的本地优先桌面控制中心。
>
> **核心目标不是再造一个 Harness，而是把现有 Harness 变成一个统一的工作环境。**
>
> 本规划将 **GrillMe 的“先拷问清楚再行动”** 与 **Superpowers 的“设计 → 计划 → TDD → 子 Agent 实施 → Review → 验证”** 组合成项目自己的研发协议。

---

## Product Positioning v3

Harness Hub 不再只是：

> Harness Manager + Usage Dashboard

而是：

> **Universal AI Development Control Plane**

统一管理：

```text
Harness
+ Session
+ Project
+ Provider
+ Profile
+ Identity
+ Credential
+ MCP
+ Skills
+ Prompts
+ Model
+ Token
+ Cost
+ Proxy / Routing
+ Git / Worktree
+ AI Coding History
```

其中：

```text
CCManager-like        → Runtime Plane
ccusage-like          → Usage Plane
CC Switch-like        → Configuration Plane
ccswitch-like         → Identity/Profile Plane
LiteLLM               → Provider Gateway
Official SDKs         → Native Provider Plane
MCP SDK               → Tool Protocol Plane
HF / Transformers     → Model Asset / Local Runtime
Agent Trail-like      → History / Replay Plane
Harness Hub           → 把这些能力统一成一个产品
```

**我们的创新重点不是复制上述工具，而是让它们的数据和生命周期真正关联起来。**

例如：

```text
Project
  → Session
    → Harness
      → Profile
        → Identity
          → Provider
            → Model
              → Token / Cost / Latency
```

这是单独的 CC Switch、ccusage 或 CCManager 都难以完整回答的关系。


## 0. 项目一句话定义

**Harness Hub = AI Coding Harness 的统一 Launcher + Session Manager + Observability + History + Usage Dashboard + Git Activity Timeline。**

它面向同时使用 Codex、Claude Code、Gemini CLI、OpenCode、Kimi、Qwen、Copilot CLI、Cline、Cursor Agent 等工具的人，解决以下问题：

- Harness 太多，入口分散。
- Session、项目、历史记录分散。
- Token / Cost / Model Usage 难以统一统计。
- 不同 Harness 的能力不同，无法统一判断“能不能恢复、能不能回放、能不能获取 Token”。
- AI Coding 过程没有长期历史视图，很难复盘“我在哪个项目、什么时候、用什么 Agent 做了什么”。
- 多 Agent + Git Worktree 并行开发缺少统一控制中心。

最终它应该更像：

> **GitHub Contributions + Steam 年度回顾 + ActivityWatch + Agent Trail + CCManager + ccusage 的统一桌面入口。**

---

# 1. 项目原则

## 1.1 不重复造轮子

优先级：

1. **直接调用成熟项目**。
2. **复用有兼容 License 的模块**。
3. **借鉴成熟架构重新实现接口层**。
4. 最后才自己写 parser / PTY / usage engine。

任何准备自己实现的功能，在写代码前必须先回答：

> “这个问题是否已经被成熟项目解决？”

如果答案是“基本解决”，默认整合，不重写。

## 1.2 Harness 能力不是 Boolean

不能只有：

```text
supports_codex = true
```

而要描述 capability：

```text
Codex
✓ detect
✓ launch
✓ terminal
✓ resume
✓ usage
✓ model breakdown
✓ session replay
✓ tool calls
✓ git integration
? live state
```

每个 Harness 的能力独立演进。

## 1.3 Harness Adapter 与 Usage Adapter 分离

“能启动”与“能可靠解析 Usage”是两件不同的事。

```text
HarnessAdapter
├── detect()
├── version()
├── capabilities()
├── launch()
├── resume()
├── stop()
└── state()

UsageAdapter
├── discover()
├── import()
├── watch()
├── usage()
├── sessions()
└── models()
```

这样一个 Harness 即使没有可靠 Token，也仍然可以被 Harness Hub 管理。

## 1.4 Local-first

默认规则：

- 原始 Prompt / Response / Code / Session Log **不上传**。
- 默认所有统计、索引、搜索在本机完成。
- SQLite 本地存储。
- 网络功能必须显式、可关闭、可解释。
- Usage 只做估算时必须标记 `estimated`。

## 1.5 Evidence over claims

来自 Superpowers 的原则：

> 不能因为 Agent 说“完成了”，就认为完成了。

所有任务完成必须有证据：

- Test 通过。
- Build 通过。
- Lint / Typecheck 通过。
- 手工验收项通过。
- 必要时保存截图、日志或 fixture。

---

# 2. 开发方法：GrillMe × Superpowers

项目不允许从“有个想法”直接跳到“写代码”。

统一流程：

```text
Idea
  ↓
Grill Gate
  ↓
Locked Spec / ADR
  ↓
Superpowers Brainstorming
  ↓
Approved Design
  ↓
Git Worktree
  ↓
Implementation Plan
  ↓
TDD / Subagent Development
  ↓
Spec Review
  ↓
Code Quality Review
  ↓
Verification
  ↓
Merge / Release
```

---

## 2.1 Grill Gate：动手前必须先锁定问题

每个非平凡 Feature 先进行 GrillMe 风格拷问。

只问**高影响问题**，能从代码、文档、现有决策中确认的内容不得重复问人。

### 必须检查五个维度

#### Goals

- 用户真正要解决什么？
- 为什么现有工具不够？
- 成功后用户行为发生什么变化？

#### Acceptance

- 怎么证明完成？
- 哪些输入必须工作？
- 哪些失败模式必须被正确处理？

#### Boundaries

- 本 Feature 不做什么？
- 是否允许修改外部 Harness 数据？
- 是否需要联网？
- 支持哪些 OS？

#### Alternatives

- 是否已有成熟项目？
- 能不能 adapter / wrapper / subprocess 解决？
- 能不能延迟到以后？

#### Assumptions

- 本地日志格式是否稳定？
- Harness 是否允许 resume？
- Token 是否是准确值还是估算？
- 当前判断是否依赖未验证假设？

### Grill Gate 输出

每个 Feature 最终至少生成：

```text
Locked Goal
Acceptance Criteria
Non-goals
Key Assumptions
Chosen Approach
Rejected Alternatives
Risks
Open Questions
```

高成本 / 难逆转决策进入 ADR。

---

## 2.2 活文档

建议维护：

```text
docs/
├── CONTEXT.md
├── CONTEXT-MAP.md
├── adr/
│   ├── 0001-tauri-desktop.md
│   ├── 0002-adapter-separation.md
│   ├── 0003-local-first.md
│   └── ...
├── specs/
└── plans/
```

### CONTEXT.md

只放已经稳定的事实：

- 产品定义。
- 技术栈。
- 统一术语。
- 核心约束。
- 已锁定接口。

### CONTEXT-MAP.md

告诉 Agent 去哪里找东西：

```text
Harness adapter → src-tauri/src/harness/
Usage ingestion → src-tauri/src/usage/
Session index → src-tauri/src/session/
DB schema → src-tauri/src/db/
Frontend dashboard → src/features/dashboard/
```

防止每次 Agent 都重新探索整个仓库。

### ADR

只记录重要决策：

```text
Status
Context
Decision
Alternatives
Consequences
Evidence
Revisit Conditions
```

---

## 2.3 Superpowers 实施协议

设计锁定后按 Superpowers 方法执行：

1. `brainstorming`
2. `using-git-worktrees`
3. `writing-plans`
4. `subagent-driven-development` 或 `executing-plans`
5. `test-driven-development`
6. `requesting-code-review`
7. `verification-before-completion`
8. `finishing-a-development-branch`

### 每个实现 Task 必须足够小

一个 Task 应该形成独立可测试成果，而不是：

> “实现整个 Codex 支持。”

而应该类似：

```text
Task 1: detect Codex binary
Task 2: read Codex version
Task 3: define Codex capabilities
Task 4: launch Codex in PTY
Task 5: persist launch metadata
Task 6: restore terminal entry in UI
```

任务内遵循：

```text
RED
↓
确认 Test 确实失败
↓
GREEN
↓
确认 Test 通过
↓
REFACTOR
↓
再次验证
↓
Commit
```

---

# 3. v0.1 产品范围

## 3.1 必做

### Harness Discovery

自动扫描本机：

- Claude Code
- Codex CLI
- Gemini CLI
- OpenCode
- Kimi CLI
- Qwen
- Copilot CLI
- Cline CLI
- Cursor Agent

并展示：

```text
Name
Installed
Binary Path
Version
Capabilities
Data Paths
```

### Unified Launcher

选择：

```text
Project → Harness → Start
```

在 Harness Hub 内直接打开 Terminal。

### Project Registry

支持：

- 添加本地 Git repository。
- 自动识别 repo root。
- 最近项目。
- 项目级 Harness 设置。
- 项目级 session 历史。

### PTY Session Manager

至少支持：

- Launch。
- Kill。
- Reconnect 到仍存活的进程。
- 多 Session。
- Session 状态持久化。

### Usage Dashboard

最少提供：

- Input Token。
- Output Token。
- Cache Token（来源支持时）。
- Total Token。
- Estimated Cost。
- Model Breakdown。
- Harness Breakdown。
- Project Breakdown。
- Day / Week / Month。

优先把 **ccusage 作为 usage engine**，不要为每个 Agent 重写 Token parser。

### Coding Activity History

这是项目差异化功能。

按：

```text
Date
Project
Harness
Model
Session
Token
Duration
Git Diff
Commit
```

构成长期 Timeline。

### Git Activity

v0.1 记录：

- Session 开始时 HEAD。
- Session 结束时 HEAD。
- `+lines / -lines`。
- Commit 数。
- Worktree 信息。

注意：

> Git diff 只能表示 Session 时间窗口内发生的代码变化，不能天然证明“这些代码就是 AI 写的”。UI 必须避免错误归因。

---

## 3.2 v0.1 明确不做

YAGNI：

- 不训练、托管或自研商业 LLM Provider。
- 不自己发明一套 Provider 协议；统一调用优先走 LiteLLM，原生能力走官方 SDK。
- Python AI Runtime 在架构上预留，但不要求 v0.1 把所有 Provider / 本地模型能力一次做完。
- v0.1 不实现完整远程运行时，只实现 LocalRuntime，但接口必须预留 RuntimeTarget。
- v0.1 不实现完整强制沙箱，只定义 Permission Schema 并记录 observed permission events。
- v0.1 不发布插件市场，只冻结最小 Plugin API 草案与一个示例插件。
- v0.1 不追求完整 OTel backend，只保证 Canonical Trace 可转换。
- v0.1 不做复杂年度评分，只保存未来可分析的数据。
- 不自己做 IDE。
- 不替代 VS Code / JetBrains。
- 不做云端账号系统。
- 不上传用户完整 Session。
- 不做团队协作 SaaS。
- 不做手机端。
- 不做 Agent Marketplace。
- 不试图统一所有 Harness 的 Prompt DSL。
- 不要求每个 Harness 都具有完全相同能力。
- 不对 Usage 做“绝对准确账单”承诺。

---

# 4. 技术架构

## 4.1 推荐技术栈

```text
Desktop
└── Tauri 2

Frontend
├── React
├── TypeScript
├── Vite
├── Tailwind CSS
└── shadcn/ui

Control Plane
├── Rust
├── tokio
├── portable-pty / 等价 PTY 抽象
├── notify / 文件监听
├── Git
└── SQLite

Usage
└── ccusage

AI Runtime Sidecar
├── Python 3.10+
├── uv
├── LiteLLM
├── OpenAI Python SDK
├── Anthropic Python SDK
├── Google Gen AI SDK
├── MCP Python SDK
├── huggingface_hub
├── tiktoken
├── tokenizers
├── transformers
└── LangChain（可选插件层，不进入核心依赖路径）
```

首要平台：

```text
Windows
Linux
```

macOS 作为后续正式支持目标。

---

## 4.2 总体结构

```text
┌─────────────────────────────────────────────────────────────┐
│                         React UI                            │
│ Dashboard / Projects / Sessions / History / Models / MCP   │
│ Terminal / Settings / Harnesses / Providers                │
└────────────────────────────┬────────────────────────────────┘
                             │ Tauri IPC
┌────────────────────────────▼────────────────────────────────┐
│                    Rust Control Plane                       │
├─────────────────────────────────────────────────────────────┤
│ Harness Registry / Process / PTY / Git / Watcher / Search  │
│ Session Index / Capability Resolver / SQLite / Audit        │
└───────────────┬────────────────────────────┬────────────────┘
                │                            │ stdio JSON-RPC
                │                            │（默认不开放本机端口）
                │             ┌──────────────▼────────────────┐
                │             │      Python AI Runtime        │
                │             ├───────────────────────────────┤
                │             │ Provider Gateway: LiteLLM     │
                │             │ Native Provider Adapters      │
                │             │ MCP Client / Server           │
                │             │ Tokenizer Service             │
                │             │ HF Model Registry / Cache     │
                │             │ Optional Orchestration        │
                │             └──────────────┬────────────────┘
                │                            │
┌───────────────▼──────────────┐   ┌────────▼─────────────────┐
│            SQLite           │   │ Models / APIs / MCP      │
└──────────────────────────────┘   │ OpenAI / Anthropic      │
                │                  │ Google / HF / Local     │
┌───────────────▼──────────────┐   └──────────────────────────┘
│       External Harnesses     │
│ Codex / Claude / Gemini /... │
└──────────────────────────────┘
```

### Rust 与 Python 的边界

Rust 是 **Control Plane**：

- 进程生命周期。
- PTY。
- Git / Worktree。
- 文件监听。
- SQLite 主数据库。
- 本机权限。
- Harness 检测。
- Sidecar 生命周期。
- Secret 句柄与审计。

Python 是 **AI Runtime / Integration Plane**：

- 调用模型 API。
- Provider 统一与原生能力。
- MCP。
- Tokenizer。
- Hugging Face 模型与缓存。
- 可选 Agent 编排。

两者默认使用 **stdio JSON-RPC** 通信，避免为桌面端无意义地常驻开放一个 localhost HTTP 端口。

---

# 4.3 AI Runtime 分层

必须先建立统一术语：

```text
Provider
= OpenAI / Anthropic / Google / Hugging Face / OpenAI-compatible endpoint

Harness
= Codex / Claude Code / Gemini CLI / OpenCode / Cline / ...

Gateway
= LiteLLM

Native SDK
= openai / anthropic / google-genai

Protocol
= MCP

Tokenizer
= tiktoken / tokenizers / Provider-reported usage

Model Registry / Artifact Layer
= huggingface_hub

Local Model Runtime
= transformers

Orchestrator
= LangChain（optional）
```

**禁止把上述对象全部抽象成一个 `Agent`。**

---

## 4.4 LiteLLM：统一 Provider Gateway

LiteLLM 负责“公共能力的统一入口”：

```text
completion / response-like call
stream
model routing
fallback
cost metadata
rate-limit / gateway policy
provider normalization
```

内部定义自己的稳定接口，例如：

```python
class ProviderGateway(Protocol):
    async def generate(self, request: GenerateRequest) -> GenerateResult: ...
    async def stream(self, request: GenerateRequest) -> AsyncIterator[ModelEvent]: ...
    async def models(self) -> list[ModelInfo]: ...
```

### 原则

> **LiteLLM 统一公共能力；官方 SDK 保留原生能力逃生舱。**

不要为了“统一”而丢失 Provider 特有功能。

---

## 4.5 官方 SDK：Native Provider Adapters

### OpenAI Python SDK

用途：

- OpenAI 原生 API。
- Responses API。
- 原生 Tool / MCP / streaming / structured output 等能力。
- LiteLLM 尚未完整暴露的新功能。

适配器：

```text
OpenAIProviderAdapter
├── generate
├── stream
├── count_input_tokens（API 支持时）
├── tools
├── mcp
└── native_capabilities
```

### Anthropic Python SDK

用途：

- Messages。
- Streaming。
- Tool use / Tool runner。
- Anthropic 原生 MCP helpers。
- Anthropic 新 Beta / Agent 能力的高保真接入。

### Google Gen AI SDK

用途：

- Gemini Developer API。
- Google Gen AI 原生功能。
- Live / Function Calling / Google 特有 API。
- LiteLLM 不适合表达的 provider-specific capability。

### 双通道策略

```text
Unified Mode
└── LiteLLM

Native Mode
├── OpenAI SDK
├── Anthropic SDK
└── Google Gen AI SDK
```

UI 明确展示当前调用模式，避免调试时“不知道请求经过了几层 wrapper”。

---

## 4.6 MCP Python SDK：统一工具协议层

MCP SDK 同时承担两个方向。

### Harness Hub 作为 MCP Client

连接：

- stdio MCP server。
- Streamable HTTP MCP server。
- 其他标准 transport。

用于：

- 列出 Tools。
- Resources。
- Prompts。
- 调用 Tool。
- 给内置 Model Session 暴露用户选择的 MCP 能力。

### Harness Hub 自己作为 MCP Server

未来可以暴露经过权限控制的工具：

```text
harness.list
harness.capabilities
project.list
session.list
session.open
usage.query
model.list
```

高风险动作例如：

```text
harness.launch
session.kill
git.worktree.create
```

必须有显式授权 / policy，不允许第三方 MCP Client 静默执行。

### 原则

MCP 是协议边界，不是内部所有模块的 RPC 总线。

---

## 4.7 Tokenization Layer

Token 统计来源必须有可信度等级。

优先级：

```text
1. Provider-reported usage
2. Provider 官方 token-count API
3. 与模型精确匹配的本地 tokenizer
4. 估算
```

数据库保存：

```text
value
source
accuracy
tokenizer_id
model_id
estimated
```

### tiktoken

主要用于：

- OpenAI 模型对应 tokenizer。
- OpenAI 风格 BPE 的精确/近精确本地计数。

**不得拿 tiktoken 强行估 Anthropic / Gemini Token。**

### Hugging Face tokenizers

用于：

- Hugging Face / 开源模型 tokenizer。
- 加载 `tokenizer.json`。
- 通用快速 Encode / Decode。
- 需要时训练或验证自定义 tokenizer。

只有当 tokenizer 与目标 model revision 明确匹配时才能标记为 exact。

---

## 4.8 huggingface_hub

`huggingface_hub` 负责 **模型资产管理，不负责成为 Harness Hub 的主推理抽象**。

用途：

- Model / tokenizer repository metadata。
- 下载。
- Revision pinning。
- 本地 cache。
- 登录 / token。
- 文件校验。
- 可选 Hugging Face API / Inference 集成。

建议建立：

```text
ModelAsset
├── repo_id
├── revision
├── local_path
├── tokenizer_path
├── size
├── source
└── license_metadata
```

下载必须用户显式触发，并显示预计磁盘占用。

---

## 4.9 transformers

这里指 Hugging Face `transformers`。

用途：

- 本地开源模型加载。
- `AutoTokenizer`。
- `AutoModel*`。
- Pipeline。
- 模型兼容性验证。
- 小模型 / 实验性本地推理。

### 不做错误承诺

`transformers` **不是桌面端所有本地大模型的万能生产推理引擎**。

后续如果需要高性能本地推理，应允许额外 Adapter：

```text
llama.cpp
vLLM
MLX
ONNX Runtime
Ollama
其他 runtime
```

因此内部接口应叫：

```text
LocalModelRuntime
```

而不是：

```text
TransformersRuntimeForever
```

---

## 4.10 LangChain

LangChain 不进入 Harness Hub 的核心 Provider 调用路径。

使用场景限定为：

- 用户自定义 chain / workflow。
- RAG 原型。
- Tool composition。
- 可选 Agent / multi-agent workflow。
- 插件式实验功能。

不用于：

- Harness detection。
- Usage ingestion。
- PTY。
- SQLite 核心模型。
- Provider 的唯一抽象。
- MCP 的唯一抽象。

原因：

> Harness Hub 自己已经是“统一 Harness 平台”，如果再把核心架构建立在另一个 Agent abstraction 上，会产生双重抽象和调试困难。

建议：

```text
core
└── 不依赖 langchain

plugins/langchain
└── 可选 extras
```

---

## 4.11 Python 依赖分组

不要默认安装一整个 AI Python 宇宙。

建议 `pyproject.toml`：

```text
core
├── pydantic
├── anyio
└── transport / rpc deps

providers
├── litellm
├── openai
├── anthropic
└── google-genai

mcp
└── mcp

hf
├── huggingface_hub
└── tokenizers

local
├── transformers
├── torch（按平台单独处理）
└── accelerate（需要时）

orchestration
└── langchain
```

发行包按 Capability 选择安装，避免普通用户为了看 Usage 被迫下载数 GB 的 Torch。

---


---

# 4.12 Configuration / Profile Control Plane

Harness Hub 不能只统一“启动 Harness”和“调用模型”，还需要统一第三块核心能力：

> **Harness 配置、Provider Profile、账号身份、MCP、Skills、Prompts 与代理切换。**

这一层主要参考 / 复用 **CC Switch / ccswitch / CCS** 一类成熟工具的思路。

必须明确区分：

```text
Harness
= Codex / Claude Code / Gemini CLI / OpenCode / ...

Provider
= OpenAI / Anthropic / Google / OpenRouter / 自定义兼容端点

Profile
= 某个 Harness 可切换的一组 Provider + Model + Auth + Env + Proxy 配置

Identity
= 官方 OAuth / API Key / SSO 等登录身份

Credential
= Key / OAuth credential envelope / keyring entry

Config Projection
= 将统一 Profile 安全投影到各 Harness 原生配置文件

Skill
= Harness 可加载的 Skill / Prompt / Extension

MCP Binding
= 某个 MCP Server 在哪些 Harness / Profile / Project 中启用
```

不要把它们全部塞进一个 `ProviderConfig`。

---

## 4.13 CC Switch 类项目的定位

CC Switch 类工具成熟解决了这些痛点：

```text
Provider profiles
One-click switch
Config file projection
MCP sync
Skills sync
Prompt sync
Preset registry
System tray switching
Import / export
Backup / restore
Health / latency test
Proxy / failover
Cloud sync
```

Harness Hub 不应重新从零造一套配置编辑器。

### Harness Hub 中对应模块

```text
Config Plane
├── Profile Registry
├── Identity Registry
├── Credential Store
├── Provider Presets
├── Config Projector
├── MCP Binding Registry
├── Skill Registry
├── Prompt Registry
├── Proxy Profile
├── Backup / Restore
└── Config Doctor
```

---

## 4.14 Profile Adapter

每个 Harness 增加第二套能力接口：

```rust
trait ProfileAdapter {
    fn discover_profiles(&self) -> Result<Vec<Profile>>;
    fn current_profile(&self) -> Result<Option<ProfileId>>;
    fn validate(&self, profile: &Profile) -> Result<ValidationReport>;
    fn activate(&self, profile: &Profile) -> Result<ActivationResult>;
    fn backup(&self) -> Result<ConfigSnapshot>;
    fn restore(&self, snapshot: &ConfigSnapshot) -> Result<()>;
}
```

与原有 Harness Adapter 分离：

```text
HarnessAdapter
├── detect
├── version
├── launch
├── resume
└── capabilities

ProfileAdapter
├── discover
├── current
├── validate
├── activate
├── backup
└── restore
```

理由：

- “能启动 Harness”不代表“能安全切换 Provider”。
- “能切换 Provider”不代表“能切换官方 OAuth 身份”。
- “能改配置”不代表“运行中的 Session 会热加载”。
- 不同 Harness 的生效策略不同。

---

## 4.15 Config Projection：统一配置 → 原生配置

Harness Hub 内部保存**规范化 Profile**，实际运行时投影到原生配置。

示例：

```text
Unified Profile
│
├── Claude Code
│   └── ~/.claude/settings.json
│
├── Codex
│   ├── ~/.codex/config.toml
│   └── ~/.codex/auth.json / OS keyring
│
├── Gemini CLI
│   ├── ~/.gemini/.env
│   └── ~/.gemini/settings.json
│
└── OpenCode
    └── native config
```

### 投影原则

任何写入必须：

```text
read
→ parse
→ validate
→ create backup
→ render candidate
→ schema / syntax validate
→ atomic replace
→ verify read-back
```

禁止：

```text
直接字符串 replace 配置文件
```

### Config Ownership

必须记录每个字段是谁管理的：

```text
user
harness-hub
external-tool
unknown
```

Harness Hub 默认不覆盖自己没有 ownership 的用户字段。

---

## 4.16 原子写与灾难恢复

参考 CC Switch 的“minimal intrusion”思路：

> 即使 Harness Hub 崩溃或被卸载，外部 Harness 也应该继续可用。

实现要求：

- 原子写。
- 修改前快照。
- 至少保留最近 N 个版本。
- 恢复到 Official Login / 原生配置的明确入口。
- Crash 中断不能留下半截 JSON / TOML。
- 所有 destructive migration 可回滚。
- Config Doctor 必须只读优先。

数据库增加：

```text
config_snapshots
config_projections
config_mutations
config_ownership
```

---

## 4.17 Identity / Credential Manager

账号切换必须和普通 Provider Profile 区分。

参考多账号 `ccswitch` 的模型：

```text
Identity
├── provider
├── account_hint
├── organization_hint
├── auth_kind
├── credential_ref
├── last_used
└── quota_metadata
```

### Credential 不直接存 SQLite

数据库只保存：

```text
credential_ref
```

真实 Secret 优先进入：

```text
Windows Credential Manager
macOS Keychain
Linux Secret Service / keyring
```

只有上游 Harness 本身必须使用文件时，才写入对应原生凭据文件。

### Credential Envelope

对于不理解内部结构也能安全快照 / 恢复的凭据：

> 将凭据视为 opaque envelope，而不是自行拆 token、刷新 token、重新拼 OAuth 数据。

这样上游 schema 变化时风险更低。

---

## 4.18 Multi-Account / Isolated Runtime

需要支持两种账号模型。

### Global Switch

```text
Claude Code
current identity = personal
```

适合简单使用。

### Isolated Session

```text
Terminal A
Claude Code → personal

Terminal B
Claude Code → school / team

Terminal C
Claude Code → API profile
```

Harness Hub 可以通过：

- 独立 config root。
- 独立环境变量。
- 独立 HOME / XDG projection（必要时）。
- OS Secret Store 的受控映射。
- Harness 官方支持的 profile mechanism。

来实现并发隔离。

**禁止通过共享文件频繁覆盖来伪造并发隔离。**

---

## 4.19 Provider Preset Registry

参考 CC Switch 的 preset 思路，但 Registry 独立于 UI。

```text
ProviderPreset
├── id
├── display_name
├── protocol
├── base_url
├── auth_schema
├── model_mapping
├── supported_harnesses
├── health_check
├── source
└── version
```

Preset 来源：

```text
builtin
community
user
organization
```

### 安全约束

Community preset：

- 默认不能包含 secret。
- 显示实际 endpoint。
- 安装前展示将修改哪些配置。
- 可签名 / hash。
- 支持禁用。
- 不自动执行任意脚本。

---

## 4.20 MCP Registry 与 Harness Sync

前面的 MCP Python SDK 负责协议连接。

这里的 Config Plane 负责：

> **哪个 MCP Server 应该写进哪个 Harness 的配置。**

两者必须区分。

```text
MCP Runtime
= 真正建立 MCP connection

MCP Binding
= 配置层决定 Claude / Codex / Gemini 是否启用该 MCP
```

模型：

```text
McpServer
└── Bindings
    ├── global
    ├── harness:claude
    ├── harness:codex
    ├── project:OJ-NEXUS
    └── profile:work
```

同步采用：

```text
Registry
→ projection diff
→ preview
→ atomic sync
```

支持 bidirectional import，但内部 Registry 是 Harness Hub 的 source-of-truth 时必须明确提示。

---

## 4.21 Skills / Prompts Registry

CC Switch 已证明跨 Harness 管理 Skills / Prompts 有实际需求。

Harness Hub 统一模型：

```text
Skill
├── id
├── source
├── version
├── files
├── compatibility
└── bindings

PromptArtifact
├── role
├── content
├── project_scope
└── harness_projection
```

可能投影：

```text
CLAUDE.md
AGENTS.md
GEMINI.md
Harness-specific skill directories
```

### 关键约束

不要暴力把同一段 Prompt 复制到所有 Harness。

需要：

```text
Canonical Content
        ↓
Harness-specific Renderer
        ↓
CLAUDE.md / AGENTS.md / GEMINI.md
```

这样可以处理不同 Harness 的语义与格式差异。

---

## 4.22 Proxy / Routing Plane

CC Switch / CCS 类工具还覆盖：

- local proxy。
- hot switch。
- provider failover。
- health monitoring。
- format conversion。

Harness Hub 中这一层与 LiteLLM 的关系：

```text
                 ┌─ Harness Native Config → Provider
Harness Hub ─────┤
                 └─ Harness → Local Gateway → LiteLLM → Provider
```

### 两种模式

**Direct Mode**

```text
Codex → OpenAI
Claude → Anthropic
```

**Gateway Mode**

```text
Codex / Claude / Gemini
        ↓
Harness Hub Local Gateway
        ↓
LiteLLM
        ↓
Provider Pool
```

Gateway Mode 可提供：

- failover。
- health check。
- centralized usage。
- cost attribution。
- model routing。
- request audit。
- rate limit。

但 **v0.1 不自研完整反向代理**；优先复用 LiteLLM Proxy / 已有成熟组件。

---

## 4.23 Usage + Profile + Identity 联动

这是 Harness Hub 能超过单独 CC Switch / ccusage 的地方。

Usage Event 不只记录：

```text
harness
model
tokens
cost
```

还应该尽可能关联：

```text
project
session
harness
profile
identity
provider
endpoint
model
gateway_mode
token_source
cost
```

这样 Dashboard 能回答：

```text
这个月 Codex 官方 OAuth 用了多少？
这个项目在 OpenRouter 花了多少？
Claude personal 与 team 各用了多少？
哪个 Provider 延迟最低但失败率最高？
切换到某个 Profile 后成本发生了什么变化？
```

---

## 4.24 Config / Account Capability Matrix

Capability 不再只有 Harness Runtime。

增加：

```text
Runtime
├── launch
├── resume
└── terminal

Usage
├── tokens
├── cost
└── replay

Config
├── provider_switch
├── model_switch
├── mcp_sync
├── skill_sync
└── prompt_sync

Identity
├── official_oauth
├── api_key
├── multi_account
├── isolated_session
└── quota

Routing
├── proxy
├── failover
├── health_check
└── hot_switch
```

UI 必须根据 Capability 显示真实支持情况。

---

## 4.25 System Tray

Provider / Profile switching 很适合 Tray。

托盘菜单建议：

```text
Harness Hub
├── Codex
│   ├── ● Official
│   ├── OpenRouter
│   └── Local
├── Claude
│   ├── ● Personal
│   └── Team
├── Gemini
│   └── Google Official
├── Active Sessions
├── Pause Gateway
└── Open Dashboard
```

但切换动作仍必须经过同一套 `ProfileAdapter` 与 audit log，Tray 不允许绕过安全层。

---

## 4.26 Config Doctor

参考多账号 ccswitch 的 `doctor` 思路。

提供只读诊断：

```text
✓ Codex config parseable
✓ Claude credentials accessible
✓ Gemini env valid
⚠ Profile points to unreachable endpoint
⚠ MCP server executable missing
⚠ Current config differs from Harness Hub projection
✓ Backup available
```

默认只给修复建议。

只有用户显式执行 `Fix` 才修改配置。



---

# 4.27 Unified Trace / Event Plane

Harness Hub 的长期核心不应只是 `session + usage`，而应是：

> **一次 AI 开发行为的完整可追踪事件链。**

统一采用：

```text
Trace
└── Span
    ├── harness
    ├── agent
    ├── llm
    ├── tool
    ├── mcp
    ├── shell
    ├── filesystem
    ├── git
    ├── subagent
    └── runtime
```

内部 Trace 模型尽量对齐：

- OpenTelemetry GenAI Semantic Conventions。
- OpenInference 的 LLM / Agent / Tool span 语义。

### 目标

一次 Session 可以回答：

```text
用户说了什么？
哪个 Harness 接收了任务？
调用了哪个 Provider / Model？
用了多少 Token？
调用了什么 Tool？
执行了哪些 Shell？
读写了哪些文件？
启动了几个 Subagent？
产生了什么 Git diff / commit？
哪些操作失败？
总耗时在哪里？
```

### Canonical Event

统一事件必须最少包含：

```text
event_id
trace_id
span_id
parent_span_id
timestamp
duration
event_type
project_id
session_id
harness_id
runtime_target_id
profile_id
identity_id
provider_id
model_id
attributes
status
source
schema_version
```

### Event Types

第一版建议：

```text
session.started
session.ended

message.user
message.assistant

model.request
model.response
model.error

tool.call
tool.result
tool.error

mcp.call
mcp.result

shell.started
shell.ended

file.read
file.write
file.delete

git.diff
git.commit
git.checkout

subagent.started
subagent.ended

profile.activated

permission.requested
permission.allowed
permission.denied
```

### 存储

```text
Raw source
    ↓
Adapter normalization
    ↓
Canonical Event
    ↓
Trace Builder
    ↓
SQLite
    ├── Dashboard
    ├── Replay
    ├── Cost Analysis
    └── OTLP Exporter
```

### Export

后续允许输出到：

```text
OTLP
OpenTelemetry Collector
Phoenix
Langfuse
Grafana / Tempo
Jaeger
其他 OpenTelemetry-compatible backend
```

原则：

> Harness Hub 必须能独立工作；外部 observability backend 只能是可选 Exporter。

---

# 4.28 Permission / Policy Engine

Coding Agent 本质上可以获得：

```text
filesystem
shell
network
git
MCP
browser
credentials
process
package manager
```

因此 Harness Hub 必须建立统一权限模型。

### Permission Namespace

```text
filesystem.read
filesystem.write
filesystem.delete

shell.execute

process.spawn
process.kill

network.connect
network.listen

git.read
git.write
git.commit
git.push

mcp.invoke

secret.read

browser.open
browser.interact

package.install
```

### Policy Profile

示例：

```text
Read Only
├── filesystem.read
├── git.read
└── network.connect: deny

Safe Coding
├── project filesystem read/write
├── shell.execute
├── tests
├── git local read
└── git.push: ask

Normal
├── project filesystem
├── shell
├── git local
├── network
└── MCP: ask-by-server

Full
└── user-defined
```

### Scope

Policy 必须支持：

```text
global
runtime
project
harness
profile
identity
session
tool
```

优先级采用“越具体越优先”，但 deny 规则可配置为不可覆盖。

### Audit

每次敏感操作记录：

```text
permission
resource
decision
decision_source
requesting_harness
session
timestamp
```

这样可以统计：

```text
filesystem.read        183
filesystem.write        26
shell.execute            41
network.connect           3
secret.read               0
git.push                  1
```

### v0.1 边界

v0.1 只需要：

- 定义统一 Permission Schema。
- 将现有 Harness 权限状态映射为 best-effort capability。
- 记录 permission events。

真正的强制 Sandbox Enforcement 可以后置。

---

# 4.29 Harness Hub Plugin SDK

核心仓库不能随着 Harness 数量无限膨胀。

目标：

```text
安装第三方插件
→ 自动识别 Harness
→ 自动注册 Adapter
→ 不修改 Core
```

### 插件能力

统一暴露：

```text
HarnessAdapter
UsageAdapter
ProfileAdapter
TraceAdapter
SkillAdapter
RuntimeAdapter
```

插件 Manifest：

```toml
id = "example-harness"
name = "Example Harness"
version = "1.0.0"
api_version = "1"

[capabilities]
detect = true
launch = true
resume = true
usage = true
trace = true
profiles = false
mcp = true
skills = false
```

建议目录：

```text
plugin/
├── manifest.toml
├── adapter/
├── parser/
├── fixtures/
├── migrations/
└── icon.svg
```

### 插件边界

插件默认不得：

- 直接访问主数据库。
- 直接读取所有 Secrets。
- 任意修改其他 Harness 配置。
- 绕过 Permission Engine。
- 执行未声明的外部程序。

Core 向插件提供受限 API：

```text
fs.read_scoped
process.spawn_declared
secret.resolve_ref
db.emit_event
config.read_owned
config.write_projected
```

### 插件运行方式

优先级：

```text
1. Out-of-process plugin
2. WASM plugin（适合受限解析器）
3. In-process only for trusted built-ins
```

不建议默认加载任意第三方动态库进入 Tauri 主进程。

---

# 4.30 Eval / Regression Layer

Harness Hub 自己必须有“升级不退化”的能力。

## Adapter Golden Fixtures

每个 Adapter 必须带样本：

```text
fixtures/
├── codex/
│   ├── session-v1/
│   │   ├── raw/
│   │   ├── expected-events.jsonl
│   │   ├── expected-usage.json
│   │   └── expected-session.json
│   └── session-v2/
├── claude/
└── gemini/
```

升级后自动验证：

```text
session count
turn order
tool calls
token usage
timestamps
subagents
file events
git attribution
trace parent/child
```

## Contract Tests

所有 Adapter 必须通过统一 contract：

```text
detect does not mutate
parse is deterministic
import is idempotent
timestamps are normalized
unknown fields are preserved where possible
malformed records do not crash whole import
```

## Promptfoo Integration

Promptfoo 放在可选 Eval / Security 层，而不是 Core runtime。

用途：

- Prompt regression。
- Model comparison。
- Agent red-team。
- Repository prompt injection。
- Tool-output injection。
- Secret exfiltration test。
- Sandbox escape test。
- Dangerous shell behavior test。

### Release Gate

重大 Adapter / Provider / Permission Engine 变化：

```text
unit tests
+ fixture regression
+ contract tests
+ selected eval suite
```

全部通过后才能合并。

---

# 4.31 HHAR — Harness Hub Activity Record

Harness Hub 必须有数据库之外的开放数据格式。

暂定：

> **HHAR — Harness Hub Activity Record**

采用 versioned JSONL。

示例：

```json
{"schema":"hhar/1","type":"session.started","time":"...","project":"...","session":"...","harness":"codex"}
{"schema":"hhar/1","type":"model.request","trace_id":"...","provider":"openai","model":"..."}
{"schema":"hhar/1","type":"tool.call","tool":"shell","arguments":{}}
{"schema":"hhar/1","type":"usage","input_tokens":1234,"output_tokens":456,"source":"provider_reported"}
```

### 目标

支持：

```bash
harness-hub export --project OJ-NEXUS --format hhar
harness-hub export --session abc --format hhar
harness-hub import history.hhar
```

### 要求

- append-friendly。
- stream-friendly。
- schema versioned。
- forward-compatible。
- 未识别字段应尽量保留。
- secret 默认 redact。
- 支持 gzip。
- 可按 project / session / date 切片。

### HHAR 与 SQLite

```text
HHAR
= Portable source / exchange format

SQLite
= Indexed query / UI storage
```

数据库 schema 可以演进，但 HHAR 必须保证可迁移。

---

# 4.32 Runtime Target Abstraction

Harness 不一定运行在本机。

统一抽象：

```text
RuntimeTarget
├── Local
├── WSL
├── SSH
├── Docker / Podman
└── Dev Container
```

未来：

```text
Project
  ↓
RuntimeTarget
  ↓
Harness
  ↓
Session
```

### RuntimeAdapter

```text
detect
probe
spawn
exec
read_file
write_file
watch
environment
path_mapping
forward_port
```

### Path Mapping

例如：

```text
Windows
C:\Users\Tang\project

WSL
/mnt/c/Users/Tang/project
```

必须有 canonical project identity，防止同一项目因为路径不同被重复统计。

### v0.1

只实现：

```text
LocalRuntime
```

但所有核心接口从第一天就不得假定“只有本机路径”。

---

# 4.33 AI Coding Analytics

长期特色功能：

> **GitHub Contributions + ActivityWatch + Steam Year in Review for AI Coding**

指标可以包括：

```text
projects
sessions
active time
harness distribution
provider distribution
model distribution
token usage
cost
tool usage
file churn
commits
test runs
test pass rate
revert rate
session success rate
average session duration
permission usage
subagent usage
```

避免把“Token 越多”当作“进步越大”。

优先展示趋势：

```text
更少返工？
测试通过率是否上升？
平均 Session 是否更完整？
每次提交修改是否更集中？
失败 / 回滚比例是否下降？
不同 Harness 在什么任务上效果更好？
```

这些指标只能作为行为分析，不应伪装成开发者能力的绝对评分。

---

# 4.34 新的系统总图

```text
React / Tauri UI
        │
        ▼
Rust Control Plane
├── Harness Runtime
├── Config / Profile
├── Permission Engine
├── Trace / Event Core
├── Plugin Host
├── Git / Worktree
├── Runtime Targets
├── SQLite
└── HHAR Import / Export
        │
        ├──────────────────────────────┐
        │                              │
        ▼                              ▼
External Harnesses               Python AI Runtime
Codex / Claude / ...             ├── LiteLLM
                                 ├── Native SDKs
                                 ├── MCP
                                 ├── Tokenizers
                                 ├── HF
                                 └── Optional LangChain
        │                              │
        └──────────────┬───────────────┘
                       ▼
                 Canonical Trace
                       │
              ┌────────┴────────┐
              ▼                 ▼
           SQLite              HHAR
              │
              ▼
         Optional OTLP
              │
              ▼
   Phoenix / Langfuse / Grafana / ...
```

# 5. Adapter 设计

## 5.1 Capability Model

```rust
pub struct HarnessCapabilities {
    pub launch: bool,
    pub terminal: bool,
    pub resume: bool,
    pub usage: bool,
    pub replay: bool,
    pub tool_calls: bool,
    pub subagents: bool,
    pub live_state: bool,
    pub worktree: bool,
}
```

未来可扩展为枚举等级：

```text
Unsupported
Experimental
Supported
Native
```

---

## 5.2 HarnessAdapter

概念接口：

```rust
trait HarnessAdapter {
    fn id(&self) -> HarnessId;
    fn detect(&self) -> DetectResult;
    fn version(&self) -> Result<Option<String>>;
    fn capabilities(&self) -> HarnessCapabilities;
    fn launch(&self, request: LaunchRequest) -> Result<ProcessHandle>;
    fn resume(&self, request: ResumeRequest) -> Result<ProcessHandle>;
}
```

不要把日志解析硬塞进这里。

---

## 5.3 UsageAdapter

```rust
trait UsageAdapter {
    fn source(&self) -> HarnessId;
    fn discover(&self) -> Result<Vec<DataSource>>;
    fn import(&self, range: ImportRange) -> Result<ImportResult>;
    fn watch(&self) -> Result<()>;
}
```

第一阶段优先通过 ccusage JSON 接口归一化 Usage。

只在以下条件成立时自己写 source-specific adapter：

- ccusage 不支持。
- ccusage 丢失 Harness Hub 必需数据。
- 上游无法接受所需改动。
- 自己实现有明确、可测试、长期价值。

---

# 6. 统一数据模型

核心关系：

```text
Harness
  └── Project
      └── Session
          └── Turn
              ├── Message
              ├── ToolCall
              ├── UsageEvent
              ├── FileEvent
              └── GitEvent
```

建议主要表：

```text
harnesses
projects
project_harness_settings
sessions
turns
messages
usage_events
models
tool_calls
file_events
git_events
imports
source_files

providers
provider_endpoints
provider_profiles
identities
credential_refs
profile_bindings
config_snapshots
config_projections
config_mutations
config_ownership
model_requests
model_events
token_measurements

mcp_servers
mcp_tools
mcp_resources
mcp_calls

model_assets
local_model_runtimes

traces
spans
events
permission_policies
permission_decisions
runtime_targets
plugins
plugin_capabilities
plugin_versions
eval_runs
eval_cases
export_jobs
import_jobs
```

### Session 必须区分

```text
hub_session_id
source_session_id
harness_id
project_id
parent_session_id
started_at
ended_at
status
launch_mode
cwd
worktree_path
```

避免把不同 Harness 的原始 session ID 当全局 ID。

---

# 7. 外部项目复用策略

> 外部项目状态以 **2026-09-21** 核对结果为基准；真正复制代码前再次核对 License 与版本。

## ccusage

Repository:

https://github.com/ccusage/ccusage

当前作用：

- 多 Coding Agent 本地 Usage 统一分析。
- Daily / Weekly / Monthly / Session。
- Model Breakdown。
- Estimated Cost。
- JSON 输出。

当前 README 列出的数据源包括：

- Claude Code
- Codex
- OpenCode
- Amp
- Droid
- Codebuff
- Hermes Agent
- pi-agent
- Goose
- OpenClaw
- Kilo
- Kimi
- Qwen
- GitHub Copilot CLI
- Gemini CLI
- Antigravity
- Grok Build CLI
- ZCode

License：MIT。

### 决策

**作为 Usage Engine 第一选择。**

不要复制一堆 parser 到自己仓库后永久维护；优先调用稳定 JSON 输出或抽象成可替换 Provider。

---

## CCManager

Repository:

https://github.com/kbwo/ccmanager

可借鉴：

- 多 Harness launch。
- PTY / Session 管理。
- busy / waiting / idle 状态。
- Git Worktree。
- Session restore。
- 项目级配置。

当前 README 支持包括 Claude Code、Gemini CLI、Codex CLI、Cursor Agent、Copilot CLI、Cline CLI、OpenCode、Kimi CLI、MiniMax Code。

License：MIT。

### 决策

重点研究并复用兼容模块 / 设计思路，不直接把整个 TUI 套进桌面端。

---

## Agent Trail

Repository:

https://github.com/camtrik/agent-trail

可借鉴：

- Session Index。
- Full-text Search。
- Replay。
- Tool Call 展示。
- Subagent Tree。
- SQLite ingestion。
- Live activity feed。

当前覆盖 Claude Code、Codex、OpenCode、OpenClaw、Qoder。

### 决策

**把它当 Session Observability 的重要参考实现。**

在没有明确确认可复用 License 前，不直接复制其源码；可以复现公开功能和独立实现架构。

---

## ccdash

Repository:

https://github.com/jedarden/ccdash

参考：

- Claude / Codex 实时状态。
- Hook tracking。
- tmux / process inspection。
- Token cache。
- Session state model。

### 决策

主要用于研究“实时状态检测怎么做”，不作为核心依赖。

---

## VibeUsage

Repository:

https://github.com/tyuan511/vibe-usage

参考：

- 自动扫描本地 Agent 数据源。
- Incremental ingestion。
- Usage SQLite。
- 本地优先隐私设计。
- Usage 热力图。

License：MIT。

### 决策

借鉴 scanner 和历史可视化产品思路。

---

# 8. 仓库结构

建议：

```text
harness-hub/
├── README.md
├── LICENSE
├── CONTRIBUTING.md
├── AGENTS.md
├── package.json
├── src/
│   ├── app/
│   ├── components/
│   ├── features/
│   │   ├── dashboard/
│   │   ├── harnesses/
│   │   ├── history/
│   │   ├── projects/
│   │   ├── sessions/
│   │   ├── settings/
│   │   └── terminal/
│   └── lib/
├── src-tauri/
│   └── src/
│       ├── db/
│       ├── git/
│       ├── harness/
│       │   ├── adapter.rs
│       │   ├── registry.rs
│       │   └── adapters/
│       ├── process/
│       ├── pty/
│       ├── session/
│       ├── sidecar/
│       ├── trace/
│       ├── permissions/
│       ├── plugins/
│       ├── runtime/
│       ├── hhar/
│       ├── eval/
│       ├── usage/
│       └── watcher/
├── python/
│   ├── pyproject.toml
│   └── harness_hub_runtime/
│       ├── rpc/
│       ├── providers/
│       │   ├── gateway.py
│       │   ├── litellm_adapter.py
│       │   ├── openai_adapter.py
│       │   ├── anthropic_adapter.py
│       │   └── google_adapter.py
│       ├── mcp/
│       ├── tokenization/
│       ├── hf/
│       ├── local_models/
│       └── plugins/
│           └── langchain/
├── plugins/
│   ├── sdk/
│   └── examples/
├── schemas/
│   ├── hhar/
│   ├── events/
│   └── plugin-manifest/
├── fixtures/
│   ├── codex/
│   ├── claude/
│   └── gemini/
├── evals/
│   └── promptfoo/
├── tests/
│   ├── fixtures/
│   ├── integration/
│   └── e2e/
└── docs/
    ├── CONTEXT.md
    ├── CONTEXT-MAP.md
    ├── adr/
    ├── specs/
    └── plans/
```

---

# 9. Roadmap

## Phase 0 — Research / Grill

目标：证明“统一层”边界合理，而不是立刻写 GUI。

完成条件：

- [ ] 建立项目 `CONTEXT.md`。
- [ ] 建立 `CONTEXT-MAP.md`。
- [ ] 核对 ccusage / CCManager / Agent Trail / ccdash / VibeUsage。
- [ ] 建立复用与 License 表。
- [ ] 验证 Windows / Linux 上 Codex、Claude、Gemini、OpenCode 的数据路径。
- [ ] 验证 ccusage JSON 是否足够支撑 v0.1 Dashboard。
- [ ] 写 ADR-0001：Tauri 2。
- [ ] 写 ADR-0002：HarnessAdapter / UsageAdapter 分离。
- [ ] 写 ADR-0003：local-first。
- [ ] 写 ADR-0004：SQLite schema strategy。

**Gate：上述事实没验证完，不进入大规模 UI 开发。**

---

## Phase 1 — Walking Skeleton

目标：端到端跑通一条最细链路。

只支持：

```text
Codex
+
一个 Project
+
一个 Terminal
+
一条 Usage 导入
+
一个 Dashboard 数字
```

完成条件：

- [ ] Tauri 启动。
- [ ] React UI。
- [ ] SQLite migration。
- [ ] Detect Codex。
- [ ] Launch Codex PTY。
- [ ] Session 写数据库。
- [ ] ccusage JSON ingestion。
- [ ] Dashboard 展示 Token。
- [ ] 退出重启后 Session 历史仍存在。

这一阶段成功后，架构才算真的成立。

---

## Phase 2 — Multi-Harness Core

增加：

- Claude Code。
- Gemini CLI。
- OpenCode。

完成条件：

- [ ] Adapter registry。
- [ ] Capability UI。
- [ ] Project → Harness launcher。
- [ ] 多 Terminal Session。
- [ ] Usage 跨 Harness 汇总。
- [ ] Model / Project / Harness breakdown。

---

## Phase 3 — History

实现项目真正差异化部分。

- [ ] Activity Calendar。
- [ ] Session Timeline。
- [ ] Project History。
- [ ] Git Diff summary。
- [ ] Commit tracking。
- [ ] 时间筛选。
- [ ] Harness / Model / Project 筛选。

---

## Phase 4 — Session Observability

- [ ] Session replay。
- [ ] Message search。
- [ ] Tool call 展示。
- [ ] Parent / Child / Subagent tree。
- [ ] Deep link 到 Harness Session。

必须 capability-aware：不支持的 Harness 不伪造功能。

---

## Phase 5 — Worktree / Parallel Agents

借鉴 CCManager：

- [ ] 创建 Worktree。
- [ ] Harness Session 绑定 Worktree。
- [ ] Worktree 状态。
- [ ] Merge / Delete flow。
- [ ] 并行 Agent Session。

---

## Phase 6 — More Harnesses

按 Adapter 成本逐步增加：

- Kimi。
- Qwen。
- Copilot CLI。
- Cline CLI。
- Cursor Agent。
- Droid。
- Amp。
- Goose。
- Hermes。
- pi-agent。
- ZCode。
- 其他 ccusage 已支持的数据源。

**不要为了“数量好看”添加不可维护的假支持。**

---

## Phase 7 — Configuration & Profile Plane

### 7.1 Provider / Profile Registry

- [ ] Unified ProviderProfile schema。
- [ ] Import current Claude / Codex / Gemini configs。
- [ ] Detect currently active profile。
- [ ] Profile validation。
- [ ] Provider preset registry。

### 7.2 Safe Config Projection

- [ ] JSON / TOML / env structured writers。
- [ ] Atomic write。
- [ ] Snapshot before mutation。
- [ ] Read-back verification。
- [ ] Ownership-aware merge。
- [ ] Restore official / original config。

### 7.3 Credential / Identity

- [ ] Secret-store abstraction。
- [ ] Credential refs only in SQLite。
- [ ] Official OAuth identity discovery where safely supported。
- [ ] Multi-account profile model。
- [ ] Opaque credential-envelope snapshot strategy where applicable。
- [ ] Config Doctor。

### 7.4 MCP / Skills / Prompts

- [ ] Unified MCP registry。
- [ ] Harness-specific MCP bindings。
- [ ] Skills registry。
- [ ] Canonical prompt artifact。
- [ ] CLAUDE.md / AGENTS.md / GEMINI.md renderers。
- [ ] Import / diff / preview。

### 7.5 Tray

- [ ] Current profile display。
- [ ] One-click profile switch。
- [ ] Active session display。
- [ ] Switch audit logging。

### 7.6 Routing

- [ ] Direct / Gateway mode abstraction。
- [ ] LiteLLM Proxy integration。
- [ ] Endpoint health metadata。
- [ ] Failover policy model。
- [ ] No custom proxy engine unless proven necessary。

---

## Phase 8 — Unified AI Runtime

这一阶段让 Harness Hub 从“只管理外部 Harness”升级为“同时理解 Harness 与底层 Model Provider”。

### 8.1 Python Sidecar

- [ ] Rust 能启动 / 关闭 Python sidecar。
- [ ] stdio JSON-RPC handshake。
- [ ] Runtime version / capability negotiation。
- [ ] Sidecar crash 可恢复。
- [ ] 无 Python AI 功能时主 Harness 功能仍能工作。

### 8.2 Provider Gateway

- [ ] LiteLLM adapter。
- [ ] OpenAI native adapter。
- [ ] Anthropic native adapter。
- [ ] Google Gen AI native adapter。
- [ ] Unified / Native mode。
- [ ] Streaming event normalization。
- [ ] Provider error normalization。

### 8.3 MCP

- [ ] MCP Client registry。
- [ ] stdio。
- [ ] Streamable HTTP。
- [ ] Tool discovery。
- [ ] MCP permission policy。
- [ ] Harness Hub MCP Server（只读工具先行）。

### 8.4 Tokenizer

- [ ] Provider-reported usage 优先。
- [ ] OpenAI token count / tiktoken。
- [ ] HF tokenizers。
- [ ] `estimated` / `exact` 标记。
- [ ] Tokenizer-model revision 绑定。

### 8.5 Hugging Face / Local Model

- [ ] HF repo metadata。
- [ ] Cache discovery。
- [ ] Explicit download。
- [ ] Revision pinning。
- [ ] Transformers experimental runtime。
- [ ] Local runtime capability API。

### 8.6 LangChain Plugin

只有真实 workflow 需求出现时再开启：

- [ ] 独立 optional extra。
- [ ] 不污染 core schema。
- [ ] 不成为 provider gateway。
- [ ] 至少一个有实际价值的 workflow 后再发布。

---


---

## Phase 9 — Trace / Plugin / Safety Foundation

### 9.1 Canonical Event Model

- [ ] Event schema v1。
- [ ] Trace / Span model。
- [ ] Codex events mapped into canonical trace。
- [ ] Tool / shell / file / git events。
- [ ] OpenTelemetry-compatible attributes。
- [ ] Trace replay test。

### 9.2 Permission Engine

- [ ] Permission namespace。
- [ ] Policy precedence。
- [ ] Session permission audit。
- [ ] Ask / Allow / Deny model。
- [ ] Secret / network / filesystem categories。
- [ ] UI permission timeline。

### 9.3 Plugin SDK

- [ ] Manifest schema。
- [ ] Adapter contracts。
- [ ] Capability negotiation。
- [ ] Out-of-process plugin prototype。
- [ ] Example plugin。
- [ ] Plugin permission boundary。

### 9.4 Regression / Eval

- [ ] Golden fixtures。
- [ ] Adapter contract tests。
- [ ] Import idempotency tests。
- [ ] Promptfoo optional suite。
- [ ] Release regression gate。

### 9.5 HHAR

- [ ] `hhar/1` schema。
- [ ] Export。
- [ ] Import。
- [ ] Redaction。
- [ ] Round-trip test。
- [ ] Gzip streaming。

---

## Phase 10 — Remote Runtime

- [ ] RuntimeTarget schema。
- [ ] LocalRuntime adapter。
- [ ] WSL adapter。
- [ ] SSH adapter。
- [ ] Container adapter。
- [ ] Canonical path mapping。
- [ ] Remote Harness detection。
- [ ] Remote PTY / process lifecycle。

---

## Phase 11 — Analytics / Review

- [ ] Activity heatmap。
- [ ] Harness / Model / Provider trends。
- [ ] Cost / Token trends。
- [ ] Tool usage。
- [ ] Test pass / failure trend。
- [ ] Revert / rollback trend。
- [ ] Project timeline。
- [ ] Monthly / yearly report。

# 10. v0.1 页面设计

## Home / Dashboard

```text
Today

Tokens          12.8M
Sessions        23
Coding Time     6h 42m
Estimated Cost  $41.27

Harness
Codex       █████████████ 47%
Claude      █████████     31%
Gemini      ████          13%
OpenCode    ██             9%

Projects
OJ-NEXUS       4.8M
website        3.1M
algorithm      2.4M
```

## Harnesses

```text
Codex          Installed  v...
Claude Code    Installed  v...
Gemini CLI     Installed  v...
OpenCode       Installed  v...
Kimi           Missing
```

点开后看到 Capability Matrix。

## Projects

每个项目：

```text
Sessions
Tokens
Models
Harnesses
Last Active
Git Activity
```

## Sessions

```text
2026-09-21 20:17
Codex
project: harness-hub
model: ...
tokens: ...
duration: ...
git: +421 -88
```

## Activity

类似 GitHub heatmap：

```text
Mon Tue Wed Thu Fri Sat Sun
░   ▓   █   █   ▒   ░   ░
▒   █   █   ▓   █   ▒   ░
█   █   █   █   █   ▓   ▒
```

点一天展开所有 Session。

---

# 11. 测试策略

## Unit

测试：

- Path detection。
- Capability resolution。
- Parser normalization。
- Cost normalization。
- DB repository。
- Git diff calculations。

## Adapter Contract Tests

每个 Harness Adapter 必须跑同一套 contract：

```text
detect does not crash
missing binary returns NotInstalled
version failure is recoverable
launch arguments are deterministic
capabilities are explicit
```

## Fixture Tests

保存匿名 / 合成 fixture：

```text
tests/fixtures/codex/
tests/fixtures/claude/
tests/fixtures/gemini/
```

绝不能把真实用户 Prompt、Token、API Key 或私有代码 commit 到测试仓库。

## Integration

至少测试：

```text
fixture → ingest → sqlite → query → UI DTO
```

## E2E

最关键三条：

1. 添加 Project → Launch Harness → Session 出现。
2. 导入 Usage → Dashboard 数字正确。
3. 重启 Harness Hub → 历史仍可读取。

---

# 12. 安全与隐私

## Secrets

禁止持久化：

- API Key 明文。
- OAuth Token 明文。
- Harness 登录 cookie。

若未来必须保存 secret：

> 使用 OS Keychain / Credential Manager。

## Logs

应用自身日志不得默认打印：

- Prompt 全文。
- Response 全文。
- Source code。
- Environment secrets。

Debug mode 也应有 redaction。

## Session Replay

原始 Session 内容属于敏感本地数据。

必须：

- 默认本地。
- 明确显示来源路径。
- 提供 exclude project / exclude harness。
- 支持关闭内容索引，只保留 usage metadata。

---

# 13. 性能原则

不能每打开 Dashboard 就重新扫描几十 GB 日志。

采用：

```text
Initial Scan
    ↓
Incremental Import
    ↓
Source Fingerprint
    ↓
File Watcher
    ↓
Only changed sources re-ingested
```

记录：

```text
source path
size
mtime
hash / cursor
last imported offset
```

大量 JSONL 采用流式解析。

Dashboard 默认查 SQLite 聚合表，不直接扫原始日志。

---

# 14. 风险登记

## R1：Harness 日志格式变化

Mitigation：

- Adapter isolation。
- Fixture regression tests。
- Version detection。
- 不把 source-specific schema 泄漏到 UI。

## R2：ccusage 作为外部依赖变化

Mitigation：

- 自己定义内部 `UnifiedUsageEvent`。
- ccusage 只是 Provider。
- Provider 可替换。

## R3：不同 Harness 的 Session 语义完全不同

Mitigation：

- 保留原始 `source_session_id`。
- 统一模型只抽公共部分。
- 其余数据放 source-specific metadata。

## R4：成本统计产生误解

Mitigation：

UI 区分：

```text
Reported Cost
Estimated Cost
Subscription / Unknown
```

禁止把 Estimated Cost 表述成真实账单。

## R5：Git Diff 被误认为 AI 贡献

Mitigation：

用词：

> “Changes during session”

而不是：

> “AI wrote 4,291 lines”。

## R6：Scope 膨胀

Mitigation：

每个新 Feature 先经过 Grill Gate。

如果不能回答：

> “它是否让统一 Harness 工作流更完整？”

默认不进 v0.1。

## R7：Provider 抽象套娃

风险：

```text
LangChain → LiteLLM → Official SDK → HTTP
```

层数过多后，stream、error、tool call、usage 都难以定位。

Mitigation：

- 默认最多一个统一层。
- `Unified Mode` 明确走 LiteLLM。
- `Native Mode` 明确直接走官方 SDK。
- LangChain 只允许在 workflow plugin 中出现。
- 每个 Model Request 记录真实 execution path。

## R8：Token 统计“看起来精确但其实不精确”

Mitigation：

Token measurement 强制记录来源：

```text
provider_reported
provider_count_api
local_exact
estimated
```

不同来源不可在 UI 中无标记混为一谈。


## R10：配置文件被 Harness Hub 写坏

Mitigation：

- Structured parser / renderer。
- Mutation 前 snapshot。
- Atomic replace。
- Verify read-back。
- Config Doctor。
- 可一键 Restore。

## R11：OAuth / API Secret 泄露

Mitigation：

- Secret 不入普通 SQLite。
- 不写日志。
- UI 默认遮罩。
- Clipboard 自动清理作为可选能力。
- 导出默认剥离 secrets。
- OS Secret Store。
- Debug bundle 自动 redact。

## R12：多个工具同时修改同一个配置

Mitigation：

- 文件版本 / mtime / hash 检测。
- 乐观并发控制。
- 写入前 diff。
- Conflict UI。
- 不静默覆盖 external mutation。


## R13：Trace 数据量爆炸

Mitigation：

- Raw / normalized 分层。
- 大字段按需存储。
- Retention policy。
- Trace sampling。
- 压缩。
- 大型 tool output 只存 hash + preview + optional blob。

## R14：插件破坏主程序

Mitigation：

- out-of-process。
- capability declaration。
- permission sandbox。
- crash isolation。
- API version negotiation。
- 插件签名 / trust level 后续支持。

## R15：权限模型“只展示不强制”

Mitigation：

- v0.1 明确标记 observed / enforced。
- 不把 Harness 自己的权限提示伪装成 Harness Hub 强制沙箱。
- 后续 RuntimeAdapter 增加真正 enforcement。

## R16：开放事件格式过早冻结

Mitigation：

- `hhar/1` 明确 version。
- unknown-field preservation。
- extension namespace。
- migration tests。
- experimental 字段不进入 stable namespace。

## R17：Remote Runtime 路径与身份混乱

Mitigation：

- runtime_target_id。
- canonical project id。
- explicit path mapping。
- remote identity 独立。
- 不用字符串路径作为项目唯一主键。

## R9：Python / Torch 使桌面发行包膨胀

Mitigation：

- Python Runtime feature-gated。
- `transformers` / `torch` 不进入默认最小安装。
- Provider-only 用户无需安装 local extra。
- 本地模型资产与 Runtime 单独管理。


---

# 15. Agent 开发规范

根目录建立 `AGENTS.md`，要求所有 Agent：

## 开工前

1. 读 `docs/CONTEXT.md`。
2. 读 `docs/CONTEXT-MAP.md`。
3. 找相关 ADR。
4. 搜现有实现，禁止凭感觉新建重复 abstraction。
5. 非平凡功能必须有 spec / plan。

## 开发时

- TDD。
- 小 Task。
- 小 Commit。
- 不跨无关模块重构。
- 不偷偷改变 schema。
- 不在没有证据时宣称完成。

## 完成前

至少执行：

```text
format
lint
unit tests
integration tests
typecheck
build
```

根据 Feature 再执行 E2E。

---

# 16. Definition of Done

任何 Feature 只有满足以下条件才算 Done：

- [ ] Goal 与 Acceptance Criteria 都满足。
- [ ] Non-goals 没被偷偷扩大。
- [ ] Test 先于或伴随实现建立。
- [ ] 全部相关 Tests 通过。
- [ ] Build 通过。
- [ ] 没有已知 Critical / High review issue。
- [ ] 新架构决策已更新 ADR。
- [ ] 新稳定事实已更新 CONTEXT。
- [ ] 用户可见行为有文档。
- [ ] Error path 有明确行为。
- [ ] 没泄漏隐私 / secrets。
- [ ] 手工验收通过。

---

# 17. Feature 开发模板

以后每个功能先复制下面模板：

```markdown
# <Feature> Spec

## Problem
为什么要做？

## Locked Goal
做完后用户能完成什么？

## Acceptance Criteria
- [ ] ...
- [ ] ...

## Non-goals
- ...

## Evidence / Existing Solutions
- Existing code:
- Upstream projects:
- Docs:

## Grill Report
### Goals
### Acceptance
### Boundaries
### Alternatives
### Assumptions

## Decision
选什么？

## Rejected Alternatives
为什么不选？

## Risks

## Open Questions

## Verification
怎么证明真的完成？
```

Spec 锁定后才生成 Superpowers 风格 Implementation Plan。

---

# 18. Implementation Plan 模板

路径：

```text
docs/plans/YYYY-MM-DD-<feature>.md
```

格式：

```markdown
# <Feature> Implementation Plan

> Implement task-by-task using subagent-driven development or executing-plans.

## File Map

### Task 1: ...

**Files**
- Create: `...`
- Modify: `...`
- Test: `...`

**Step 1 — Write failing test**

**Step 2 — Run test and verify failure**

**Step 3 — Minimal implementation**

**Step 4 — Run test and verify pass**

**Step 5 — Refactor if needed**

**Step 6 — Verification**

**Step 7 — Commit**
```

---

# 19. 第一批 ADR

## ADR-0001 — Tauri 2 而不是 Electron

原因：

- 本项目高度依赖本机进程、PTY、文件系统、Git、SQLite。
- Rust core 适合承担本地管理层。
- React 保留高效 UI 生态。

重新评估条件：

- PTY 跨平台维护成本远高于预期。
- Tauri 插件体系成为明显阻碍。

## ADR-0002 — Harness 与 Usage Adapter 分离

原因：

- 启动能力和 usage 数据能力不一致。
- 避免为了“统一接口”制造虚假能力。

## ADR-0003 — Local-first

原因：

- Coding session 可能含私有代码与凭据。
- 本项目无需云端即可提供主要价值。

## ADR-0004 — SQLite 为统一索引数据库

原因：

- 本地部署简单。
- 足够支持历史、聚合、全文索引和增量 ingest。

## ADR-0005 — External-first / Adapter-first

原则：

> 能调用上游就调用上游；能包装就包装；只有在明确价值成立时才 fork / rewrite。

## ADR-0006 — Python AI Runtime 使用独立 Sidecar

原因：

- AI Python 生态成熟。
- 不把 Python / Torch / Provider SDK 强行嵌入 Rust core。
- Sidecar crash 不应拖死 PTY / Git / Session 核心。

默认 transport：

> stdio JSON-RPC。

## ADR-0007 — LiteLLM First，Official SDK Escape Hatch

公共 Provider 能力优先 LiteLLM。

需要 OpenAI / Anthropic / Google 原生新特性时直接走官方 SDK。

禁止为了接口“看起来统一”而降级能力。

## ADR-0008 — Token Truth Precedence

```text
Provider reported
> Provider count API
> Exact matching tokenizer
> Estimate
```

所有 token measurement 必须记录来源与准确级别。

## ADR-0009 — MCP 是标准边界

Harness Hub 同时是 MCP Client，并可选择成为 MCP Server。

MCP 不替代内部 Rust ↔ Python RPC。

## ADR-0010 — LangChain Optional

LangChain 仅用于用户 workflow / orchestration 插件。

核心 Provider、Harness、Usage、MCP 不依赖 LangChain。


## ADR-0011 — Configuration Plane 独立于 Harness Runtime

启动 Harness 与修改 Harness 配置是两种权限等级、两套能力接口。

因此 `HarnessAdapter` 与 `ProfileAdapter` 必须分离。

## ADR-0012 — Secrets Never in Main SQLite

主数据库只保存 `credential_ref`。

Secret 优先进入 OS Secret Store。

导出配置默认不包含 Secret；只有用户显式选择 encrypted secret export 才允许。

## ADR-0013 — Atomic Projection

Harness 原生配置始终通过：

```text
snapshot → render → validate → atomic write → verify
```

修改。

不允许直接字符串替换。

## ADR-0014 — Direct Mode and Gateway Mode Coexist

Harness Hub 不强迫所有 Harness 经本地代理。

用户可以：

```text
Direct
Harness → Provider

Gateway
Harness → LiteLLM / local gateway → Provider
```

## ADR-0015 — Profile / Identity / Credential 分离

Provider Profile 是可复用配置；
Identity 是账号身份；
Credential 是 Secret。

三者不可共用一个数据库对象。


## ADR-0016 — Canonical Event First

所有 Harness / Provider / Tool 数据先归一为 Canonical Event，再进入 Replay / Analytics / Export。

UI 不直接依赖某个 Harness 原始日志结构。

## ADR-0017 — Trace Schema 对齐开放标准

内部字段优先对齐 OpenTelemetry GenAI / OpenInference。

不为短期 UI 便利发明不可迁移的私有语义。

## ADR-0018 — Plugin Core Isolation

第三方插件默认 out-of-process。

插件不得直接访问主数据库和 Secret Store，只能通过受限 Host API。

## ADR-0019 — Portable Data Before Analytics Lock-in

所有长期用户活动数据必须可导出为 HHAR。

SQLite 是实现细节，不是用户数据的唯一容器。

## ADR-0020 — Permission Model Is First-Class Data

权限不是 UI 开关，而是可查询、可审计、可导出的事件。

## ADR-0021 — Runtime Is Not Always Local

Core API 不允许假定绝对本机路径或本机进程。

所有 Harness 运行必须关联 `runtime_target_id`。

## ADR-0022 — Eval Before Adapter Expansion

新 Harness Adapter 必须有 fixture + contract test。

“能检测到”不算支持，“可稳定回归”才算支持。

---

# 20. v0.1 开发顺序

严格按下面顺序，不先做炫酷 Dashboard：

```text
1. Research / Grill
2. Tauri skeleton
3. SQLite migrations
4. Harness registry
5. Codex detect
6. PTY launch
7. Session persistence
8. ccusage provider
9. Usage normalization
10. Minimal dashboard
11. Claude adapter
12. Gemini adapter
13. OpenCode adapter
14. Project registry
15. Activity timeline
16. Git observer
17. Packaging
```

**原则：每一步都必须保持项目可运行。**

---

# 21. v0.1 Release Gate

满足下面条件才发布 `0.1.0`：

### Platform

- [ ] Windows 可安装运行。
- [ ] Linux 可安装运行。

### Harness

至少：

- [ ] Codex。
- [ ] Claude Code。
- [ ] Gemini CLI。
- [ ] OpenCode。

### Core

- [ ] Harness 自动检测。
- [ ] 内置 Terminal 启动 Session。
- [ ] 多项目。
- [ ] Session 历史。
- [ ] Usage 导入。
- [ ] Token / Cost / Model Dashboard。
- [ ] Activity Timeline。
- [ ] Git Session Snapshot。

### Quality

- [ ] Crash 不导致数据库损坏。
- [ ] 外部日志只读。
- [ ] Import 可重复执行且不会重复计数。
- [ ] 主要 Adapter 有 fixtures。
- [ ] E2E 主流程通过。
- [ ] README 能让新用户 10 分钟内跑起来。

---

# 22. v0.2 以后再考虑

- Unified AI Runtime UI。
- LiteLLM Provider Gateway。
- OpenAI / Anthropic / Google Native Provider Mode。
- MCP Client / Server 管理。
- Hugging Face Model Registry。
- Local Model Runtime。
- LangChain workflow plugin（只有出现真实需求时）。
- OpenTelemetry / OTLP exporter。
- OpenInference exporter。
- Promptfoo eval / red-team presets。
- Third-party Plugin SDK beta。
- HHAR public schema。
- WSL / SSH / Container Runtime。
- Permission enforcement / sandbox adapters。
- Monthly / yearly AI Coding Review。
- Full Session Replay。
- Global Session Search。
- Tool Call visualization。
- Subagent tree。
- Worktree GUI。
- Parallel Agent orchestration。
- Cross-device aggregated metrics。
- Export JSON / CSV。
- Annual Review。
- Plugin SDK。
- Harness community adapters。

---

# 23. 项目的真正护城河

不是：

> “我们支持 30 个 Agent。”

因为这种数量很快会被别人追平。

真正价值应该是：

### 统一控制

用户不用关心当前正在用哪个 Harness。

### 统一历史

用户第一次真正拥有跨 Agent、跨模型、跨项目的 AI Coding 时间线。

### 统一可观察性

Token、Cost、Tool Call、Session、Git Activity 被放进同一个上下文。

### 可扩展 Adapter

新 Harness 加入时无需修改整个系统。

### AI-native development history

最终能回答：

> “过去半年，我到底用 AI 做了哪些项目？我主要用什么工具？在哪些阶段 Token 消耗最多？哪些 Session 最有效？我的开发方式是怎么变化的？”

这比单纯 Token Dashboard 有更长期的价值。

---

# 24. 当前锁定方案

```text
Product
Harness Hub

Positioning
Universal local-first AI Coding Harness control center

Desktop
Tauri 2

Frontend
React + TypeScript + Vite + Tailwind + shadcn/ui

Core
Rust

Database
SQLite

Usage
ccusage-first

AI Runtime
Python sidecar + stdio JSON-RPC

Provider Gateway
LiteLLM-first

Native Provider SDKs
OpenAI Python SDK + Anthropic SDK + Google Gen AI SDK

MCP
MCP Python SDK v2

Tokenization
Provider-reported > official count > tiktoken/tokenizers > estimate

Hugging Face
huggingface_hub + tokenizers

Local Model
transformers as experimental adapter, not permanent universal runtime

Orchestration
LangChain optional plugin only

Session / PTY reference
CCManager

Observability reference
Agent Trail

Live-state reference
ccdash

Scanner / activity reference
VibeUsage

Development Method
GrillMe → ADR / Locked Spec → Superpowers → TDD → Review → Verify

Primary Platforms
Windows + Linux

v0.1 Core Harnesses
Codex + Claude Code + Gemini CLI + OpenCode
```

---

# 25. 下一步唯一正确动作

不是开始画十几个页面。

而是创建仓库后先完成：

```text
Phase 0
├── CONTEXT.md
├── CONTEXT-MAP.md
├── ADR-0001 ~ ADR-0005
├── upstream-research.md
├── capability-matrix.md
└── v0.1-walking-skeleton-spec.md
```

然后对 **Walking Skeleton** 再跑一次 GrillMe：

> “只用 Codex，能否完整跑通 Detect → Launch → Session → Usage → SQLite → Dashboard？”

如果这条最小链路成立，再按 Superpowers 写 Implementation Plan，然后进入第一行正式业务代码。

---

# References

- Superpowers: https://github.com/obra/superpowers
- Superpowers writing plans: https://github.com/obra/superpowers/blob/main/skills/writing-plans/SKILL.md
- GrillMe: https://github.com/hiDaDeng/grill-me
- ccusage: https://github.com/ccusage/ccusage
- CCManager: https://github.com/kbwo/ccmanager
- Agent Trail: https://github.com/camtrik/agent-trail
- ccdash: https://github.com/jedarden/ccdash
- VibeUsage: https://github.com/tyuan511/vibe-usage
- CC Switch: https://github.com/redeyespc/cc-switch
- ccswitch (multi-account credential/profile design): https://github.com/nhtera/ccswitch
- ccswitch (isolated concurrent sessions/session recall reference): https://github.com/mysqto/ccswitch
- CCS / runtime-profile manager reference: https://github.com/cresseelia/ccswitch
- OpenAI Python SDK: https://github.com/openai/openai-python
- Anthropic Python SDK: https://github.com/anthropics/anthropic-sdk-python
- Google Gen AI Python SDK: https://github.com/googleapis/python-genai
- MCP Python SDK: https://github.com/modelcontextprotocol/python-sdk
- LiteLLM: https://github.com/BerriAI/litellm
- Hugging Face Hub: https://huggingface.co/docs/huggingface_hub/
- Hugging Face Tokenizers: https://huggingface.co/docs/tokenizers/
- Transformers: https://huggingface.co/docs/transformers/
- tiktoken: https://github.com/openai/tiktoken
- LangChain: https://docs.langchain.com/oss/python/
- OpenTelemetry GenAI semantic conventions: https://opentelemetry.io/docs/specs/semconv/gen-ai/
- OpenInference: https://github.com/Arize-ai/openinference
- Promptfoo: https://github.com/promptfoo/promptfoo


---

# Architecture Stability Gate

在项目宣称“支持一个 Harness”之前，至少满足：

```text
Detect
✓

Launch / Read
✓

Capability Matrix
✓

Fixture
✓

Contract Test
✓

Canonical Event Mapping
✓

Usage source labelled
✓

Config mutation safe / or explicitly unsupported
✓

Permission visibility
✓

HHAR export survives round-trip
✓
```

在项目宣称“支持一个 Remote Runtime”之前：

```text
Path mapping
✓

Process lifecycle
✓

PTY lifecycle
✓

File access
✓

Secret boundary
✓

Disconnect recovery
✓

Trace continuity
✓
```

在项目宣称“安全支持某权限”之前：

```text
observed
or
enforced
```

必须明确标记，禁止模糊表述。
