# 插件宿主 v0.1 执行计划（Phase 9.3）

- 日期：2026-10-16
- 规格：`docs/specs/2026-10-16-plugins-host-design.md`
- 分支：`feat/plugins-host`（基于 `c056e5e`，spec 已提交为 `8b524db`）
- 提交切分（沿用上一步惯例）：`docs(plan): …` + `feat(plugins): …`，走 PR，推送 SSH 优先失败回退 HTTPS。
- 前置确认：`git status` 只有未跟踪的 `docs/handoffs/`（不提交）。

## 目标（六项对照）

| 计划书 9.3 项            | 交付物                                                                                               |
| ------------------------ | ---------------------------------------------------------------------------------------------------- |
| Manifest schema          | `plugins/sdk/manifest.schema.json` + `src-tauri/src/plugins/manifest.rs`                             |
| Adapter contracts        | `src-tauri/src/plugins/adapters.rs` 六 trait + `plugins/sdk/protocol.md`                             |
| Capability negotiation   | `supervisor.rs` 握手（`supported_methods ∩ manifest capabilities`）                                  |
| Out-of-process prototype | `supervisor.rs`（stdio JSON-RPC + Windows Job Object）                                               |
| Example plugin           | `plugins/examples/usage-connect/`（connect 型）+ `plugins/examples/hello-skill/`（skill 型）         |
| Permission boundary      | `host_api.rs` + migration `0008_permission_decisions` + `docs/adr/0018-plugin-isolation-boundary.md` |

## Task 1 — 契约层（先 RED）

1. `src-tauri/Cargo.toml` 加 `toml = "0.8"`（当前只是间接依赖，不能直接 `use`）。
2. `src-tauri/src/plugins/manifest.rs`：先写测试，再写实现。测试覆盖：
   - 正常解析（id/name/version/api_version/`[adapter].command`/capabilities 布尔）；
   - 非法 id（`Usage_Connect`、`-x`、空串）→ `Error::InvalidInput`；
   - `api_version = "2"` → 拒绝（宿主只接受 `"1"`）；
   - `[adapter].command` 缺失 / 空数组 / 含空串 → 拒绝；
   - `[capabilities]` 缺省全 false；未知 capability 键 → 拒绝（拼错就是没声明，不许静默）；
   - 未知顶层键不报错（向前兼容只允许**新增可选**字段，不允许新增必填）。
3. `src-tauri/src/plugins/rpc.rs`：行分隔 JSON-RPC 2.0 codec。
   - `Request { jsonrpc, id, method, params }`、`Response`、`ErrorObject { code, message, data }`、`Notification`；
   - `encode_line` / `decode_line`（空行跳过、坏 JSON → `Error::InvalidInput` 且**不 panic**）；
   - 错误码常量：`-32700` parse、`-32601` method_not_found、`-32603` internal、`-32001 not_supported`、`-32002 capability_denied`、`-32003 not_implemented`。
   - **跨 IPC 契约测试**：`PluginSummary` / `PluginSkill` 的 JSON 键名逐字锁定（AGENTS.md 纪律）。
4. `src-tauri/src/plugins/adapters.rs`：六类 trait 契约（`HarnessAdapter`/`UsageAdapter`/`ProfileAdapter`/`TraceAdapter`/`SkillAdapter`/`RuntimeAdapter`），四个未实现通路统一返回 `not_supported()` 的构造器；`AdapterError` 与 JSON-RPC 错误码互转（单测覆盖每个映射）。
5. `src-tauri/src/plugins/mod.rs` 挂子模块；`plugins` 保持 `pub mod`。
6. RED 证据：`cargo test --manifest-path src-tauri/Cargo.toml --lib plugins` 失败，把真实输出记进本文件。

## Task 2 — 数据层

1. `src-tauri/src/db/migrations/0008_permission_decisions.sql`（列见 spec §2，含 `created_at` 索引）。
2. `src-tauri/src/db/migrations/0009_plugins.sql`：
   ```sql
   plugins(id TEXT PRIMARY KEY, name TEXT NOT NULL, version TEXT NOT NULL,
           api_version TEXT NOT NULL, path TEXT NOT NULL,
           capabilities_json TEXT NOT NULL, skills_cache_json TEXT,
           status TEXT NOT NULL, registered_at TEXT NOT NULL)
   ```
3. `migrations.rs` 注册两条；`cargo test --lib db` 必须验证**裸 `Connection` + `apply_until(7)` → `apply_pending` → 版本 9**（AGENTS.md 冷启动纪律）。
4. `src-tauri/src/plugins/registry.rs` 的 store 层：`upsert_plugin` / `list_plugins` / `mark_degraded`；`status` 取值 `ready` / `degraded`。

## Task 3 — Host API（observe-only）

`src-tauri/src/plugins/host_api.rs`：

1. `dispatch(plugin_id, plugin_root, method, params, &Connection) -> RpcResult`：
   - `host/log` 真实实现（`tracing::info!` + 写决策行）；
   - `fs.read_scoped` / `secret.resolve_ref` / `db.emit_event` / `config.read_owned` / `config.write_projected` / `process.spawn_declared` → `-32003 not_implemented`（**仍写决策行**，失败也要留痕）；
   - 未知方法 → `-32601`。
2. `record_decision`：`args_json` 序列化后**按字符边界截断到 512 字符**（不能用 `[..512]` 切 UTF-8，会 panic）；超长追加 `…` 且单测覆盖中文/emoji。
3. 单测：截断、未知方法、`decision` 恒 `'observed'`、失败调用也写行。

## Task 4 — Supervisor（最难的一块）

`src-tauri/src/plugins/supervisor.rs`：

1. `Supervisor::spawn(manifest, plugin_root) -> Result<PluginProcess>`：
   - `std::process::Command` + `cwd(plugin_root)` + `stdin/stdout` piped + `stderr` piped；
   - **不经 shell**；`CREATE_NO_WINDOW`（Windows）；
   - Windows：`Containment::create()` + `assign(pid)`（`#[cfg(windows)]` 门控——`pty::containment` 没有非 Windows stub，直接调会编译失败）。
2. 握手 `handshake(&mut self, host_version) -> Result<AllowedMethods>`：发 `initialize`，等 `response` + `initialized`；超时 10s → 杀掉进程 + `Error::PluginHandshake{…}`；`plugin_id` 回包与 manifest 不一致 → 拒绝。
3. 方法调用 `request(method, params, timeout) -> RpcResult`：超时 30s；插件进程已死 → 返回 `degraded` 并 lazy respawn 一次（重试仍失败则报错，不 panic 主线程）。
4. stderr 环形缓冲 64 KiB；崩溃时错误信息带上最后若干行。
5. `Drop` / `shutdown()`：关 stdin → 等 2s → `terminate_tree()`。
6. 测试（真进程，`python` 在 PATH）：
   - `fixtures/echo_plugin.py`（应答握手 + `usage/import`）→ 全链路 ok；
   - `fixtures/silent_plugin.py`（不该答）→ 握手超时且进程被杀；
   - `fixtures/crashing_plugin.py`（握手后立刻 `os._exit(1)`）→ 调用返回 degraded，宿主不 panic；
   - `fixtures/chatty_stderr.py`（狂写 stderr）→ 缓冲不无界增长。

## Task 5 — Registry 装配

`registry.rs`：

1. `default_root(app_data_dir)`：`HARNESS_HUB_PLUGINS_DIR` 有值则用它，否则 `app_data_dir/plugins`。
2. `scan(root) -> Vec<DiscoveredPlugin>`：只认含 `manifest.toml` 的一级/二级目录；单个插件坏掉只记 `error`，**不影响其他插件**（R14）。
3. `refresh(&mut self, ...)`：scan → 逐个 spawn + 握手 → `usage` 通路不自动跑（导入是显式动作）；`skills` 通路在注册时 `skill/list` + `skill/get` 拉取并写 `skills_cache_json`（v0.1 不热更新）。
4. 单测：坏 manifest 不影响好 manifest；缺目录返回空而不是报错。

## Task 6 — IPC 命令

1. `src-tauri/src/commands.rs`：`plugin_list`（只读，ADR-0012）、`plugin_refresh`（显式写操作）。
2. `src-tauri/src/lib.rs`：`invoke_handler` 19 → 21（保持既有风格），`AppState` 加 `plugins: Arc<Mutex<PluginRegistry>>`；`setup` 里建 root 目录（不存在就创建）。
3. 命令层契约测试：`plugin_list_serializes_summary`（camelCase 键逐字）。
4. `src/lib/ipc.ts` 加类型 + `listPlugins()` / `refreshPlugins()` + fixture test（AGENTS.md：TS normalize 必须有 fixture test）。**v0.1 不加 UI**，只保证 IPC 契约可用。

## Task 7 — 示例插件与 SDK 文档（可与 Task 1-6 并行）

1. `plugins/sdk/protocol.md`：方法表、握手时序、错误码、`initialize` 参数形状、Host API 参数形状。
2. `plugins/sdk/manifest.schema.json`：镜像 `manifest.rs` 的校验规则（id/pattern、api_version const、command minItems 1）。
3. `plugins/examples/usage-connect/`：`manifest.toml` + `adapter/usage.py`（stdlib-only，手写 JSON-RPC 循环）+ `fixtures/sample.json` + `icon.svg`。
4. `plugins/examples/hello-skill/`：`manifest.toml` + `adapter/skill.py` + `skills/demo/SKILL.md` + `icon.svg`。
5. `plugins/README.md` 更新：从「当前为空」改为实际布局 + `HARNESS_HUB_PLUGINS_DIR` 说明。
6. 自测：`python plugins/examples/usage-connect/adapter/usage.py` 手工喂一行 `initialize` 能回包（记录命令与输出）。

## Task 8 — 真机端到端

`src-tauri/tests/real_plugins.rs`（`mod common;` + `CARGO_MANIFEST_DIR` 定位仓库根）：

1. 两个示例插件登记进临时 plugins root → `refresh` → 断言两个都 `ready`、capabilities 与 skills_cache 正确。
2. `usage/import` 全链路：插件返回 `{"sessions":[…]}` → `usage::parse_report` → `normalize_session` → `UsageImporter` → 断言 `usage_events` 行数、幂等（连导两次行数不变）、`source = "plugin:usage-connect"`。
3. `skill/get` 返回 `SKILL.md` 正文原文。
4. 杀插件进程 → 下一次调用 degraded → `refresh` 后恢复 `ready`。
5. `permission_decisions` 有 `host/log` 记录，且超长参数被截断到 ≤512。

## Task 9 — 文档与提交

1. `docs/adr/0018-plugin-isolation-boundary.md`（补 `plugins/README.md:10` 与 `src-tauri/src/plugins/mod.rs:4` 的缺失锚点）。
2. `docs/CONTEXT.md` §6 + `docs/CONTEXT-MAP.md`：插件宿主模块与两条通路。
3. Gates 表填**真实输出**（format / rust fmt / lint / typecheck / pnpm test / rust:test / python:test / build / clippy）。
4. 提交：`docs(plan): Plugins host v0.1 plan` → `feat(plugins): out-of-process plugin host with usage and skill paths`。

## 完成门（2026-10-17 实测）

| Gate        | 命令                                 | 真实输出                                                                                                  |
| ----------- | ------------------------------------ | --------------------------------------------------------------------------------------------------------- |
| format      | `pnpm format:check`                  | `All matched files use Prettier code style!`                                                              |
| rust fmt    | `pnpm rust:fmt:check`                | 无输出（`cargo fmt --check` 通过即静默）                                                                  |
| lint        | `pnpm lint`                          | `✖ 4 problems (0 errors, 4 warnings)`——四个 `react-refresh/only-export-components` 系既有告警，非本次引入 |
| typecheck   | `pnpm typecheck`                     | `tsc --noEmit` 无输出                                                                                     |
| unit (web)  | `pnpm test`                          | `Test Files 9 passed (9)` / `Tests 179 passed (179)`，5.25s                                               |
| unit (rust) | `pnpm rust:test -- --test-threads=1` | 全绿：lib 435 passed；28 个集成测试目标全部 `0 failed`（`real_plugins` 12 passed 含在内），退出码 0       |
| unit (py)   | `pnpm python:test`                   | `Ran 12 tests in 0.001s` / `OK`                                                                           |
| build       | `pnpm build`                         | `✓ built in 6.83s`（rolldown；tailwind hook 3.3s 属正常 timing 报告）                                     |
| clippy      | `pnpm rust:clippy`                   | `Finished dev profile ... in 4.98s`（`-D warnings` 干净）                                                 |

插件相关单测细目：`cargo test --lib plugins` **76 passed / 0 failed**（10.35s）；
`cargo test --test real_plugins` **12 passed / 0 failed**（1.13s）。

两处环境性插曲，如实记录：

1. 首次 `pnpm rust:test` 并行跑时，多个测试目标报
   `E0786: found invalid metadata files for crate harness_hub_lib` /
   `failed to mmap file ... (os error 1455 页面文件太小)`，随后级联成一片 `E0463`。
   这是提交内存不足导致的 mmap 失败，不是代码问题：`--test-threads=1` 串行跑即全绿。
2. 同一轮并行跑里 `opencode_lifecycle` 的 S2 失败过一次，单独重跑与串行全跑均通过。
   真机 TUI 测试对机器负载敏感（S2 的 25s 观察窗 + TUI 首屏时序），
   与此前 2026-10-16 记录的抖动同源，非回归。

## 已知风险

1. **`pty::containment` 没有非 Windows stub** → supervisor 必须 `#[cfg(windows)]` 门控，否则 Linux/macOS 编译失败。
2. **`toml` 不在直接依赖里** → 先加 `Cargo.toml`，否则 `use toml::` 直接编译失败。
3. **插件响应形状**：`CcusageReport.session` 的 JSON 键是 `session`（单数），spec §3 写的是 `sessions`。宿主侧组装 `{"session":[…]}` 再走 `parse_report`，不要另写解析器。
4. **`Containment::assign` 必须在进程刚起时调用**，晚了子进程可能已派生孙进程（ADR-0013）。
5. **真机测试会真起 `python`**：PATH 里必须有 `python`（本机 `D:\Python\314\python.exe` 在 PATH）。测试里用 `python` 而不是绝对路径，缺失时用 `#[ignore]` 式跳过而不是硬失败。
6. **前端 invoke spy 收 3 个参数**（上一步踩过）：断言 `?.limit` 而不是参数个数。
7. **SQLite `?N` 绑定顺序**：插件表没有时区修饰符，但若 join usage 需注意（本设计不 join）。
