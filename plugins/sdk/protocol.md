# 插件协议 v1（v0.1 冻结）

本文件是插件作者的唯一契约来源。宿主侧的实现在 `src-tauri/src/plugins/`，**以宿主为准**：
本文件与宿主行为冲突时，宿主行为正确，请提 issue 而不是自己挑一个实现。

> 相关：`docs/adr/0018-plugin-isolation-boundary.md`（隔离边界）、
> `docs/specs/2026-10-16-plugins-host-design.md`（设计规格）。

## 1. 传输层

插件是**独立进程**，通过标准输入输出与宿主通信：

| 方向        | 通道        | 内容                                       |
| ----------- | ----------- | ------------------------------------------ |
| 宿主 → 插件 | 插件 stdin  | 一行一个完整 JSON-RPC 请求                 |
| 插件 → 宿主 | 插件 stdout | 一行一个完整 JSON-RPC 响应或通知           |
| 插件 → 宿主 | 插件 stderr | **只放诊断文本**，宿主收进 64 KiB 环形缓冲 |

规则（宿主 `rpc.rs` 已按此实现并有测试）：

- **一行一个 JSON 对象**，行内不得出现裸换行。序列化时不要 pretty-print。
- 空行与纯空白行被静默跳过（`""`、`"   "`、`"\t"`、`"\r\n"`）。
- 容忍 CRLF 行尾，但**建议只写 `\n`**（见 §7 编码陷阱）。
- `jsonrpc` 字段必须是字符串 `"2.0"`，写 `2.0`（数字）或 `"1.0"` 都会被判为 invalid request。
- `params` 缺省视为 `null`。
- 一条消息不能同时带 `result` 和 `error`；响应必须至少带其中一个。
- `error` 对象必须同时有整数 `code` 和字符串 `message`。

**stdout 是协议通道**：任何调试输出、进度条、`print()` 都会污染它。所有诊断请走 stderr。

宿主在插件结束时（关闭 stdin）先等待一段宽限期，再强制结束整个进程树。

## 2. 握手时序

```text
宿主                                                   插件进程
 |                                                       |
 |-- initialize ---------------------------------------->|
 |   {"jsonrpc":"2.0","id":1,                            |
 |    "method":"initialize",                             |
 |    "params":{"host_version":"0.1.0",                  |
 |              "api_version":"1",                       |
 |              "manifest_capabilities":{"usage":true}}} |
 |                                                       |
 |<---------------------------------------- result -----|
 |   {"jsonrpc":"2.0","id":1,                            |
 |    "result":{"plugin_id":"usage-connect",             |
 |              "plugin_version":"0.1.0",                |
 |              "supported_methods":["usage/import"]}}   |
 |                                                       |
 |<----------------------------- notification -----------|
 |   {"jsonrpc":"2.0","method":"initialized","params":{}}|
 |                                                       |
 |  允许集 = supported_methods ∩ manifest_capabilities    |
 |                                                       |
 |-- usage/import (id=2) ------------------------------->|
 |<---------------------------------------- result -----|
 |   {"jsonrpc":"2.0","id":2,"result":{"session":[...]}} |
```

握手要点：

- 超时 **10 秒**。超时即注册失败，插件被标记为 `degraded`，主程序不受影响。
- 回包里的 `plugin_id` **必须与 manifest 的 `id` 完全一致**，否则握手直接失败
  （宿主报「插件自报 id 与 manifest 不一致」）。不要在这里返回别名的展示名。
- `plugin_version` 可缺省，宿主补 `"0.0.0"`；但请老实填。
- `supported_methods` 缺省为空数组。它是**自报**清单，不是授权——宿主再与 manifest 的
  `[capabilities]` 求交集，交集之外的方法一律不能调用。
- 随后的 `initialized` 通知是**尽力而为**的：宿主会等，但等不到不算失败（因为此时
  已经拿到了 initialize 的 result）。仍然强烈建议发，它便于宿主日志排障。
- v0.1 没有心跳。调用超时 **30 秒**。

### 2.1 `initialize` 参数形状

宿主发给插件的 `params` 恰好三个字段（多一个都没有，也别指望靠它们做版本协商）：

| 字段                    | 类型   | 说明                                                                     |
| ----------------------- | ------ | ------------------------------------------------------------------------ |
| `host_version`          | string | 宿主版本，例如 `"0.1.0"`。**仅供日志/诊断**，不要在插件里按它分支。      |
| `api_version`           | string | 协议版本，v0.1 恒为 `"1"`。与 manifest 的 `api_version` 是同一个概念。   |
| `manifest_capabilities` | object | 宿主从你的 `manifest.toml` 解析出的 `[capabilities]`，8 个键全布尔、无缺省 |

`manifest_capabilities` 总是**完整 8 个键**（缺省的能力填 `false`），不是只出现 `true` 的键：

```json
{
  "detect": false,
  "launch": false,
  "resume": false,
  "usage": true,
  "trace": false,
  "profiles": false,
  "mcp": false,
  "skills": false
}
```

插件回包的 `result` 字段（全部为可选，但强烈建议至少给 `plugin_id` 与 `supported_methods`）：

| 字段               | 必需 | 缺省行为                                        |
| ------------------ | ---- | ----------------------------------------------- |
| `plugin_id`        | 是   | 缺失或与 manifest `id` 不一致 → 握手失败        |
| `plugin_version`   | 否   | 宿主补 `"0.0.0"`                                |
| `supported_methods` | 否  | 宿主当作空数组，此后**任何方法都不能调用**      |

## 3. 方法表

### 3.1 通路方法（v0.1 真正可用）

| 方法           | 需要的 capability | params                     | result                                              |
| -------------- | ----------------- | -------------------------- | --------------------------------------------------- |
| `usage/import` | `usage`           | `{"source":"<plugin-id>"}` | `{"session":[<ccusage session 行>...]}`             |
| `skill/list`   | `skills`          | `{}`                       | `{"skills":[{"skill_id","name","description"}...]}` |
| `skill/get`    | `skills`          | `{"skill_id":"demo"}`      | `{"skill_id","name","description","body"}`          |

### 3.2 已定义但 v0.1 不实现

| 方法族                                  | v0.1 行为                   |
| --------------------------------------- | --------------------------- |
| `harness/*`（detect / launch / resume） | 返回 `-32001 not_supported` |
| `profile/*`                             | 返回 `-32001 not_supported` |
| `trace/*`                               | 返回 `-32001 not_supported` |
| `runtime/*`                             | 返回 `-32001 not_supported` |

插件不需要为这些方法写任何代码。宿主不会把没声明的方法发给插件；即使收到，
直接回 `-32001` 或 `-32601` 都是合理的，示例插件回 `-32001` 并带上方法名。

## 4. usage/import 的数据形状

`result.session` 是 **ccusage unified session 报告里 `session` 数组的等价物**。宿主把它
包成 `{"session":[...]}` 后**直接喂给现有的 ccusage 解析器**（`usage::parse_report` →
`normalize_session`），不会为插件单独写一套解析。

因此你产出的每一行必须满足以下约束，否则宿主会带明确错误拒收：

| 字段                                                             | 类型             | 说明                                                                                                             |
| ---------------------------------------------------------------- | ---------------- | ---------------------------------------------------------------------------------------------------------------- |
| `agent`                                                          | string，**必需** | 决定落到哪个 harness。缺失 → 「session 行缺少 agent」                                                            |
| `period`                                                         | string，**必需** | 会话身份（Codex 是 rollout 路径片段，也可以直接放 UUID）。缺失 → 「缺少 period」                                 |
| `inputTokens`                                                    | number           | 四类 token 之一                                                                                                  |
| `outputTokens`                                                   | number           |                                                                                                                  |
| `cacheCreationTokens`                                            | number           |                                                                                                                  |
| `cacheReadTokens`                                                | number           |                                                                                                                  |
| `totalTokens`                                                    | number           | **必须 ≥ 四类之和**；小于则拒收                                                                                  |
| `metadata.lastActivity`                                          | string           | 形如 `2026-10-15T09:12:04.517Z`。这是**唯一可靠的发生时间**来源；缺失时退回从 `period` 的 `YYYY/MM/DD/` 前缀解析 |
| `modelBreakdowns[]`                                              | array            | **有 token 就必须非空**。每行按模型落库，空数组 + 非零 token 会被拒收（否则会静默丢数据）                        |
| `modelBreakdowns[].modelName`                                    | string，**必需** |                                                                                                                  |
| `modelBreakdowns[].{input,output,cacheCreation,cacheRead}Tokens` | number           |                                                                                                                  |
| `modelBreakdowns[].cost`                                         | number           | 美元。缺省表示**未定价**（NULL），不是 0 元                                                                      |
| `modelBreakdowns[].missingPricing`                               | boolean          | 为 `true` 时 `cost` 必须是 0 或缺省；非零 → 「标了 missingPricing 却给出金额」                                   |
| `modelsUsed`                                                     | string[]         | 可选，ccusage 会带，宿主不解析                                                                                   |
| `totalCost`                                                      | number           | 可选，行级汇总；**权威金额是 `modelBreakdowns[].cost`**                                                          |

一致性硬规则（宿主逐行校验）：

1. 若某行声明了某类 token 的汇总值，它**必须等于该类在 `modelBreakdowns` 上的合计**。
   不一致 → `session {period} 的 modelBreakdowns 与行汇总不一致：{field} 行=… 明细合计=…`
2. `totalTokens` 必须 ≥ 四类之和；多出来的部分被记为残差，不再分摊。
3. 不要发 `sessionId`：那是 ccusage 的 focused/legacy schema 字段。带 `sessionId` 却没有
   `period` 的行会被明确判为「focused session schema，v0.1 只支持 unified schema」。

> **注意**：设计规格 §3 里把该回包写成了 `sessions`（复数）。以本文件与宿主代码为准，
> 键是单数 `session`。宿主的 `CcusageReport` 反序列化 `session`，写复数会解析失败。

## 5. skill/list 与 skill/get

- `skill/list` 返回摘要，`skills_cache_json` 会在注册时被写入；
  `"[]"` 表示「已取过，零个技能」，`NULL` 表示「还没取过」。两者语义不同，别把前者当后者。
- `skill/get` 的 `body` 是 SKILL.md 的**原文**，宿主原样透传给会话，**不做解释、不做渲染**。
  所以 body 必须自包含、可直接执行，frontmatter 头部也一并保留即可（宿主不解析它，
  但会话看得懂）。
- 未知 `skill_id`：返回错误（示例用 `-32600` 并附 `unknown skill_id`），不要返回空 body。
- v0.1 **没有热重载**。改了 SKILL.md 需要重新注册/重启宿主。

## 6. Host API

插件可以在等待宿主回包期间，反向向宿主发请求（`plugin → host`），这就是 Host API。
它是**唯一的跨边界能力面**：

| 方法                     | v0.1 行为                | params                                                 |
| ------------------------ | ------------------------ | ------------------------------------------------------ |
| `host/log`               | ✅ 已实现                | `{"level":"info","message":"..."}` → `{"logged":true}` |
| `fs.read_scoped`         | `-32003 not_implemented` | —                                                      |
| `secret.resolve_ref`     | `-32003 not_implemented` | —                                                      |
| `db.emit_event`          | `-32003 not_implemented` | —                                                      |
| `config.read_owned`      | `-32003 not_implemented` | —                                                      |
| `config.write_projected` | `-32003 not_implemented` | —                                                      |
| `process.spawn_declared` | `-32003 not_implemented` | —                                                      |

- `host/log` 只读取 `level` 与 `message` 两个字段，写一行宿主 stderr 日志（本仓库不引日志框架），
  返回 `{"logged": true}`。其余字段**不会**被写出——免得插件借日志把 secret 送进宿主日志。
- 未知的 Host API 方法返回 `-32601`（`unknown host api: xxx`）。
- **每次**调用（无论成功失败）都会写一行 `permission_decisions` 审计记录；参数 JSON 超过
  **512 字符**会被截断并追加 `…` 后入库。所以不要把大对象塞进 Host API 参数。

### 6.1 红线（不可协商）

- ❌ **插件不得直连主 SQLite**。插件进程拿不到数据库句柄，也不得自己去打开
  `%APPDATA%/harness-hub/*.sqlite`。数据库归 Rust Control Plane 独占。
- ❌ **插件不得读取 Secret Store**。插件进程没有 Secret Store 句柄，也不得自己去读
  钥匙串 / 凭据文件 / 环境里的 `HARNESS_HUB_*` 密钥。
- ❌ **不得把 secret 原文放进 Host API 参数**。`secret.resolve_ref` 将来的语义是「传引用，
  宿主自己解析」，不是「把 token 递给我」。审计表会记录参数（截断后），写进去就等于落盘。
- 另外：v0.1 的权限是**只观察不拦截**（`decision` 恒为 `observed`）。这意味着
  「没被拦住」不等于「被允许」——请不要把当前能跑通当作安全许可。

## 7. 编码陷阱（Windows 必读）

宿主按 **UTF-8** 解析 stdout，按 **`\n`** 切行。Windows 上 Python 有一个默认行为会让你的
插件在英文测试里全绿、遇到中文就炸：

- Python 只对**控制台**使用 UTF-8（PEP 528）；stdout 一旦被重定向成管道，就退回**本地
  代码页**（中文 Windows 是 cp936）。于是中文被写成非 UTF-8 字节，宿主 `json.loads` 直接崩。
- Windows 文本模式默认把 `\n` 翻译成 `\r\n`。

两个示例插件都在 `main()` 第一步显式钉死：

```python
sys.stdin.reconfigure(encoding="utf-8", errors="strict")
sys.stdout.reconfigure(encoding="utf-8", newline="\n")
sys.stderr.reconfigure(encoding="utf-8", errors="replace")
```

这不是可选的风格问题：跳过它，`hello-skill` 返回的 SKILL.md 中文正文会是非法 UTF-8。

## 8. 错误码

| code     | 名称                | 何时用                                                                               |
| -------- | ------------------- | ------------------------------------------------------------------------------------ |
| `-32700` | `parse_error`       | 收到的行不是合法 JSON。`id` 填 `null`                                                |
| `-32600` | `invalid_request`   | JSON 合法但不是合法 JSON-RPC（如 `jsonrpc` 不是 `"2.0"`、缺 `method`、参数类型不对） |
| `-32601` | `method_not_found`  | 方法不存在，或 Host API 未知。消息里带上方法名                                       |
| `-32603` | `internal_error`    | 插件内部异常。**必须**捕获，不能让异常冒泡杀掉进程                                   |
| `-32001` | `not_supported`     | 方法存在但本版本不实现（`harness/*`、`profile/*`、`trace/*`、`runtime/*`）           |
| `-32002` | `capability_denied` | 保留给权限引擎（Phase 9.2）。**v0.1 不会产生**                                       |
| `-32003` | `not_implemented`   | Host API 在 v0.1 未实现（其余 6 个方法用它）                                         |

错误回包形状：

```json
{ "jsonrpc": "2.0", "id": 7, "error": { "code": -32601, "message": "method not found: foo/bar" } }
```

消息请写成人类可读、带上下文的短句（例如把 `method` 名带进去），它会直接进宿主日志。

## 9. 插件目录约定

```text
<plugin-dir>/
├── manifest.toml       必需：宿主扫描入口，没有它就不是插件
├── adapter/            必需：适配器程序（可以是任何语言的可执行文件）
├── fixtures/           可选：示例数据 / 测试输入
├── skills/             可选：skill 型插件的 SKILL.md 目录树
└── icon.svg            可选：24×24 SVG
```

- 目录名不要求等于 `id`，但强烈建议一致（`id` 是数据库主键与 RPC 句柄）。
- 宿主扫描规则：接受含 `manifest.toml` 的一级或二级子目录；**一个坏插件不影响其他**。
- 扫描根默认是 `<app_data_dir>/plugins`，可用环境变量 `HARNESS_HUB_PLUGINS_DIR` 覆盖。
- `[adapter].command` 的**工作目录固定为插件目录**，所以 `adapter/usage.py` 这种相对路径
  可以放心写；取自己的资源时用脚本位置反推更稳：
  `Path(__file__).resolve().parent.parent`。
- 不经过 shell：`command` 是 argv 数组，`["python", "adapter/usage.py"]` 不会发生
  glob 展开、引号解析或管道。单元素数组（只有程序名）合法。
- Windows 上宿主以 `CREATE_NO_WINDOW` 启动插件，不会弹黑框。

## 10. manifest 字段

```toml
id = "usage-connect"          # ^[a-z][a-z0-9-]*$，trim 后匹配，必填
name = "Usage Connect"        # trim 后非空，必填，允许空格与中文
version = "0.1.0"             # trim 后非空，必填，宿主不解析 semver
api_version = "1"             # 必须是字符串 "1"，其他值一律拒绝注册

[adapter]
command = ["python", "adapter/usage.py"]   # 非空数组，每个元素 trim 后非空

[capabilities]
usage = true                  # 8 个布尔键，缺省全 false
```

| 字段               | 规则                                                                                                             | 失败时   |
| ------------------ | ---------------------------------------------------------------------------------------------------------------- | -------- |
| `id`               | `^[a-z][a-z0-9-]*$`，必须字母开头；`trailing-` 合法，`Usage_Connect` / `usage_connect` / `-lead` / `9lives` 非法 | 注册失败 |
| `name` / `version` | trim 后非空                                                                                                      | 注册失败 |
| `api_version`      | 必须 `"1"`；缺失与「值不对」分开报错                                                                             | 注册失败 |
| `adapter.command`  | 必需、非空数组、元素非空                                                                                         | 注册失败 |
| `capabilities`     | 未知键或非布尔值 → 拒绝并**点名**该键                                                                            | 注册失败 |
| 未知的**顶层**键   | 忽略（向前兼容）                                                                                                 | 不影响   |

`[capabilities]` 的 8 个键：`detect`、`launch`、`resume`、`usage`、`trace`、`profiles`、
`mcp`、`skills`。缺省整节等价于全部 `false`。

为什么未知 capability 键要报错而不是忽略：`usages = true` 这种拼写错误如果被静默吞掉，
插件会以「什么能力都没声明」的身份启动，然后在第一个 RPC 上莫名其妙被拒——那种 bug
极难排查。拼错就是没声明，宿主选择直接拒绝。

机器可校验的版本见 `manifest.schema.json`（JSON Schema draft 2020-12）。
