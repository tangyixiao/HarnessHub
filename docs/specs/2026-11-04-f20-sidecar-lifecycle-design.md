# F20 Python Sidecar 生命周期 — 设计规格

- 日期：2026-11-04
- 阶段：Track F / F20（目标 v0.5.0）
- 依据：`docs/ROADMAP.md:144-146`、`HARNESS_HUB_PROJECT_PLAN.md:2873-2879`（§8.1 验收清单）、ADR-0006（`HARNESS_HUB_PROJECT_PLAN.md:3591`）
- 前置：`src-tauri/src/plugins/supervisor.rs`（进程监督先例）、`src-tauri/src/plugins/rpc.rs`（行分隔 JSON-RPC 编解码）、`src-tauri/src/pty/containment.rs`（ADR-0013 Job Object 进程树）
- 分支基线：`feat/f20-sidecar-lifecycle` @ `3ebcb2e`（= 真实远程 main）

## 1. 目标与验收

`HARNESS_HUB_PROJECT_PLAN.md` §8.1 的五项验收逐条落到可测行为：

| #   | 验收项                                     | 本规格的落点                                                                                           |
| --- | ------------------------------------------ | ------------------------------------------------------------------------------------------------------ |
| 1   | Rust 能启动 / 关闭 Python sidecar          | `SidecarProcess::spawn` / `shutdown`；IPC `sidecar_start` / `sidecar_stop`                             |
| 2   | stdio JSON-RPC handshake                   | `initialize` 请求 + 版本校验 + 能力协商                                                                |
| 3   | Runtime version / capability negotiation   | `RuntimeInfo` DTO + 交集协商 + 不兼容即降级                                                            |
| 4   | Sidecar crash 可恢复                       | `SidecarManager` 状态机 `Degraded → Ready`；`sidecar_restart`                                          |
| 5   | 无 Python AI 功能时主 Harness 功能仍能工作 | sidecar 全部失败都收敛为 `SidecarStatus`，**不向上传播为 IPC 错误**；`AppState` 构造不依赖 Python 存在 |

### Non-goals（明确不做，留给 F21/F23）

- 不实现 Provider Gateway / LiteLLM / Tokenizer（F21）。
- 不实现 MCP client / server（F22）。
- 不打包 Python 解释器、不引入 torch/transformers、不下载 HF 模型（F23）。
- 不开放 localhost HTTP 端口 —— transport 只有 stdio JSON-RPC（ADR-0006）。
- Python sidecar **不得**直接访问 SQLite；数据库归 Rust Control Plane 独占。
- 不改 SQLite schema、不改已提交的 migration。
- 不改 `src/features/*` 之外的既有 UI 区块；只新增一个 Sidecar 状态卡片。

## 2. 架构与依赖方向

```text
src/features/settings/SidecarCard
  → src/lib/ipc.ts（唯一 IPC 入口，组件内禁止 invoke()）
  → Tauri IPC（sidecar_status / sidecar_start / sidecar_stop / sidecar_restart）
  → src-tauri/src/commands.rs
  → src-tauri/src/sidecar/manager.rs   (Arc<SidecarManager>，AppState 持有)
  → src-tauri/src/sidecar/process.rs   (子进程 + stdio 读线程)
  → src-tauri/src/plugins/rpc.rs       (复用 codec，不重复造)
  → python/harness_hub_runtime/        (stdlib-only，stdio JSON-RPC)
```

约束（AGENTS.md / `docs/CONTEXT-MAP.md`）：

- `src-tauri/src/sidecar/**` 是领域模块，**不得依赖任何 Tauri GUI 类型**；只有 `commands.rs` 与 `lib.rs::run()` 允许碰 `tauri::State` / `AppHandle`。
- 复用 `crate::plugins::rpc::{decode_line, encode_request, Incoming, RpcError}`；**禁止**在 sidecar 模块里再写一份 JSON-RPC 编解码器。
- `crate::error::Result<T>` 是单参数别名；需要双参数结果时写全 `std::result::Result<T, E>`（supervisor.rs 已有此注释先例）。
- 进程树回收在 Windows 走 `crate::pty::containment::Containment`（ADR-0013），`KILL_ON_JOB_CLOSE`；**立即** `assign(child.id())`，否则抓不到后代。
- Windows 一律 `CREATE_NO_WINDOW = 0x0800_0000`，不弹黑框。

## 3. 线协议（stdio JSON-RPC 2.0，行分隔）

- 每行一个 JSON 对象，以恰好一个 `\n` 结尾；消息体内部换行必须被转义（复用 `plugins::rpc::encode_line`）。
- 空行忽略；坏行不杀读线程，记 `eprintln!` 后继续。
- `jsonrpc` 恒为 `"2.0"`；`id` 为 `u64`，从 1 递增；通知无 `id`、无回包。

### 3.1 `initialize`（握手，Host → Sidecar）

request params：

```json
{
  "hostVersion": "0.5.0",
  "protocolVersion": "1.0",
  "requiredCapabilities": ["ping", "runtime.info", "shutdown"]
}
```

result（成功）：

```json
{
  "protocolVersion": "1.0",
  "runtime": {
    "name": "harness-hub-runtime",
    "version": "0.5.0",
    "transport": "stdio-jsonrpc",
    "python": "3.14.3",
    "protocolVersion": "1.0",
    "providers": [],
    "capabilities": ["ping", "runtime.info", "shutdown"],
    "methods": ["ping", "runtime.info", "initialize", "shutdown"]
  },
  "capabilities": ["ping", "runtime.info", "shutdown"],
  "methods": ["ping", "runtime.info", "initialize", "shutdown"]
}
```

协商规则（Rust 侧强制）：

1. `protocolVersion` 主版本号（`.` 前第一段）必须与 Host 一致，否则 `Error::SidecarHandshake`。
2. `requiredCapabilities` 必须全部出现在 `capabilities` 中，缺任何一个即 `Error::SidecarHandshake`，错误信息列出缺失项。
3. `runtime.name` 必须等于 `"harness-hub-runtime"`，否则 `Error::SidecarHandshake`（防误连到别的 Python 进程）。
4. 握手超时 `HANDSHAKE_TIMEOUT = 10s`；超时 → 杀进程 + `Error::SidecarTimeout`。
5. 握手**不**等待 `initialized` 通知（sidecar 可不发），避免像插件宿主那样依赖额外往返。

### 3.2 `runtime.info`

result 形状与 `initialize.result.runtime` **完全一致**（同一构造函数产出，禁止两处手写）。Host 用它做运行期复核。

### 3.3 `ping`

result：`{"pong": true}`。调用超时 `CALL_TIMEOUT = 30s`。

### 3.4 `shutdown`

- 带 `id` 的请求：result `{"stopping": true}`，**写完并 flush 后** `serve()` 退出循环，`main()` 返回 0。
- 不带 `id` 的通知：静默退出循环，不回包。
- 这是优雅关闭路径；Rust 侧仍保留「关 stdin → 等 `SHUTDOWN_GRACE = 2s` → kill 进程树」的兜底。
- `serve()` 收到 EOF（stdin 关闭）也必须正常返回（不抛异常）。

### 3.5 错误码

沿用 `plugins/rpc.rs` 与 Python `server.py` 已有常量：`PARSE_ERROR=-32700` / `INVALID_REQUEST=-32600` / `METHOD_NOT_FOUND=-32601` / `INTERNAL_ERROR=-32603`。不新增错误码。

版本：`python/harness_hub_runtime/__init__.py` 的 `__version__` 由 `"0.0.0"` 升为 `"0.5.0"`（与 Track F 目标版本对齐；F20 是 Track F 第一个落地阶段）。

## 4. Rust 侧契约

### 4.1 DTO（`src-tauri/src/sidecar/dto.rs`）

**所有跨 IPC 的 DTO 必须带 `#[serde(rename_all = "camelCase")]` 且必须有 JSON serialization contract test 锁定实际键名与嵌套形状。** `rename_all` 只改变枚举体名；结构变体的字段必须 `rename_all_fields` —— 因此本规格的枚举**只用单元变体**，规避该坑。

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInfo {
    pub name: String,
    pub version: String,
    pub transport: String,
    pub python: String,
    pub protocol_version: String,
    pub providers: Vec<String>,
    pub capabilities: Vec<String>,
    pub methods: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SidecarState { Stopped, Ready, Degraded }

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SidecarStatus {
    pub state: SidecarState,
    pub pid: Option<u32>,
    pub runtime: Option<RuntimeInfo>,
    pub negotiated_capabilities: Vec<String>,
    pub protocol_version: Option<String>,
    pub last_error: Option<String>,
    pub restart_count: u32,
}
```

锁定的 JSON 形状（contract test 必须逐键断言，不能只 `assert!(is_ok())`）：

```json
{
  "state": "ready",
  "pid": 1234,
  "runtime": {
    "name": "harness-hub-runtime",
    "version": "0.5.0",
    "transport": "stdio-jsonrpc",
    "python": "3.14.3",
    "protocolVersion": "1.0",
    "providers": [],
    "capabilities": ["ping"],
    "methods": ["ping"]
  },
  "negotiatedCapabilities": ["ping", "runtime.info", "shutdown"],
  "protocolVersion": "1.0",
  "lastError": null,
  "restartCount": 0
}
```

`SidecarState` 单元变体序列化为 `"stopped"` / `"ready"` / `"degraded"`（小写、非 snake_case、非 SCREAMING），contract test 必须逐个断言字符串字面量。

### 4.2 配置（`src-tauri/src/sidecar/config.rs`）

```rust
pub const PROTOCOL_VERSION: &str = "1.0";
pub const REQUIRED_CAPABILITIES: &[&str] = &["ping", "runtime.info", "shutdown"];
pub const RUNTIME_NAME: &str = "harness-hub-runtime";

pub struct SidecarConfig {
    pub python: PathBuf,
    pub runtime_root: PathBuf,
    pub host_version: String,
}
```

解析优先级：

1. `HARNESS_HUB_PYTHON` → 指定的解释器可执行文件。
2. 否则字面量 `"python"`（走 PATH；与 supervisor 真机测试同一策略）。
3. `HARNESS_HUB_RUNTIME_ROOT` → Python 包根目录；否则 `CARGO_MANIFEST_DIR/../python`（dev 回退）。
4. `host_version` = `env!("CARGO_PKG_VERSION")`。

启动命令固定为 `<python> -m harness_hub_runtime`，并 `Command::env("PYTHONPATH", runtime_root)`。**不经 shell**、不拼接用户输入。打包态的资源定位留到 F23（本规格只保证 dev / env 覆盖可用，并写入已知限制）。

### 4.3 进程（`src-tauri/src/sidecar/process.rs`）

```rust
pub struct SidecarProcess { /* 字段私有 */ }

impl SidecarProcess {
    pub fn spawn(config: &SidecarConfig) -> Result<Self>;
    pub fn handshake(&mut self, config: &SidecarConfig) -> Result<RuntimeInfo>;
    pub fn call(&mut self, method: &str, params: &Value) -> Result<Value>;
    pub fn ping(&mut self) -> Result<bool>;
    pub fn pid(&self) -> u32;
    pub fn is_alive(&mut self) -> bool;
    pub fn stderr_tail(&self, max_lines: usize) -> String;
    pub fn shutdown(&mut self);
}
```

- stdout 读线程：`BufReader::new(stdout).lines()` → `rpc::decode_line` → `mpsc::channel::<Incoming>()`；坏行只记日志。
- stderr 读线程：8192 字节 chunk 写入本地 64 KiB `RingBuffer`（`STDERR_LIMIT = 64 * 1024`），`tail(max_lines)` 用 `String::from_utf8_lossy`。**不复用 supervisor 的私有 RingBuffer**（不跨无关模块重构），在 sidecar 模块内实现一份小的。
- `wait_response(id, deadline)`：丢弃过期 id 的响应；`Response.error` → `Error::SidecarProtocol`；`Request` / `Notification` 一律 `eprintln!` 丢弃（F20 不提供 Host API，F21 再扩）。
- `RecvTimeoutError::Timeout` → 杀进程 + `Error::SidecarTimeout`（文案里写实际毫秒数 + `stderr_tail(20)`）。
- `RecvTimeoutError::Disconnected` → 杀进程 + `Error::SidecarDegraded`（带 `child.try_wait()` 退出状态 + stderr tail）。
- 写 stdin 失败（`BrokenPipe`）→ `Error::SidecarDegraded`。
- `Drop` 调 `shutdown()`；`shutdown()` 必须幂等。

### 4.4 管理器（`src-tauri/src/sidecar/manager.rs`）

```rust
pub struct SidecarManager { inner: Mutex<ManagerInner>, config: SidecarConfig }

impl SidecarManager {
    pub fn new(config: SidecarConfig) -> Arc<Self>;
    pub fn status(&self) -> SidecarStatus;
    pub fn start(&self) -> SidecarStatus;
    pub fn stop(&self) -> SidecarStatus;
    pub fn restart(&self) -> SidecarStatus;
    pub fn call(&self, method: &str, params: &Value) -> std::result::Result<Value, String>;
    pub fn shutdown(&self);
}
```

状态机：

```text
Stopped --start()--> Ready        (spawn + handshake 成功)
        --start()--> Degraded     (spawn / handshake 失败，last_error 落库到内存)
Ready   --进程死亡--> Degraded    (status() 用 try_wait() 检出，last_error = "sidecar 进程已退出")
Degraded--start() / restart()--> Ready | Degraded（自动恢复，restart_count += 1）
任意    --stop()--> Stopped
```

**关键纪律（对应验收 #5）**：`start` / `stop` / `restart` / `status` **一律不返回 `Err`** —— 所有失败都转成 `SidecarStatus { state: Degraded, last_error: Some(..) }`。这样 Python 缺失、`PYTHONPATH` 错、握手失败都只影响这一个卡片，PTY / Git / Session / 插件宿主等主功能零感知。`call()` 单独保留 `Result` 供 F21 使用（当前 UI 不调用它）。

- `status()` 必须检查 `is_alive()`：进程已退出就从 `Ready` 翻成 `Degraded`，这是**崩溃可恢复**的检出点。
- `start()` 幂等：`Ready` 且进程仍活着时直接返回当前状态，不重复 spawn。
- `restart()` = `stop()` + `start()`，`restart_count += 1`。
- `Mutex` 中毒不能 panic：`unwrap_or_else(|e| e.into_inner())`，与仓库既有做法一致。
- `Drop for SidecarManager` 调 `shutdown()`；`lib.rs::run()` 的 `RunEvent::Exit` 也显式调一次（双保险、幂等）。

### 4.5 错误变体（`src-tauri/src/error.rs`，只新增，不改既有）

```rust
SidecarSpawn(String)       // "Python sidecar 启动失败：{0}"
SidecarHandshake(String)   // "Python sidecar 握手失败：{0}"
SidecarTimeout(String)     // "Python sidecar 调用超时：{0}"
SidecarProtocol(String)    // "Python sidecar 协议错误：{0}"
SidecarDegraded(String)    // "Python sidecar 降级：{0}"
SidecarUnavailable(String) // "Python sidecar 不可用：{0}"
```

`impl Serialize for Error` 已把 `to_string()` 序列化为字符串，跨 IPC 形状不变；新增变体**不需要**改 `Serialize`。

### 4.6 IPC 命令（`src-tauri/src/commands.rs`）

| 命令              | 签名                                                                      | 说明                 |
| ----------------- | ------------------------------------------------------------------------- | -------------------- |
| `sidecar_status`  | `fn sidecar_status(state: State<'_, AppState>) -> Result<SidecarStatus>`  | 纯读，含崩溃检出     |
| `sidecar_start`   | `fn sidecar_start(state: State<'_, AppState>) -> Result<SidecarStatus>`   | 幂等；失败降级为状态 |
| `sidecar_stop`    | `fn sidecar_stop(state: State<'_, AppState>) -> Result<SidecarStatus>`    | 幂等                 |
| `sidecar_restart` | `fn sidecar_restart(state: State<'_, AppState>) -> Result<SidecarStatus>` | 显式恢复入口         |

全部加入 `lib.rs` 的 `tauri::generate_handler![...]`（按字母序插在 `session_*` 与 `skills_*` 之间）。`AppState` 新增 `pub sidecar: Arc<SidecarManager>`；`setup()` 里用 `SidecarConfig::from_env()` 构造并 manage，**不主动 spawn**（懒启动：用户点「启动」或 F21 首次调用才拉起）。

## 5. 前端契约

`src/lib/ipc.ts` 新增（放在插件段之后、`skills_list` 之前或之后均可，按现有分区习惯）：

```ts
export type SidecarState = 'stopped' | 'ready' | 'degraded';

export interface SidecarRuntimeInfo {
  name: string;
  version: string;
  transport: string;
  python: string;
  protocolVersion: string;
  providers: string[];
  capabilities: string[];
  methods: string[];
}

export interface SidecarStatus {
  state: SidecarState;
  pid: number | null;
  runtime: SidecarRuntimeInfo | null;
  negotiatedCapabilities: string[];
  protocolVersion: string | null;
  lastError: string | null;
  restartCount: number;
}
```

- `normalizeSidecarStatus(raw: unknown): SidecarStatus`：**导出**，`unknown` 输入，逐字段安全默认（`null` / `[]` / `0` / `'degraded'`），未知 `state` 一律收敛为 `'degraded'`（fail-closed，不假装 ready）。
- `sidecarStatus()` / `sidecarStart()` / `sidecarStop()` / `sidecarRestart()`：走 `invokeCommand<unknown>('sidecar_*')` + normalize，返回 `IpcResult<SidecarStatus>`。
- `src/lib/ipc.test.ts` 必须有**同形状 payload 的 fixture test**（手工复刻 Rust `SidecarStatus` 的 camelCase 形状）+ 坏输入安全默认 + 命令名断言（`invokeSpy.mock.calls[0]?.[0]`）。注意 invoke spy 收 3 个参数，别断言参数个数。
- `src/features/settings/SettingsPage.tsx` 新增 `SidecarCard`（`data-testid="sidecar-card"`）：
  - **渲染路径零 IPC**：只在点击「刷新 / 启动 / 停止 / 重启」时调用；与本页其它卡片同一纪律。
  - 展示 `state` 徽标、`pid`、runtime 版本 / python 版本 / protocolVersion、协商能力、`restartCount`、`lastError`。
  - `degraded` 时显示警示文案「Python sidecar 不可用，主 Harness 功能不受影响」。
  - 组件内**禁止** `invoke()`，只能从 `@/lib/ipc` 导入。
  - `SettingsPage.test.tsx` 补对应用例（mock `sidecarStatus` 等）。

## 6. 测试纪律（AGENTS.md）

### TDD

每个 Task：先写失败测试 → 跑一次拿到 RED 证据（贴真实输出或至少记录失败信息）→ 写最小实现 → 跑 GREEN → 重构。

### 冷启动真路径

本阶段**不改 SQLite schema、不碰 registry bootstrap**，因此「真·空库」用例不适用；但必须保留一条**真·冷启动**用例：从 `Stopped`（`SidecarManager::new`，无任何夹具预置进程）调用 `start()` 走完 `spawn → handshake → Ready`，**不允许**用夹具直接注入 `Ready` 状态来冒充。

### 真机集成

`src-tauri/tests/`（或 `process.rs` 的 `#[cfg(test)]`）必须有至少一条用真实 `python -m harness_hub_runtime` 起进程的用例，覆盖：握手成功、`ping` 通、`shutdown` 后 `process_alive(pid) == false`。用 `#[cfg(windows)]` 的 `tasklist` / `#[cfg(not(windows))]` 的 `kill -0` 探活（不要写恒 true 的桩）。`python` 不存在时 `skip`（返回 Ok 而非 panic），与 `supervisor.rs::python_available()` 同一策略。

### 跨 IPC 契约

- Rust：`dto.rs` 里 `SidecarStatus` / `SidecarState` / `RuntimeInfo` 的 `serde_json::to_value` 逐键断言（含 `rename_all` 后实际键名、单元变体的字符串字面量、嵌套形状）。
- TS：`ipc.test.ts` 用同一形状 payload 的 fixture 锁住解析行为与安全默认值。

## 7. 提交与 PR 切分

按小 Task 拆 commit（Conventional Commits），按**互不重叠的写作用域**拆 PR：

| 切片                               | 写作用域（唯一）                                                                            | 分支                                     | PR 目标                      |
| ---------------------------------- | ------------------------------------------------------------------------------------------- | ---------------------------------------- | ---------------------------- |
| S0 规格与计划                      | `docs/specs/2026-11-04-*`、`docs/plans/2026-11-04-*`                                        | `feat/f20-sidecar-lifecycle`（集成基线） | —                            |
| S1 Python 协议                     | `python/**`                                                                                 | `f20/python-protocol`                    | `feat/f20-sidecar-lifecycle` |
| S2 Rust 核心                       | `src-tauri/src/sidecar/**`、`src-tauri/src/error.rs`、`src-tauri/tests/fixtures/sidecar/**` | `f20/rust-core`                          | 同上                         |
| S3 前端                            | `src/lib/ipc.ts`、`src/lib/ipc.test.ts`、`src/features/settings/**`                         | `f20/frontend`                           | 同上                         |
| S4 文档                            | `docs/CONTEXT.md`、`docs/CONTEXT-MAP.md`、`docs/ROADMAP.md`                                 | `f20/docs`                               | 同上                         |
| S5 Rust 接线（**依赖 S2 合并后**） | `src-tauri/src/commands.rs`、`src-tauri/src/lib.rs`                                         | `f20/rust-wiring`                        | 同上                         |

S1/S2/S3/S4 可完全并行（写作用域零交集，且 S3 只依赖命令名字符串、不依赖 Rust 编译产物）。S5 必须等 S2 合并进集成分支后再基于新基线开工。

每个切片至少 2 个 commit（测试 / 实现分离），最后 lead 合并全部 PR、跑完整质量门。

## 8. 已知风险

1. **SSH 通道被拦**：`git push` / `git fetch` 走 SSH 报 `Connection closed by 198.18.0.132 port 443`；`gh`（HTTPS API）与 `git fetch https://github.com/...` 正常。推送用 HTTPS 或 `gh`；见 skill `windows-host-git-ci` §3/§4，失败时循环重试并用 `git ls-remote` / `gh api` 核对远程。
2. **提交被吞**：宿主可能把分支引用重置回旧值。每次 `git commit` 后立刻 `git log --oneline -1` 核对；被吞时按 skill §2 用 tag 锚住 + 直接编辑 `packed-refs` + 按 sha 推送。
3. **远程 main 领先本地**：本次实测远程 main = `3ebcb2e`（PR #18），本地 `main` = `412cd4f`（落后 55 个提交）。所有 F20 分支必须基于 `3ebcb2e`，不要基于本地 `main`。
4. **Python 解释器路径**：打包态 `PYTHONPATH` 未定，F20 只保证 dev + `HARNESS_HUB_PYTHON` / `HARNESS_HUB_RUNTIME_ROOT` 覆盖；写入 CONTEXT 的「尚未实现」。
5. **Windows 黑框**：忘记 `CREATE_NO_WINDOW` 会在启动 sidecar 时弹控制台；必须设置。
6. **进程树泄漏**：`assign` 必须在 spawn 后立刻做；关闭时 Windows 走 `terminate_tree()`，否则 `child.kill()`。
7. **前端 invoke spy**：`invokeCommand` 调用 invoke 时传 3 个参数，断言命令名用 `mock.calls[0]?.[0]`，不要断言 `toHaveBeenCalledWith` 的完整参数列表。
8. **不要提交** `build-release-040/`（上一阶段遗留构建产物，未跟踪）与 stash 里的 `docs/releases/v0.4.0.md` 证据回填（另案处理）。
