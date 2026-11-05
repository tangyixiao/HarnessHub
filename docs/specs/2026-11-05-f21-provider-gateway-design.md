# F21 Provider Gateway + Tokenizer — 设计规格（冻结契约）

- 日期：2026-11-05
- 对应路线图：`docs/ROADMAP.md` §F21（Track F 阶段 21）
- 对应规格源：`HARNESS_HUB_PROJECT_PLAN.md` §4.4 / §4.5 / §8.2 / §8.4、ADR-0007、ADR-0008（R10）、
  `docs/adr/0003-local-first.md`、`docs/adr/0005-capability-vs-readiness.md`
- 前置阶段：F20（Python sidecar 生命周期 / stdio JSON-RPC / 握手）已合入 `main`（`6d6c327`）
- 实施计划：`docs/plans/2026-11-05-f21-provider-gateway.md`

> **本文件是跨语言契约的唯一事实来源。** Python / Rust / TypeScript 三侧的键名、枚举值、
> 嵌套形状、默认值一律以此为准。任何一侧要改契约，先改本文件并同步三侧与各自 fixture，
> 不允许「看起来应该是 camelCase」式猜测（AGENTS.md「跨 IPC 的契约纪律」）。

---

## 1. 目标 / 验收 / 非目标

### 1.1 目标

在 **零新增第三方依赖** 的前提下，为 Provider Gateway 与 Tokenizer 建立**可独立测试、可跨
IPC 验证的契约层**：

1. Python sidecar 提供 `providers` 契约层：DTO、streaming event 归一化、provider error 归一化、
   unified / native 路由决策、lazy-import 优雅降级。
2. Python sidecar 提供 `tokenization` 契约层：四级可信度 precedence、`estimated` / `local_exact`
   两类本地来源、tokenizer-model revision 绑定。
3. RPC 方法表 + 能力协商扩展（保持 F20 握手与集成测试不破坏）。
4. Rust `GatewayStatus` DTO + 安全 decoder + 薄命令（跨 IPC 契约测试）。
5. 前端 `normalizeGatewayStatus` fixture + `ProviderGatewayCard`。

### 1.2 验收标准

| #   | 验收                                                                                                  | 证据                                                                                            |
| --- | ----------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| 1   | F20 的 25 个 Python 测试、握手 / shutdown 语义、Rust `SidecarStatus` 形状全部不回归                   | `pnpm python:test`、`pnpm rust:test`、`pnpm test`                                               |
| 2   | 新增 Python 契约层全部 TDD（先 RED 后 GREEN），无第三方 import                                        | `python/tests/test_providers.py`、`test_tokenization.py`、`test_rpc_server.py`                  |
| 3   | 新增 Rust DTO 有 JSON serialization contract test，锁定 camelCase 与嵌套形状                          | `src-tauri/src/sidecar/gateway.rs` 内联 `#[cfg(test)]`                                          |
| 4   | 新增 TS normalize 有同形状 payload fixture test + 坏输入安全默认值                                    | `src/lib/ipc.test.ts`                                                                           |
| 5   | `src/features/*` 组件不直接 `invoke()`，渲染路径零 IPC                                                | `ProviderGatewayCard.test.tsx`                                                                  |
| 6   | Python 缺失 / sidecar 未启动 / JSON 坏掉时，`gateway_status` 仍返回完整可解析状态，不升级成 IPC `Err` | Rust 命令测试 + 前端 fixture                                                                    |
| 7   | 全质量门通过并贴真实输出                                                                              | 计划文档「完成门记录」                                                                          |
| 8   | 零新增第三方依赖                                                                                      | `python/pyproject.toml` `dependencies = []` 不变；`package.json` / `Cargo.toml` 不变            |
| 9   | 不把 Prompt / Response / Session 原文发到网络                                                         | 所有新增 RPC 方法不接收 `prompt` / `messages` / `response` 字段；`gateway.route` 只读路由元数据 |

### 1.3 非目标（明确不做）

- **不接入真实 LiteLLM / OpenAI / Anthropic / Google SDK**（硬约束：零新增依赖）。本阶段只做
  契约层与降级路径；`lazy-import` 的预期常态就是「可选 SDK 不存在 → `importable:false`」。
- **不发起任何网络请求**、不读取真实 API Key、不实现 `generate()` / `stream()` 真实调用。
- 不改 SQLite schema，不改 F20 进程管理 / 握手超时 / PTY 隔离策略。
- 不在 `src/features/*` 里新增 `invoke()`。
- 不实现 MCP Runtime connection（F22）、packaged Python 资源定位（F23）。

---

## 2. 架构与依赖方向

```
TypeScript (src/lib/ipc.ts)          —— normalizeGatewayStatus / gatewayStatus()
        │  Tauri IPC: gateway_status
        ▼
Rust Control Plane (src-tauri)
  commands.rs        gateway_status()  薄命令：Ok(GatewayStatus)，失败收敛为 degraded
  sidecar/gateway.rs GatewayStatus DTO + decode_gateway_status(&Value) 安全 decoder
  sidecar/manager.rs SidecarManager::call()  （F20 已存在，不修改）
  sidecar/process.rs initialize 握手      （F20 已存在，不修改）
        │  stdio JSON-RPC 2.0 (JSON Lines)
        ▼
Python sidecar (python/harness_hub_runtime)
  rpc/server.py      DEFAULT_METHODS / SUPPORTED_CAPABILITIES / gateway.status
  providers/         DTO / normalize_stream_event / normalize_error / route / lazy probe
  tokenization/      TRUTH_PRECEDENCE / estimate / local_exact / revision binding
```

依赖方向单向向下：`rpc/server.py` 可 lazy-import `providers` / `tokenization`；
`providers` **不得** import `rpc`；`tokenization` 不得 import `providers` 或 `rpc`。
`sidecar` 域模块不得依赖 Tauri GUI 类型（沿用 F20 规则）。

### 2.1 ADR 对齐

- **ADR-0003 local-first**：所有新增方法只处理「元数据 / 数字 / 枚举」，
  `gateway.route` 入参明确排除 `prompt` / `messages` / `response` 字段；估计值必须带
  `estimated` 标记。
- **ADR-0005 capability vs readiness**：`SUPPORTED_CAPABILITIES` / `DEFAULT_METHODS` 是
  **静态声明**（「能力已实现」，须有测试证据）；`gateway.status.providers[].importable` 是
  **动态就绪度**（「可选 SDK 当前能否导入」）。**运行时缺 SDK 绝不改写 capabilities**。
  这条区分是避免各切片互相依赖的关键：静态声明在任意 worktree 都一致。
- **ADR-0007 LiteLLM First / Official SDK Escape Hatch**：`mode` 只有 `unified` / `native`；
  unified 不吞掉 provider-specific 能力，`gateway.route` 遇到 `nativeOnly` 能力必须返回 native。
- **ADR-0008 Token Truth Precedence (R10)**：四级顺序固定，见 §5。
- **ADR-0006**：sidecar 任何失败不得连累主 Harness 功能；`gateway_status` 永不返回 IPC `Err`。

---

## 3. 线协议（Python ↔ Rust，JSON-RPC 2.0 / JSON Lines）

### 3.1 F20 契约（保留，不得回归）

- `PROTOCOL_VERSION = "1.0"`，只比较 `.` 前主版本段。
- `initialize` 入参 `{hostVersion, protocolVersion, requiredCapabilities}`，
  出参 `{protocolVersion, runtime, capabilities, methods}`。
- `runtime.info` / `initialize.runtime` **必须由同一个构造函数产出**，8 键：
  `name, version, transport, python, protocolVersion, providers, capabilities, methods`。
- Rust 握手 `requiredCapabilities` 仍是 F20 三项 `["ping","runtime.info","shutdown"]`
  （`src-tauri/src/sidecar/config.rs::REQUIRED_CAPABILITIES` **不改**），
  这样 F20 握手与集成测试零回归。
- `shutdown` 请求返回 `{"stopping": true}` 并退出 serve 循环；通知不回包但同样退出。
- 错误码：`PARSE_ERROR=-32700`、`INVALID_REQUEST=-32600`、`METHOD_NOT_FOUND=-32601`、
  `INTERNAL_ERROR=-32603`。

### 3.2 F21 静态声明（冻结值）

`SUPPORTED_CAPABILITIES` 的最终精确列表（顺序有意义，前端按序展示）：

```json
[
  "ping",
  "runtime.info",
  "shutdown",
  "gateway.status",
  "gateway.routing",
  "gateway.streaming",
  "gateway.errors",
  "tokenization.precedence",
  "tokenization.estimate",
  "tokenization.revision"
]
```

`DEFAULT_METHODS` 的最终精确列表（顺序有意义）：

```json
[
  "ping",
  "runtime.info",
  "initialize",
  "shutdown",
  "gateway.status",
  "gateway.providers",
  "gateway.route",
  "gateway.normalize.stream",
  "gateway.normalize.error",
  "tokenization.estimate",
  "tokenization.precedence",
  "tokenization.revision"
]
```

> `runtime.info.providers` 保持 `[]`（F21 仍不宣称已接入真实 provider）；
> 可用 provider 描述符由 `gateway.providers` / `gateway.status` 动态给出。

### 3.3 `gateway.status`（Rust 命令调用它）

请求：`{"jsonrpc":"2.0","id":1,"method":"gateway.status","params":{}}`
**`params` 恒为 `{}`** —— 绝不携带任何 prompt / response 原文。

响应 `result`（冻结形状，camelCase）：

```json
{
  "state": "ready",
  "mode": "unified",
  "protocolVersion": "1.0",
  "providers": [
    {
      "id": "litellm",
      "mode": "unified",
      "importable": false,
      "detail": "optional SDK not installed"
    },
    {
      "id": "openai",
      "mode": "native",
      "importable": false,
      "detail": "optional SDK not installed"
    },
    {
      "id": "anthropic",
      "mode": "native",
      "importable": false,
      "detail": "optional SDK not installed"
    },
    {
      "id": "google",
      "mode": "native",
      "importable": false,
      "detail": "optional SDK not installed"
    }
  ],
  "tokenization": {
    "precedence": ["provider_reported", "provider_count_api", "local_exact", "estimated"],
    "levels": ["local_exact", "estimated"],
    "revisionBinding": true
  },
  "capabilities": ["ping", "...(§3.2 全量)"],
  "methods": ["ping", "...(§3.2 全量)"],
  "lastError": null
}
```

- `state`：`"ready"` = 契约层可运行；`"degraded"` = 核心契约模块导入失败（真实故障）。
  **可选 SDK 缺失不算 degraded**（这是 ADR-0005 的 readiness，不是 capability）。
- `mode`：当前默认路由模式，固定 `"unified"`（ADR-0007）。
- `providers[].importable`：由 `importlib.util.find_spec` lazy 探测，禁止顶层 import 第三方 SDK。
- `providers[].detail`：短 ASCII/中文说明，**不得包含任何凭据或用户文本**。
- `tokenization.precedence` / `levels` / `revisionBinding` 必须与 §5 一致。
- `capabilities` / `methods` 必须与 §3.2 静态声明逐项相等。

### 3.4 `gateway.providers`

请求 `params: {}`；响应 `result` = §3.3 的 `providers` 数组（同样 4 个描述符、同样字段）。

### 3.5 `gateway.route`（纯路由决策，无网络、无原文）

请求 `params`（**白名单字段**，多余字段忽略）：

```json
{ "provider": "openai", "requestedMode": "unified", "capability": "responses.streaming" }
```

响应 `result`：

```json
{
  "mode": "native",
  "provider": "openai",
  "reason": "native_only_capability",
  "fallback": "litellm"
}
```

路由规则（冻结，按序首个命中者胜）：

1. `capability` 属于 `NATIVE_ONLY_CAPABILITIES` → `mode="native"`, `reason="native_only_capability"`。
2. `requestedMode == "native"` → `mode="native"`, `reason="explicit_native"`。
3. `provider` 不在 `PROVIDER_IDS`（`["litellm","openai","anthropic","google"]`）→
   `mode="unified"`, `reason="unknown_provider"`。
4. 其余 → `mode="unified"`, `reason="litellm_first"`。

**`fallback` 字段恒为字符串 `"litellm"`，所有四个分支都有**（表示「原生逃生舱随时可退回
统一通道」）；它不是 nullable，避免前端分支。示例里的 `fallback:"litellm"` 即此恒定值。
`provider` 字段原样回显入参（未知 provider 也原样回显，便于排查）。

`NATIVE_ONLY_CAPABILITIES`（冻结）：
`["responses.streaming", "anthropic.thinking", "google.grounding", "provider.raw_request"]`

> 安全闸：`gateway.route` 若收到 `prompt` / `messages` / `response` / `content` 任一键，
> 返回 `RpcError(INVALID_REQUEST, "route 不接受原文内容")` —— 防止调用方误把原文送进 sidecar。

### 3.6 `gateway.normalize.stream` / `gateway.normalize.error`

`gateway.normalize.stream` 入参 `{"provider":"openai","event":{...}}`，出参统一 `ModelEvent`：

```json
{ "type": "delta", "textDelta": "hi", "finishReason": null, "usage": null, "rawProvider": "openai" }
```

`type` 枚举（冻结）：`"start" | "delta" | "usage" | "done" | "error" | "unknown"`。
`gateway.normalize.error` 入参 `{"provider":"openai","error":{...}}`，出参统一 `ProviderError`：

```json
{
  "kind": "rate_limit",
  "retryable": true,
  "message": "rate limited",
  "provider": "openai",
  "httpStatus": 429
}
```

`kind` 枚举（冻结）：
`"auth" | "rate_limit" | "invalid_request" | "context_length" | "server" | "timeout" | "network" | "unknown"`。

**归一化函数不得回传原始 `event` / `error` 对象**（避免原文泄漏）；`message` 只取短摘要，
且必须脱敏（不 echoes 输入里的长文本）。详细映射表见 §4.2 / §4.3。

### 3.7 `tokenization.*` 三个方法

- `tokenization.precedence` → `{"precedence": [...§5], "levels": ["local_exact","estimated"], "revisionBinding": true}`
- `tokenization.revision` 入参 `{"model":"gpt-4o","tokenizerModel":"..."}`，出参见 §5.3。
- `tokenization.estimate` 入参 `{"textLength": 120, "model": "gpt-4o", "reportedUsage": null, "countApiUsage": null}`，
  出参见 §5.2。**只接收长度，不接收文本原文**。

---

## 4. Python 契约层

### 4.1 `providers` 包（`python/harness_hub_runtime/providers/`）

模块与导出（冻结 API，`__init__.py` 必须 re-export）：

| 模块             | 导出                                                                                                                                                          | 说明                                                          |
| ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| `dto.py`         | `PROVIDER_IDS`, `MODES`, `MODEL_EVENT_TYPES`, `ERROR_KINDS`, `NATIVE_ONLY_CAPABILITIES`, `ProviderDescriptor`, `ModelEvent`, `ProviderError`, `RouteDecision` | 纯数据类（`dataclass(frozen=True)`）+ `to_json()`；全部零依赖 |
| `streaming.py`   | `normalize_stream_event(provider: str, event: dict) -> ModelEvent`                                                                                            | 见 §4.2                                                       |
| `errors.py`      | `normalize_error(provider: str, error: dict) -> ProviderError`                                                                                                | 见 §4.3                                                       |
| `routing.py`     | `route(provider: str, requested_mode: str, capability: str \| None) -> RouteDecision`                                                                         | 见 §3.5                                                       |
| `lazy_import.py` | `probe(module: str) -> bool`, `probe_all() -> list[ProviderDescriptor]`                                                                                       | `importlib.util.find_spec`，捕获所有异常返回 `False`          |

`ProviderDescriptor.to_json()` 冻结键序：`{"id","mode","importable","detail"}`。
`ModelEvent.to_json()` 冻结键：`{"type","textDelta","finishReason","usage","rawProvider"}`。
`ProviderError.to_json()` 冻结键：`{"kind","retryable","message","provider","httpStatus"}`。
`RouteDecision.to_json()` 冻结键：`{"mode","provider","reason","fallback"}`。

`probe()` 的**可选 SDK 模块名**（冻结，全部是第三方，预期不存在）：
`litellm`, `openai`, `anthropic`, `google.generativeai`。
`lazy_import.py` 禁止在模块顶层 `import` 它们。

### 4.2 streaming event 归一化映射（冻结）

| provider  | 输入特征                                              | `type`    | 备注                                                   |
| --------- | ----------------------------------------------------- | --------- | ------------------------------------------------------ |
| any       | `{"type":"..."}` 中含 `message_start` / `start`       | `start`   |                                                        |
| openai    | `choices[0].delta.content` 存在                       | `delta`   | `textDelta` 取该值                                     |
| anthropic | `{"type":"content_block_delta","delta":{"text":...}}` | `delta`   |                                                        |
| google    | `candidates[0].content.parts[0].text`                 | `delta`   |                                                        |
| any       | `usage` / `usageMetadata` 存在                        | `usage`   | `usage` 字段只保留 `inputTokens` / `outputTokens` 整数 |
| openai    | `choices[0].finish_reason` 非空                       | `done`    | `finishReason` 原样短字符串                            |
| anthropic | `{"type":"message_stop"}`                             | `done`    |                                                        |
| any       | `error` 键存在                                        | `error`   |                                                        |
| 其他      | —                                                     | `unknown` | 所有字段取安全默认                                     |

`textDelta` 必须做 `MAX_TEXT_DELTA = 256` 截断；`finishReason` 限 `MAX_SHORT = 64`。
`usage` 只允许整数，缺失/非法 → `null`。

### 4.3 provider error 归一化映射（冻结）

| 输入特征                                                      | `kind`            | `retryable` |
| ------------------------------------------------------------- | ----------------- | ----------- |
| httpStatus / status ∈ {401,403} 或 `type` 含 `authentication` | `auth`            | `false`     |
| 429 或含 `rate_limit`                                         | `rate_limit`      | `true`      |
| 400 / `invalid_request`                                       | `invalid_request` | `false`     |
| 含 `context_length` / `max_tokens` / `too_long`               | `context_length`  | `false`     |
| `timeout` / `timed_out`                                       | `timeout`         | `true`      |
| `network` / `connection` / `ECONN`                            | `network`         | `true`      |
| 5xx 或 `server` / `overloaded`                                | `server`          | `true`      |
| 其他                                                          | `unknown`         | `false`     |

优先级从上到下。`message` 只取 `error.message` / `error.error.message` 的首 120 字符，
并移除换行；**绝不回显完整输入对象**。`httpStatus` 只接受 `int` 且 100..599，否则 `null`。

### 4.4 `tokenization` 包（`python/harness_hub_runtime/tokenization/`）

| 模块          | 导出                                                                                                                        | 说明                   |
| ------------- | --------------------------------------------------------------------------------------------------------------------------- | ---------------------- |
| `truth.py`    | `TRUTH_PRECEDENCE`, `LOCAL_LEVELS`, `truth_rank(level) -> int`, `select_truth(candidates: dict) -> tuple[str, int \| None]` | 见 §5.1                |
| `estimate.py` | `estimate(text_length: int, model: str) -> TokenEstimate`                                                                   | **heuristic**，见 §5.2 |
| `revision.py` | `TOKENIZER_REVISIONS`, `bind_revision(model, tokenizer_model=None) -> RevisionBinding`                                      | 见 §5.3                |

`TokenEstimate.to_json()` 冻结键：
`{"inputTokens","outputTokens","totalTokens","truthLevel","tokenizerModel","revision","estimated"}`。
`RevisionBinding.to_json()` 冻结键：`{"model","tokenizerModel","revision","bound","reason"}`。

---

## 5. Token Truth 四级可信度（ADR-0008 / R10）

### 5.1 顺序（冻结，索引越小越可信）

```text
0 provider_reported      Provider 在响应里直接回报 usage
1 provider_count_api     Provider 的 count / tokenize API 回报
2 local_exact            本地与模型匹配的精确 tokenizer
3 estimated              本地启发式估算
```

`TRUTH_PRECEDENCE = ("provider_reported","provider_count_api","local_exact","estimated")`。
`LOCAL_LEVELS = ("local_exact","estimated")`（F21 零依赖阶段**只能本地产出**这两级；
前两级只作为「更高优先级候选」参与选择，不伪造）。

`select_truth(candidates)`：`candidates` 形如
`{"provider_reported": 12, "provider_count_api": None, "local_exact": 11, "estimated": 13}`，
按 `TRUTH_PRECEDENCE` 从高到低取第一个 `int` 且 `>= 0` 的值，返回 `(level, value)`；
全无 → `("estimated", None)`。`truth_rank` 给未知 level 返回 `len(TRUTH_PRECEDENCE)`（最低）。

> **禁止把 `estimated` 伪装成 `provider_reported`**（ADR-0003：估计必须标记）。

### 5.2 `estimate`（heuristic，明确标注 estimated）

`estimate(text_length, model)`：

- `text_length` 必须是 `int >= 0`，否则抛 `ValueError`。
- 估算公式冻结：`max(1, ceil(text_length / 4))`（4 字符/token 的粗估）。
- 返回 `TokenEstimate(inputTokens=<n>, outputTokens=0, totalTokens=<n>,
truthLevel="estimated", tokenizerModel=<bind_revision(model).tokenizerModel>,
revision=<bind_revision(model).revision>, estimated=True)`。

`tokenization.estimate` RPC 方法按下列优先级组合（**体现四级 precedence**）：

1. 若 `reportedUsage` 是 `{"inputTokens":int,"outputTokens":int}` → `truthLevel="provider_reported"`。
2. 否则若 `countApiUsage` 同上 → `truthLevel="provider_count_api"`。
3. 否则若 `model` 在 `LOCAL_LEVELS` 可精确匹配（F21 恒为 false，因零依赖）→ `local_exact`。
4. 否则 → `estimated`（调用 `estimate()`）。

`local_exact` 在 F21 只作为**契约占位与测试路径**存在：测试须验证
`select_truth({"local_exact": 7, "estimated": 9}) == ("local_exact", 7)`，
证明 precedence 逻辑正确，而不是声称已有精确 tokenizer。

### 5.3 tokenizer-model revision 绑定（冻结）

`TOKENIZER_REVISIONS`（冻结表）：

| model 前缀                     | tokenizerModel               | revision                    |
| ------------------------------ | ---------------------------- | --------------------------- |
| `gpt-4o` / `gpt-4` / `gpt-3.5` | `o200k_base` / `cl100k_base` | `2024-05-13` / `2023-06-13` |
| `claude-`                      | `anthropic-claude`           | `2024-06-20`                |
| `gemini-`                      | `sentencepiece-gemini`       | `2024-09-24`                |
| 其他                           | `unknown`                    | `unknown`                   |

`bind_revision(model, tokenizer_model=None)`：

- 命中已知前缀 → `{"model":model, "tokenizerModel":..., "revision":...,
"bound":true, "reason":"known_model"}`。
- 未命中且显式给了 `tokenizer_model` → `bound=false, reason="explicit_unverified"`，
  `revision="unknown"`（**不假装绑定成功**）。
- 未命中且未给 → `bound=false, reason="unknown_model"`, `revision="unknown"`。

> revision 绑定的意义：同一段文本在不同 tokenizer revision 下 token 数不同，
> 所以任何 token 数字都必须能追到 `(tokenizerModel, revision)`；绑定不上就明确标 `unknown`，
> 不允许用「看起来对」的默认值蒙混。

---

## 6. Rust 契约（`src-tauri/src/sidecar/gateway.rs`）

### 6.1 DTO（全部 `#[serde(rename_all = "camelCase")]`）

```text
enum GatewayState { Ready, Degraded, Unavailable }        // "ready"|"degraded"|"unavailable"
struct GatewayProvider { id:String, mode:String, importable:bool, detail:String }
struct GatewayTokenization { precedence:Vec<String>, levels:Vec<String>, revision_binding:bool }
struct GatewayStatus {
    state: GatewayState,
    sidecar_state: SidecarState,       // 复用 F20 枚举，值 "stopped"|"ready"|"degraded"
    mode: String,
    providers: Vec<GatewayProvider>,
    tokenization: GatewayTokenization,
    capabilities: Vec<String>,
    protocol_version: Option<String>,
    last_error: Option<String>,
}
```

序列化后顶层键（**锁定，8 键**）：
`state, sidecarState, mode, providers, tokenization, capabilities, protocolVersion, lastError`
嵌套：`providers[]` = `id, mode, importable, detail`；
`tokenization` = `precedence, levels, revisionBinding`。
不得泄漏任何 snake_case 顶层/嵌套键。

`GatewayStatus::unavailable(sidecar_state, reason)` 与
`GatewayStatus::degraded(sidecar_state, reason)` 两个构造器必须产出完整可解析对象
（`providers`/`capabilities` 用**静态声明常量**填充，不用空数组，保证 UI 有东西可显示）。

### 6.2 decoder（安全默认值）

`pub fn decode_gateway_status(value: &Value, sidecar_state: SidecarState) -> GatewayStatus`：

- `value` 非对象 / `state` 非已知枚举 → `GatewayStatus::degraded(sidecar_state, "gateway.status 应答形状非法")`。
- 每个字段用 `as_str` / `as_bool` / `as_array` + 逐项类型检查；坏类型取安全默认，不 panic。
- 字符串字段统一 `MAX_FIELD_LEN = 256` 截断；数组最多 `MAX_ITEMS = 64` 项。
- `providers` 里未知 `mode` → `"unknown"`；`capabilities` 只保留 `String`。
- **`protocol_version` 必须来自 payload；缺失 → `None`**。
- decoder 不得把 payload 里的任意长文本原样塞进 `last_error`（同样截断）。

### 6.3 薄命令

`src-tauri/src/commands.rs` 新增：

```text
#[tauri::command] pub fn gateway_status(state: State<'_, AppState>) -> Result<GatewayStatus>
```

行为（冻结，验收 #6）：

1. `let sidecar = state.sidecar.status();`（纯读，不触发冷启动，渲染路径零副作用）
2. 若 `sidecar.state != SidecarState::Ready` → `Ok(GatewayStatus::unavailable(sidecar.state,
"Python sidecar 未就绪，Provider Gateway 状态不可用"))`
3. 否则 `state.sidecar.call("gateway.status", &json!({}))`：
   - `Ok(value)` → `Ok(decode_gateway_status(&value, sidecar.state))`
   - `Err(msg)` → `Ok(GatewayStatus::degraded(SidecarState::Degraded, msg))`
4. **永不 `Err(...)`**，永不传 prompt 类参数。

`src-tauri/src/lib.rs::run` 的 `generate_handler![...]` 按字母序插入
`commands::gateway_status,`（在 `get_usage_sources` 与 `kill_terminal` 之间）。

### 6.4 静态声明常量

`gateway.rs` 内定义 `pub const F21_CAPABILITIES: &[&str]` 与
`pub const F21_METHODS: &[&str]`，值与 §3.2 逐项一致，供降级构造器与契约测试共用。
`src-tauri/src/sidecar/config.rs::REQUIRED_CAPABILITIES` **不改**（保 F20 握手）。
`src-tauri/src/sidecar/mod.rs` 增加 `pub mod gateway;`。

---

## 7. 前端契约（`src/lib/ipc.ts` / `src/features/settings/ProviderGatewayCard.tsx`）

### 7.1 类型与 normalize

```text
type GatewayState = 'ready' | 'degraded' | 'unavailable'
interface GatewayProvider { id, mode, importable, detail }
interface GatewayTokenization { precedence, levels, revisionBinding }
interface GatewayStatus { state, sidecarState, mode, providers, tokenization,
                          capabilities, protocolVersion, lastError }
```

`export function normalizeGatewayStatus(raw: unknown): GatewayStatus`：

- 非对象 / `state` 非三枚举之一 → `unavailableGatewayStatus()`（fail-closed），
  其中 `sidecarState='stopped'`，`capabilities=F21_CAPABILITIES`（静态常量），
  `providers=[]`，`tokenization` 用 §5 冻结默认值，`protocolVersion=null`，
  `lastError='网关状态不可用'`。
- 逐字段安全默认：字符串不存在/非 string → `''`（`protocolVersion`/`lastError` → `null`）；
  数组非 array → `[]` 且只保留合法元素；`importable` 非 bool → `false`；
  `revisionBinding` 非 bool → `false`；`sidecarState` 非三值 → `'stopped'`。
- `export function gatewayStatus(): Promise<IpcResult<GatewayStatus>>`，
  内部 `invokeCommand<unknown>('gateway_status')` 后 normalize（**只走 `src/lib/ipc.ts`**）。

### 7.2 fixture test（`src/lib/ipc.test.ts`）

必须包含：

1. **同形状 payload**：完整 `GatewayStatus` JSON（键名与 §6.1 一致）→ 逐字段断言。
2. **坏输入安全默认值**：`null` / `[]` / `{state:'weird'}` / 字段类型全错 → 断言 fail-closed。
3. **命令名断言**：`gatewayStatus()` → `invokeSpy.mock.calls[0]?.[0] === 'gateway_status'`
   （`invoke` 有 3 个参数，**不要断言完整参数数组**）。
4. `Err` 路径：`IpcResult{ok:false}` 不透传坏数据。

### 7.3 `ProviderGatewayCard`（`src/features/settings/`）

- 文件：`ProviderGatewayCard.tsx` + `ProviderGatewayCard.test.tsx`（新文件，禁止改
  `SettingsPage.tsx` 之外的既有测试）。
- 从 `@/lib/ipc` 导入 `gatewayStatus` 与类型；**禁止 `invoke()`**。
- `data-testid="gateway-card"`，含「刷新」按钮（点一次 → `gateway_status`）。
- 展示：`state`、`mode`、`sidecarState`、`capabilities` 数量、`providers` 列表
  （`importable` 用「可用/未安装」文案）、tokenization precedence、`lastError`。
- `degraded` / `unavailable` 显示警示文案「Provider Gateway 不可用，主 Harness 功能不受影响」。
- 渲染路径零 IPC（IPC 只在点击后发生），测试用 `stubCommands` 断言。
- `SettingsPage.tsx` 在 `<SidecarCard />` 之后渲染 `<ProviderGatewayCard />`。

---

## 8. 测试纪律

- **TDD**：每个切片先提交 `test:` 的 RED 测试（须真实观察到失败），再提交 `feat:` 最小实现，
  最后必要时 `refactor:`。RED 证据写进 PR 描述。
- **跨 IPC 契约测试**：Rust DTO 序列化形状（§6.1 全键 + 无 snake_case 泄漏）、
  TS normalize fixture（§7.2）、Python `runtime.info` / `initialize` / `gateway.status` 形状
  三者必须锁同一份 payload。
- **冷启动真路径**：`gateway_status` 在 sidecar `Stopped` 时走真实
  `SidecarManager::status()` + `GatewayStatus::unavailable(...)`，不允许夹具替生产代码补状态。
- **安全默认值**：所有 decoder / normalize 对坏输入 fail-closed，不抛异常、不 panic。
- **零依赖验证**：`python/pyproject.toml` 的 `dependencies = []` 不变；
  新增 Python 模块只允许 `from __future__`, `dataclasses`, `importlib.util`, `math`, `typing`, `json`。
- **local-first 验证**：新增 RPC 方法测试须覆盖 §3.5 的原文拒绝闸与 §4.2/§4.3 的
  「不回显原始对象」。

---

## 9. 提交 / PR 切分（写作用域互斥）

| 切片 | 分支                        | 写作用域（独占）                                                                                                                                  | 产出                      |
| ---- | --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------- |
| S0   | `feat/f21-provider-gateway` | `docs/specs/2026-11-05-f21-*`, `docs/plans/2026-11-05-f21-*`                                                                                      | 冻结契约（本文件 + 计划） |
| S1   | `f21/py-providers`          | `python/harness_hub_runtime/providers/**`, `python/tests/test_providers.py`                                                                       | providers 契约层          |
| S2   | `f21/py-tokenization`       | `python/harness_hub_runtime/tokenization/**`, `python/tests/test_tokenization.py`                                                                 | tokenization 契约层       |
| S3   | `f21/py-rpc`                | `python/harness_hub_runtime/rpc/server.py`, `python/tests/test_rpc_server.py`                                                                     | RPC 方法 + 能力协商       |
| S4   | `f21/rust`                  | `src-tauri/src/sidecar/gateway.rs`, `.../mod.rs`, `.../config.rs`, `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs`                            | Rust DTO + decoder + 命令 |
| S5   | `f21/web`                   | `src/lib/ipc.ts`, `src/lib/ipc.test.ts`, `src/features/settings/ProviderGatewayCard.tsx`, `...test.tsx`, `src/features/settings/SettingsPage.tsx` | 前端 normalize + 卡片     |

每个切片开独立 PR，base = `feat/f21-provider-gateway`，合并后进入集成分支；
S1–S5 作用域两两不交，可并行。S6（质量门 + 最终 PR → `main`）由 Lead 串行执行。

---

## 10. 已知风险与对策

| 风险                                                 | 对策                                                                                                                                                                 |
| ---------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| SSH git 通道被 `198.18.0.125:443` 拦截               | 一律用 `gh` + HTTPS；`gh auth setup-git` 已配置 credential helper                                                                                                    |
| commit 被「吞掉」                                    | 每次 commit 后 `git log -1 --oneline` 复核                                                                                                                           |
| 本地 `main` 标签落后真实 `origin/main` 83 个提交     | **以 `6d6c327`（真实远端 main / 当前 HEAD）为基线**，不拿本地 `main` 做 diff 基线                                                                                    |
| 各 worktree 缺少 `node_modules` / `target` / `.venv` | Python 用 `python -m unittest discover -s python/tests -t python -v` 直接跑；web worktree 建 `node_modules` junction；rust worktree 设 `CARGO_TARGET_DIR` 指向主检出 |
| S1/S2 尚未合入时 S3 的 lazy-import 探测拿不到适配器  | §2.1 ADR-0005：静态声明与动态就绪度分离；`gateway.status` 的核心模块探测只看自身，可选 SDK 缺失是预期状态，S3 测试断言形状 + 降级路径而非「必须 importable」         |
| Windows CI 上 PTY/进程树测试偶发 flake               | F20 已记录为既有 flake；若复现，单独标注并复跑，不算 F21 回归                                                                                                        |
| `build-release-040/` 未跟踪产物                      | **绝不提交**（既有遗留）                                                                                                                                             |
