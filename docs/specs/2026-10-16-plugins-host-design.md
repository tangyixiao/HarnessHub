# 插件宿主 v0.1 设计（Phase 9.3）

- **日期**: 2026-10-16
- **状态**: 已获用户批准（brainstorming 分节确认：方案 A、§1 架构、§2 协议、§3 通路与测试）
- **规格来源**: HARNESS_HUB_PROJECT_PLAN.md Phase 9.3（六项：Manifest schema、Adapter contracts、Capability negotiation、Out-of-process prototype、Example plugin、Permission boundary）

## 0. 目标与已锁定的决策

把计划书 9.3 的六项一次性落成可运行的最小闭环：宿主能发现、spawn、握手、调用两个示例插件（一个 connect 型、一个 skill 型），并把每次 Host API 调用记录为观察数据。

经用户确认的决策：

1. **Permission boundary = 完全 observe-only**。记录插件调用了哪些 Host API，不做强制拒绝；manifest capabilities 是声明元数据，为将来 9.2 Permission Engine 留钩子（规格 4.28 同口径：v0.1 只记录 observed permission events）。
2. **插件形态 = 数据面优先，双示例**。connect 型 = 外部数据/服务接入插件（吃现有 usage 导入管道）；skill 型 = SKILL.md 式可复用指令包，经 SkillAdapter 注入会话启动。两个示例插件各覆盖一个形态。
3. **方案 A：契约全冻结 + 双通路**。六类 Adapter（Harness/Usage/Profile/Trace/Skill/Runtime）类型与 trait 契约全部定义并由 manifest `api_version` 锁定；v0.1 可运行通路只打通 Usage + Skill 两条，其余四类 trait 返回结构化「不支持」。

## 1. 总体架构与目录布局

插件 = 一个目录（`manifest.toml` + `adapter/` + `fixtures/` + `skills/` + `icon.svg`）。

```text
plugins/
  sdk/                        协议与类型定义（v0.1 是文档 + JSON Schema，不是代码库）
    protocol.md               JSON-RPC 方法、握手流程、错误码规范
    manifest.schema.json      manifest.toml 的字段规范（宿主校验依据）
  examples/
    usage-connect/            connect 型示例（Python，stdlib-only）
      manifest.toml
      adapter/usage.py        UsageAdapter 实现（JSON-RPC over stdio）
      fixtures/sample.json    测试数据
      icon.svg
    hello-skill/              skill 型示例（Python，stdlib-only）
      manifest.toml
      adapter/skill.py        SkillAdapter 实现
      skills/demo/SKILL.md    注入用的指令包
      icon.svg
```

宿主侧模块（`src-tauri/src/plugins/`，全部新建）：

| 文件            | 职责                                                                      |
| --------------- | ------------------------------------------------------------------------- |
| `manifest.rs`   | manifest.toml 解析 + 校验（id 规范、api_version 匹配、capabilities 布尔） |
| `registry.rs`   | 已安装插件发现/注册（扫描 `plugins/` 目录），插件表落 SQLite              |
| `rpc.rs`        | 行分隔 JSON-RPC 2.0 codec + 跨 IPC DTO（含 contract test）                |
| `supervisor.rs` | 子进程生命周期：spawn、握手超时、崩溃隔离、stderr 捕获                    |
| `host_api.rs`   | 受限 Host API 分发点 + observe-only 决策记录                              |
| `adapters.rs`   | 六类 Adapter trait 契约（Rust 侧）                                        |

数据流：UI/命令 → `plugins::registry` → `supervisor` spawn 插件进程 → JSON-RPC 握手（capability negotiation）→ 按需调用 Adapter 方法 → 所有 Host API 调用经 `host_api` 记录 `permission_decisions`。

**manifest.toml 形状**（沿用计划书草案，字段全冻结）：

```toml
id = "usage-connect"            # ^[a-z][a-z0-9-]*$，全局唯一
name = "Usage Connect"
version = "0.1.0"
api_version = "1"               # 宿主只接受 "1"

[adapter]
command = ["python", "adapter/usage.py"]   # argv，不经 shell；cwd = 插件目录

[capabilities]
usage = true                    # detect/launch/resume/usage/trace/profiles/mcp/skills 布尔
# 其余缺省 false
```

`[adapter].command` 是**必需**字段（补 spec 初稿遗漏的启动声明）：宿主 `spawn(argv[0], argv[1..])`，
`cwd` 固定为插件目录，因此相对路径参数天然解析到插件内部；**禁止经 shell 展开**
（不做 `sh -c`，避免 manifest 变成任意命令执行入口）。空数组/空串/缺失 = manifest 校验失败。

## 2. 协议、握手与 Adapter 契约

**传输**：行分隔 JSON-RPC 2.0 over stdio（宿主写插件 stdin，插件回 stdout；每行一个完整 JSON 消息）。stderr 独立管道捕获到 64 KiB 环形缓冲，崩溃时随错误报告输出。`api_version = "1"`，宿主只接受匹配值。

**握手（capability negotiation）**：

```text
宿主→插件  initialize    {host_version, api_version:"1", manifest_capabilities}
插件→宿主  response      {plugin_id, plugin_version, supported_methods:[...]}
插件→宿主  initialized   (notification —— 握手完成)
```

允许集 = 插件上报 `supported_methods` ∩ manifest `[capabilities]` 声明。宿主只会调用声明过的 Adapter 通路（静态声明路由，不是权限引擎）。握手超时 10s → 注册失败；方法调用超时 30s；v0.1 不做心跳（YAGNI）。

**Adapter 方法**（宿主→插件）：

| 方法                                             | 通路         | v0.1 状态                                             |
| ------------------------------------------------ | ------------ | ----------------------------------------------------- |
| `usage/import`                                   | connect 示例 | **打通**：结果喂现有 usage 导入管道                   |
| `skill/list` / `skill/get`                       | skill 示例   | **打通**：SKILL.md 注入会话启动                       |
| `harness/*`、`profile/*`、`trace/*`、`runtime/*` | —            | trait + 协议方法定义，实现返回 `-32001 not_supported` |

**Host API**（插件→宿主，7 个受限 API 按计划书全列）：

| 方法                                                                                                                             | v0.1 状态                                   |
| -------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------- |
| `host/log`                                                                                                                       | **真实实现**（写 tracing + 记录决策）       |
| `fs.read_scoped`、`secret.resolve_ref`、`db.emit_event`、`config.read_owned`、`config.write_projected`、`process.spawn_declared` | 协议定义，宿主返回 `-32003 not_implemented` |

**Permission boundary（observe-only + migration 0008）**：每次 Host API 调用写一行：

```sql
permission_decisions(
  plugin_id TEXT NOT NULL,
  api TEXT NOT NULL,
  decision TEXT NOT NULL,          -- v0.1 恒 'observed'
  reason TEXT,
  args_json TEXT,                  -- 截断到 512 字符
  created_at TEXT NOT NULL
)
```

红线：Host API 参数禁止携带 secret 原文（写入前截断即是兜底，示例插件也不得传）。结构层 `-32002 capability_denied` 保留给未声明通路的调用（v0.1 下宿主根本不会发起这类调用，错误码为 9.2 留位）。

**进程遏制（R14 + ADR-0013）**：插件进程纳入 Windows Job Object，宿主退出不留孤儿；插件异常退出标记 degraded，下次调用 lazy respawn，绝不 panic 宿主主线程。

**ADR-0018**：随本设计落 `docs/adr/0018-plugin-isolation-boundary.md`（补上 `plugins/README.md:10` 与 `src-tauri/src/plugins/mod.rs:4` 引用的缺失锚点），内容 = 两条红线 + observe-only 决策。

## 3. 两条打通通路的数据流 + 测试策略

**通路 1：Usage connect（connect 型示例）**

```text
插件进程(Python) --usage/import--> 宿主 adapters.rs
  → 解析成 Vec<NormalizedUsage 草稿>（形状约定复用 usage::parse_report/normalize_session）
  → UsageImporter::import(ImportRequest) 走 usage_imports/usage_events 现有表
  → source = "plugin:usage-connect"，不新建 usage 表
```

`usage/import` 响应载荷 = ccusage `session` 数组同构 JSON（复用为契约）。宿主侧解析规则定死：插件响应 JSON **直接组装成 ccusage 报告形状**（`{"sessions":[...]}`），然后走**现有** `usage::parse_report → normalize_session` 路径——不为插件新建第二套解析器，保证插件数据与 ccusage 数据在同一套契约下校验。幂等 key = `stable_source_key`（ADR-0011），重复导入自动去重。插件不被信任：解析失败 = 结构化错误回给调用方，不影响主库。

**通路 2：Skill inject（skill 型示例）**

```text
UI(命令) → plugins::registry → spawn 插件 → skill/list + skill/get
  → 返回 {skill_id, name, description, body} 列表
  → 写入 plugins 表 skills_cache（每次注册时刷新）
```

v0.1 只做**注册时拉取并缓存**，不做运行时热更新（YAGNI）；SKILL.md 只透传不解析（宿主不解释 skill 语义）。新增 2 个 IPC 命令：`plugin_list`（读插件表 + skills_cache，ADR-0012 只读边界）、`plugin_refresh`（显式触发扫描 + spawn + 刷新缓存）。

**数据落点（migration 0008 + 0009）**：

- `0008_permission_decisions.sql`：上表。
- `0009_plugins.sql`：`plugins(id PK, name, version, api_version, path, capabilities_json, skills_cache_json, status, registered_at)`。

**测试策略（TDD 顺序）**：

- `manifest.rs`：解析/校验单测（坏 id、api_version 不匹配、capabilities 非布尔等）。
- `rpc.rs`：JSON-RPC codec 往返单测 + 跨 IPC DTO contract test（AGENTS.md IPC 契约纪律：`rename_all_fields` 锁键名）。
- `supervisor.rs`：真进程握手测试（fixture Python 脚本，Windows 下 python.exe 直起；握手超时、崩溃 → degraded、stderr 捕获各一条）。
- `host_api.rs`：observe-only 记录单测（含 512 字符截断）。
- `real_plugins`（`src-tauri/tests/`）：端到端真机测试 = 两个示例插件全链路（spawn → 握手 → usage/import 落库 → skill/list 缓存 → 杀进程 → degraded → lazy respawn）。
- 前端不加 UI（v0.1 无 UI 需求，IPC 命令 + contract test 覆盖）。

## 4. 明确不做（v0.1 边界）

- 强制权限拒绝（9.2 Permission Engine 的职责，本设计只留 `capability_denied` 错误码与 decisions 表钩子）。
- WASM 运行方式（计划书优先级 out-of-process > WASM > in-process，v0.1 只做第一种）。
- 插件市场/分发/签名（计划书 508 行：v0.1 不发布市场）。
- Skill 热更新、心跳保活、插件间通信。
- harness/profile/trace/runtime 四类通路实现（契约冻结、返回 not_supported）。
- ProfileAdapter 消费者（HHAR 是 9.5，未实现）。

## 5. 完成定义

六项对照：Manifest schema = `manifest.rs` + `manifest.schema.json`；Adapter contracts = `adapters.rs` 六 trait + protocol.md；Capability negotiation = 握手流程；Out-of-process prototype = `supervisor.rs` Job Object + stdio JSON-RPC；Example plugin = 两个示例；Permission boundary = observe-only `permission_decisions` + ADR-0018。全部完成门（format/lint/typecheck/test/rust:test/python:test/build/clippy）通过。
