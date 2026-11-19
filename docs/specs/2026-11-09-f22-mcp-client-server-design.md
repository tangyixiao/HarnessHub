# F22 — MCP Client / Server 设计（v0.5.0）

> 本文件是 F22 跨语言契约的**唯一事实来源**。实现与测试与本文冲突时，先改本文再改代码。
> 规格源：`HARNESS_HUB_PROJECT_PLAN.md` §4.6 / §8.3 / ADR-0009；ROADMAP 阶段 22。

## 0. 目标与非目标

**目标**（§8.3 验收清单逐项覆盖）：

1. MCP Client registry —— sidecar 内进程级连接注册表，状态机可见。
2. stdio transport —— spawn 外部 MCP server 子进程，行分隔 JSON-RPC。
3. Streamable HTTP transport —— urllib POST + SSE 响应解析，`Mcp-Session-Id` 头维持。
4. Tool discovery —— `tools/list` 归一为 DTO。
5. MCP permission policy —— Rust 侧最小策略：默认只放行只读启发式工具。
6. Harness Hub 作为 MCP Server —— Rust headless stdio 模式，只读工具先行。

**非目标**：

- 完整 Permission Engine（G25）：无 allow/deny 审计落库、无用户可配置策略。
  `permission_decisions` 表 decision CHECK 只有 `'observed'`（迁移 0008 文件头明确
  扩展取值必须新增迁移重建表），F22 **不复用也不改这张表**；F22 的显式授权路径 =
  前端 confirm + 每次 tool call 独立 IPC 发起，审计留给 G25。
- Resources / Prompts / sampling / roots 能力（只做 tools）。
- 把 MCP 做成内部模块总线（ADR-0009：MCP 是标准边界）。
- 第三方 MCP SDK 依赖：Python 侧纯 stdlib 实现（与 F21 纪律一致，`mcp` SDK 保持
  optional，`models.status` 类似探测展示但不在本阶段使用）。

## 1. 架构归属

```text
用户/前端 ──IPC──> Rust（配置 + 策略 + DTO）──stdio JSON-RPC──> Python sidecar
                                                                  │ mcp/ 包
                                                                  ├─ stdio transport ──> 外部 MCP server 子进程
                                                                  └─ http transport ───> 外部 MCP server (SSE)

第三方 MCP client ──stdio JSON-RPC──> harness-hub.exe --mcp-serve ──> Rust 只读工具（&Connection）
```

- **Python 拥有连接与传输**：spawn 子进程、HTTP 请求、协议协商、连接状态机。
- **Rust 拥有配置与策略**：server 定义来自 E18 `mcp_servers` 表（不新增 schema），
  tool call 策略判定在 Rust，Python 只执行不放行。
- **Hub MCP Server 在 Rust**：只读工具要查 SQLite，Python 禁碰数据库（CONTEXT 层依赖
  红线），因此 server 方向必须落在 Rust。零新增 Rust 依赖：行分隔 JSON-RPC 复用
  `plugins::rpc` codec，MCP stdio 传输本身就是 newline-delimited JSON。

## 2. 安全红线（实现前先读）

1. **env 值单次传递**：stdio server 启动需要真实环境变量值。v1 的值来源 =
   **sidecar 子进程继承宿主进程环境**（`Popen` 默认行为），Rust 不在 IPC 里传任何
   env 值。E16 凭据是 harness 维度、与 MCP server 无链接（迁移 0012 无此 FK），
   「MCP env 键 → 凭据句柄」机制留 Open Questions（§8）。快照只回 `envKeys` /
   `envMissingKeys` 名单，绝不回值；任何 env 值不落 SQLite、不写日志。
2. **命令不经 shell**：Python spawn 用参数数组直 exec（`subprocess.Popen(list)`），
   与 `sidecar/process.rs` 同纪律。
3. **策略唯一判定点在 Rust**：`mcp.tools.call` 只有 Rust 策略返回 allow 才转发；
   Python 收到的 params 里带 `policyDecision: "readonly"` 字段，值不是它就拒绝
   （双保险，防未来其他调用方绕过）。
4. **Hub server 只读**：`--mcp-serve` 模式只拿 `&Connection`，tools/call 白名单内
   函数全部是现有只读投影；不提供任何写路径，不提供 `--mcp-serve` 之外的隐藏模式。
5. **原文红线对齐**：新增 RPC 方法一律过 `_reject_raw_content`（prompt/messages/
   response/content 四键拒绝），与 F21 一致。
6. **网络面显式**：http transport 只连 `mcp_servers.url` 登记的地址；Hub server
   只监听 stdio，绝不开端口。

## 3. 跨语言契约（冻结）

### 3.1 RPC 方法表扩展

`SUPPORTED_CAPABILITIES` 追加（顺序=追加顺序，不重排既有项）：

```text
"mcp.client"
```

`DEFAULT_METHODS` 追加 5 项：

```text
"mcp.status"      "mcp.connect"      "mcp.disconnect"      "mcp.tools.list"      "mcp.tools.call"
```

`GATEWAY_STATIC_*` 两份常量**不动**（gateway 块的自包含语义不变）。

### 3.2 mcp.status

参数 `{}`。返回：

```json
{
  "servers": [
    {
      "name": "weather",
      "transport": "stdio",
      "state": "ready",
      "protocolVersion": "2025-06-18",
      "serverInfo": { "name": "weather-server", "version": "1.2.0" },
      "toolCount": 3,
      "lastError": null,
      "envKeys": ["WEATHER_API_KEY"]
    }
  ],
  "protocolVersions": ["2025-06-18", "2025-03-26", "2024-11-05"]
}
```

- `state` ∈ `disconnected | connecting | ready | degraded`（degraded = 连接后进程死亡
  / 握手失败，保留 lastError）。
- 字符串截断纪律：`lastError` ≤ 256 chars；`serverInfo.name/version` ≤ 120。
- `protocolVersions` 冻结为本实现支持的 MCP 规范版本，降序（首选在前）。

### 3.3 mcp.connect

参数（Rust→Python，一次一个连接）：

```json
{
  "name": "weather",
  "transport": "stdio",
  "command": "uvx",
  "args": ["mcp-server-weather"],
  "env": { "WEATHER_API_KEY": "..." },
  "protocolVersion": "2025-06-18"
}
```

http 变体：`{ "name": "remote", "transport": "http", "url": "https://...",
"headers": {}, "protocolVersion": "..." }`（headers 值同 env 值红线：单次传递）。

- `name` 必须匹配 `^[A-Za-z0-9._-]+$`（与 E18 registry 同规则）；重复 connect 同名
  先隐式 disconnect 旧的再连。
- 协商：client 发 `protocolVersion`（缺省取首选）；server 回的版本不在
  `protocolVersions` 支持集 → 连接判 degraded，lastError 写明两版本。
- `initialize` 成功后必须发 `notifications/initialized` 通知（无 id，不回包），
  随后才允许 `tools/list`。
- 返回：该 server 的 `mcp.status` 单元素形状（含 toolCount=0，工具数在 tools.list 后
  刷新）。任何失败收敛为 `state: "degraded"` + `lastError`，**不抛 RPC 错**
  （与 F20 状态收敛纪律一致）；但参数非法（缺 command/url、name 非法）返回
  JSON-RPC `-32602`。

### 3.4 mcp.disconnect

参数 `{ "name": "..." }`。未知名返回 `{ "state": "disconnected", "name": "..." }`
（幂等）。stdio 分支 kill 进程树（Windows: Job Object 由 Python 进程自然继承？否
—— sidecar 的子进程不归 Rust Job Object 管；Python 侧 `Popen.terminate()` → 2s →
`kill()`）。

### 3.5 mcp.tools.list

参数 `{ "name": "..." }`。执行 `tools/list`（分页游标循尽，上限 512 个工具），
归一返回：

```json
[{ "name": "get_forecast", "description": "...", "inputSchemaKeys": ["location"] }]
```

- `description` ≤ 200 chars；`inputSchemaKeys` 只取顶层属性键名（≤32 个），不回显
  完整 schema（防超大 payload；完整 schema 留给未来按需获取）。
- state 非 ready 的 server 返回 JSON-RPC `-32002`（自定义码，message 带 server 名）。

### 3.6 mcp.tools.call

参数：

```json
{
  "name": "weather",
  "tool": "get_forecast",
  "arguments": { "location": "Shanghai" },
  "policyDecision": "readonly"
}
```

- `arguments` 必须是 JSON 对象（缺失视为 `{}`）；`policyDecision` 必须等于
  `"readonly"`（Rust 策略放行的唯一值），否则 Python 侧 `-32602` 拒绝。
- 执行 `tools/call`，超时 30s（Python 侧 socket/进程读超时），超时归 degraded。
- 返回归一（MCP `content` blocks → 摘要，不回显完整原文超 4 KiB）：

```json
{
  "isError": false,
  "contentCount": 1,
  "contentTypes": ["text"],
  "textPreview": "……前 2048 字符……",
  "truncated": true
}
```

- `textPreview` 只拼 `type == "text"` 的 blocks；无 text block 时为 `null`。

### 3.7 Python mcp/ 包模块划分

```text
python/harness_hub_runtime/mcp/
  __init__.py      # 版本常量导出
  dto.py           # frozen dataclass + to_json()（键序冻结，对齐 F21 dto.py 风格）
  protocol.py      # 协议核心：initialize/tools/list/tools/call 的请求构造与响应归一
  transports.py    # StdioTransport（Popen 行分隔）/ HttpTransport（urllib + SSE 行解析）
  registry.py      # 连接注册表状态机（线程安全，connect/disconnect/status）
```

- 传输层接口冻结：`send_request(method, params, timeout) -> dict` /
  `send_notification(method, params)` / `close()`。
- HttpTransport SSE 解析只处理 `data:` 行 + `event:` 行的最小子集（MCP streamable
  HTTP 响应兼容 JSON 单对象与 SSE 两种）；`Mcp-Session-Id` 响应头存住并在后续请求
  回传；HTTP 404/4xx → 一次性重试无 session 头（server 不支持 session 时）。

### 3.8 Rust 侧归属

```text
src-tauri/src/mcp/
  registry.rs sync.rs mod.rs    # E18 既有，不动
  policy.rs                     # evaluate_tool_call(name, tool) -> Decision { allow, reason }
  client_bridge.rs              # 调 SidecarManager::call + 状态缓存；DTO: McpRuntimeStatus
  server/                       # S3：Hub MCP Server（见 §4）
```

IPC 命令（薄命令，错误收敛不返回 IPC Err，对齐 `gateway_status` 模式）：

```text
mcp_runtime_status / mcp_runtime_connect / mcp_runtime_disconnect
mcp_runtime_tools_list / mcp_runtime_tool_call
```

- `mcp_runtime_connect` 输入来自前端（serverId），Rust 从 `mcp_servers` 表读定义。
  env 值不在 IPC 传递（Python 侧子进程继承宿主环境）；返回里带
  `envMissingKeys`（= registry `env_keys` 名单，信息性：提示用户宿主环境需有这些
  变量）。**任何 env 值绝不进入日志/快照/错误文本/IPC。**
- `mcp_runtime_tool_call` 先 `policy.evaluate_tool_call`，deny 时返回结构化
  `{ allowed: false, reason }`（HTTP 200 语义，前端渲染原因），不转发 Python。

**策略 v1 冻结规则**：tool 名小写后按 `_`/`-` 取第一段，属于
`{get, list, search, read, fetch, query, find, describe, info, status, health, stats}`
→ allow（reason `readonly_by_default`）；否则 deny（reason
`not_readonly_by_default`）。空名/非法字符 deny（`invalid_tool_name`）。

### 3.9 DTO 序列化契约测试（AGENTS.md 纪律）

- Rust：`McpRuntimeStatus` / `McpToolCallOutcome` 等 DTO 必须有 serde JSON 契约测试
  （锁定 camelCase 键名与嵌套形状）。
- TS：`normalizeMcpRuntimeStatus` 等必须有 fixture test（同形状 payload + 安全默认）。

## 4. S3：Hub 作为 MCP Server（Rust headless stdio）

### 4.1 启动形态

```text
harness-hub.exe --mcp-serve
```

- `main()` 里 argv 第一参数分流：命中 `--mcp-serve` → `run_mcp_server()`（tokio 不需要
  ——同步 stdio 循环即可），**绝不初始化 Tauri Builder / webview / 托盘**；进程 EOF 或
  `shutdown` 请求退出。
- release 构建的 `windows_subsystem = "windows"` 不影响 stdio 重定向（父进程管道），
  但 `--mcp-serve` 模式下 eprintln 诊断只写 stderr。
- DB 路径：`HARNESS_HUB_DB_PATH` env 显式覆盖（集成测试用）；缺省
  `%APPDATA%/dev.harnesshub.desktop/harness-hub.sqlite3`（Windows）/ Linux
  `$XDG_DATA_HOME/dev.harnesshub.desktop`（缺省 `~/.local/share/...`）/ macOS
  `~/Library/Application Support/dev.harnesshub.desktop`。identifier 常量与
  `tauri.conf.json` 一致性用 `include_str!` 读 conf 的单测锁死。

### 4.2 协议（server 方向）

- 行分隔 JSON-RPC 2.0（MCP stdio 传输官方形态）；复用 `plugins::rpc` 的
  Incoming codec 读入。
- 方法：`initialize` / `notifications/initialized`（忽略）/ `ping` / `tools/list` /
  `tools/call` / `shutdown`（进程退出）。未知方法 `-32601`。
- `initialize` 协商：请求版本 ∈ 支持集原样回；否则回首选版本（客户端自行断）。
  返回 `capabilities: { "tools": {} }` + `serverInfo { name: "harness-hub",
version: CARGO_PKG_VERSION }`。
- `tools/call` 返回 MCP 形状 `{ content: [{ type: "text", text: "<JSON 串>" }], isError }`；
  每个 text block 内是工具结果的 `JSON.stringify`。
- 工具白名单（只读，全部复用现有领域查询，`&Connection`）：

```text
harness_list       （无参）
project_list       （无参）
session_list       { limit?: 1..100, 缺省 50 }
usage_summary      （无参；dashboard 只读投影）
activity_timeline  { limit?: 1..200, 缺省 100 }
```

- 未知名工具：`isError: true` + 文本 `unknown tool: <name>`，**不**崩进程。
- 每个工具实现 = 现有命令层同款查询函数的 `&Connection` 直调；DB 打不开 →
  initialize 回 JSON-RPC error 后退出（不假装可用）。

### 4.3 测试（真机证据模式）

`src-tauri/tests/mcp_server_stdio.rs`：
spawn `env!("CARGO_BIN_EXE_harness-hub") --mcp-serve`（`HARNESS_HUB_DB_PATH` 指向
空 DB 或带 seed 的 temp DB）→ initialize → tools/list 断言五工具 → tools/call
`harness_list` 断言 JSON → EOF 退出。另有一条冷启动真路径用例：空 DB（只有生产
migration 建表）下 initialize + tools/call 不炸。

## 5. S4：前端

- `src/lib/ipc.ts`：`normalizeMcpRuntimeStatus` / `normalizeMcpToolList` /
  `normalizeMcpToolCallOutcome`（fail-closed 安全默认）+ 五个命令包装。
- `src/features/settings/`：`McpRuntimeCard`（server 列表：connect/disconnect、
  工具数、状态；工具调用面板：只读工具名 + JSON 参数输入 + confirm 对话框明示
  「将连接外部 MCP server 并调用其工具」；deny 结果展示 reason）。
  组件只 import `@/lib/ipc`，渲染路径零 IPC。

## 6. 切片 / PR / 合并顺序

| 切片                  | 分支                       | 文件域                                                                                                           | 依赖                               |
| --------------------- | -------------------------- | ---------------------------------------------------------------------------------------------------------------- | ---------------------------------- |
| S1 Python client      | `feat/f22/py-mcp-client`   | `python/harness_hub_runtime/mcp/**`、`rpc/server.py`、`python/tests/test_mcp_*.py`                               | 无                                 |
| S2 Rust bridge+policy | `feat/f22/rust-mcp-bridge` | `src-tauri/src/mcp/{policy,client_bridge}.rs`、`mod.rs`、`commands.rs`、`src-tauri/tests/mcp_bridge.rs`          | 契约按 §3 冻结，代码不依赖 S1 合入 |
| S3 Hub server         | `feat/f22/hub-mcp-server`  | `src-tauri/src/mcp/server/**`、`main.rs`、`lib.rs`（run_mcp_server 导出）、`src-tauri/tests/mcp_server_stdio.rs` | 无文件交集                         |
| S4 前端               | `feat/f22/web`             | `src/lib/ipc.ts`、`src/features/settings/McpRuntimeCard.tsx`、`SettingsPage.tsx`、fixture tests                  | S2 契约冻结                        |

合并顺序：S1 → S2 → S3 / S4（可并行）→ docs。S1/S2 都动「方法表」的两侧：
S2 的 Rust 契约测试只锁**形状**，S1 合入后跑一次 `pnpm python:test` +
`sidecar_lifecycle` 真机回归证明两侧对上。

## 7. 验收门（每切片）

```text
pnpm python:test       # S1
pnpm rust:clippy       # S2/S3
cargo test -p harness-hub（本切片相关 target）
pnpm lint && pnpm typecheck && pnpm test   # S4
```

加 AGENTS.md「完成前」全门在合并前的整合 PR 跑一遍。

## 8. Open Questions（留给 G25+）

1. 用户可配置工具 allowlist / denylist 与每次调用的 ask 模式。
2. tool call 审计落库（新迁移 + allow/deny CHECK 语义）。
3. Resources / Prompts 发现与 UI 消费。
4. OAuth 授权的 http transport（当前 headers 由用户显式登记）。
5. MCP env 键 → E16 凭据句柄的链接机制（v1 继承宿主环境）。
