# F20 Python Sidecar 生命周期 — 实施计划

- 日期：2026-11-04
- 规格：[docs/specs/2026-11-04-f20-sidecar-lifecycle-design.md](../specs/2026-11-04-f20-sidecar-lifecycle-design.md)
- 阶段：Track F / F20（目标 v0.5.0）
- 加速模式：多子 agent 并行 + 小 commit + 多 PR 合并 + goal 驱动（用户 m00140 要求）
- 集成分支：`feat/f20-sidecar-lifecycle` @ `3ebcb2e`（真实远程 main）
- goal：`goal-a37c0e13-7716-4b50-acca-acfcecd9fd84`（rev 2，maxGoalRounds=12）

## 前置确认（已实测）

| 项                   | 结果                                                                                    |
| -------------------- | --------------------------------------------------------------------------------------- |
| 分支                 | 从 `docs/v040-evidence` 切到 `feat/f20-sidecar-lifecycle`，已 `merge --ff-only 3ebcb2e` |
| 远程 main            | `3ebcb2e5df6c365c2af75c8336d8128afc6c4bd0`（gh API / HTTPS 实测），PR #52 已合并        |
| 本地 `main`          | `412cd4f`（**落后 55 提交，不用作基线**）                                               |
| 工具                 | Python 3.14.3 / uv 0.12.7 / cargo 1.98.1 / node v26.9.0 / pnpm 11.0.9 / gh 2.97.0       |
| `HARNESS_HUB_PYTHON` | 空 → 走 PATH 的 `python`                                                                |
| git 通道             | SSH 被 `198.18.0.132:443` 拦；`gh` + HTTPS fetch/push 正常                              |
| 工作树遗留           | `build-release-040/`（未跟踪，勿提交）、v0.4.0 证据回填已 `git stash`                   |

## 目标（对照 §8.1 验收清单）

| #   | 验收项                           | 交付物                                                 | 验证方式                                 |
| --- | -------------------------------- | ------------------------------------------------------ | ---------------------------------------- |
| 1   | 启动 / 关闭                      | `SidecarProcess::spawn/shutdown`、`sidecar_start/stop` | 真机集成测试 + 进程探活                  |
| 2   | stdio JSON-RPC handshake         | `initialize` 方法 + Rust `handshake()`                 | Rust 单测 + Python 单测 + 真机           |
| 3   | version / capability negotiation | `RuntimeInfo` DTO + 交集校验                           | DTO contract test + 不兼容降级用例       |
| 4   | crash 可恢复                     | `SidecarManager` 状态机 + `sidecar_restart`            | 状态机单测（真冷启动）+ 崩溃检出         |
| 5   | 无 Python 时主功能不受影响       | 失败一律收敛为 `SidecarStatus`，不返回 Err             | `python` 缺失路径单测 + 全量既有用例仍绿 |

## 提交切分

| 切片 | 分支                  | 写作用域                                                                                    | 依赖          |
| ---- | --------------------- | ------------------------------------------------------------------------------------------- | ------------- |
| S0   | 集成分支              | `docs/specs/2026-11-04-*`、`docs/plans/2026-11-04-*`                                        | —             |
| S1   | `f20/python-protocol` | `python/**`                                                                                 | 无，可并行    |
| S2   | `f20/rust-core`       | `src-tauri/src/sidecar/**`、`src-tauri/src/error.rs`、`src-tauri/tests/fixtures/sidecar/**` | 无，可并行    |
| S3   | `f20/frontend`        | `src/lib/ipc.ts`、`src/lib/ipc.test.ts`、`src/features/settings/**`                         | 无，可并行    |
| S4   | `f20/docs`            | `docs/CONTEXT.md`、`docs/CONTEXT-MAP.md`、`docs/ROADMAP.md`                                 | 无，可并行    |
| S5   | `f20/rust-wiring`     | `src-tauri/src/commands.rs`、`src-tauri/src/lib.rs`                                         | **S2 合并后** |

每个切片至少 2 个 commit：`test:` 先行（RED）→ `feat:`/`fix:` 实现（GREEN）。合并顺序 S1 → S2 → S3 → S4 → S5 → 质量门。

## 任务分解

### Task 0 — 规格与计划（lead，集成分支）

- 写 [spec](../specs/2026-11-04-f20-sidecar-lifecycle-design.md) 与本文，作为并行 agent 的固定契约。
- 建 4 个隔离 worktree（S1–S4），分支均从 `feat/f20-sidecar-lifecycle` 切出。
- `docs: add F20 sidecar lifecycle spec and plan` 一个 commit。

### Task 1 — Python 握手协议（S1，写作用域 `python/**`）

**先 RED**：在 `python/tests/test_rpc_server.py` 新增用例并确认失败：

- `initialize` 返回 `protocolVersion` / `runtime` / `capabilities` / `methods` 四键，且 `runtime` 与 `runtime.info` 同形状。
- `requiredCapabilities` 含未知能力 → 返回 `INVALID_REQUEST`（-32600）错误响应，错误信息列出缺失项。
- `protocolVersion` 主版本不匹配 → `INVALID_REQUEST`。
- `shutdown` 请求返回 `{"stopping": true}` 且 `serve()` 退出；`shutdown` 通知无回包且退出。
- `serve()` 遇 EOF 正常返回、不抛异常。
- `__version__ == "0.5.0"`。

**实现**：`python/harness_hub_runtime/rpc/server.py` 增 `initialize` / `shutdown` handler，抽出 `build_runtime_info()` 单一构造函数供 `runtime_info` 与 `initialize` 共用；`serve()` 识别 `shutdown` 停止循环、EOF 正常返回；`__init__.py` 升版本。

**RED 证据命令**：`pnpm python:test` 或 `uv run --project python python -m unittest discover -s python/tests -t python -v`。

**提交**：`test: cover F20 sidecar handshake contract` / `feat: negotiate runtime capabilities in Python sidecar`。

### Task 2 — Rust DTO 与错误变体（S2 第一个 commit 对）

**先 RED**：`src-tauri/src/sidecar/dto.rs` 的 contract test：

- `serde_json::to_value(&SidecarStatus{...})` 逐键断言 camelCase 键名与嵌套形状（见 spec §4.1 的 JSON）。
- `SidecarState::Stopped/Ready/Degraded` 分别序列化为 `"stopped"` / `"ready"` / `"degraded"`。
- `RuntimeInfo` 的 `protocolVersion` 键名（不能是 `protocol_version`）。

`src-tauri/src/error.rs` 新增 6 个 `Sidecar*` 变体；加一个测试断言新变体的 `to_string()` 与 `serde_json::to_value` 为字符串。

**提交**：`test: lock F20 sidecar DTO serialization contract` / `feat: add sidecar DTOs and error variants`。

### Task 3 — Rust 进程监督器（S2）

**先 RED**：`src-tauri/src/sidecar/process.rs` 单测 + `src-tauri/tests/` 真机用例：

- 用真实 `python -m harness_hub_runtime` 起进程，`handshake()` 拿到 `RuntimeInfo`；`ping()` 返回 true；`shutdown()` 后 `process_alive(pid) == false`。
- `python` 不存在时 `skip`（`python_available()`），不 panic。
- 握手超时 / stdout 关闭 → `SidecarTimeout` / `SidecarDegraded`，且进程被杀。
- 坏行不杀读线程；stderr 64 KiB 封顶。
- `SidecarConfig::from_env()`：`HARNESS_HUB_PYTHON` 覆盖优先、`PYTHONPATH` 用 `runtime_root`。

**实现**：按 spec §4.2 / §4.3，复用 `crate::plugins::rpc`；Windows `CREATE_NO_WINDOW` + `Containment` 立即 assign + `terminate_tree()`。

**提交**：`test: cover sidecar process lifecycle with real python` / `feat: supervise Python sidecar process over stdio JSON-RPC`。

### Task 4 — Rust 管理器（S2）

**先 RED**：`src-tauri/src/sidecar/manager.rs` 单测：

- **真冷启动**：`SidecarManager::new` → `status() == Stopped` → `start()` 走完 spawn+handshake → `Ready`（用真实 python；缺失则 skip）。
- 进程被外部 kill 后 `status()` 检出 → `Degraded`，`restart()` → `Ready`，`restart_count >= 1`。
- `python` 路径指向不存在文件：`start()` **不返回 Err**，返回 `Degraded` 且 `last_error` 非空（验收 #5）。
- `stop()` 幂等；`start()` 在 Ready 时幂等不重复 spawn。
- 所有方法不 panic（锁中毒兜底）。

**实现**：按 spec §4.4 状态机。

**提交**：`test: cover sidecar manager cold start and crash recovery` / `feat: add sidecar manager with degradation and recovery`。

### Task 5 — 前端契约与状态面板（S3，写作用域 `src/**`）

**先 RED**：`src/lib/ipc.test.ts` 新增 fixture test（手工复刻 Rust `SidecarStatus` 的 camelCase payload）+ 安全默认 + 命令名断言；`SettingsPage.test.tsx` 加 `sidecar-card` 渲染 / 按钮 / degraded 文案用例。

**实现**：`src/lib/ipc.ts` 增类型 + `normalizeSidecarStatus` + 4 个 wrapper；`SettingsPage.tsx` 增 `SidecarCard`（渲染零 IPC、组件内不 invoke）。

**提交**：`test: lock F20 sidecar IPC decoder and settings card` / `feat: add sidecar status card to settings`。

### Task 6 — 文档（S4，写作用域 `docs/CONTEXT*.md`、`docs/ROADMAP.md`）

- `docs/CONTEXT.md` §6「尚未实现」：移除/标注 F20 已实现项，新增「打包态 PYTHONPATH 未定」。
- `docs/CONTEXT-MAP.md` 第 19 行条目补 `sidecar/{dto,config,process,manager}.rs` 与 IPC 命令名。
- `docs/ROADMAP.md` F20 行标状态（按仓库既有标记习惯）。

**提交**：`docs: record F20 sidecar lifecycle implementation`。

### Task 7 — Rust 接线（S5，**等 S2 合并**）

**先 RED**：`src-tauri/src/commands.rs` 的跨 IPC contract test（4 个命令返回的 `SidecarStatus` 形状）。

**实现**：`AppState` 加 `sidecar: Arc<SidecarManager>`；`setup()` 构造（不主动 spawn）；4 个命令；`invoke_handler!` 按字母序加入；`RunEvent::Exit` 显式 `shutdown()`。

**提交**：`test: lock sidecar command IPC shapes` / `feat: wire sidecar manager into AppState and commands`。

### Task 8 — 全质量门（lead，集成分支）

合并全部 PR 后，按 AGENTS.md「完成前」逐项跑并**贴真实输出**：

```text
pnpm format:check && pnpm rust:fmt:check
pnpm lint
pnpm typecheck
pnpm test && pnpm rust:test && pnpm python:test
pnpm build
pnpm rust:clippy
```

任一失败即未完成，先修再报。

## 完成门记录

> 以下为**最终集成态**（集成分支 `feat/f20-sidecar-lifecycle`，HEAD `2050c3d` = S0+S1+S2+S3+S4+S5+S6 全部合并后）在 `D:\HarnessHub` 上**已实测**的真实输出。远古 mid-integration 记录（`4abc99b`）已被本节取代。

| 门            | 命令                  | 结果 | 输出                                                                                               |
| ------------- | --------------------- | ---- | -------------------------------------------------------------------------------------------------- |
| format        | `pnpm format:check`   | PASS | `All matched files use Prettier code style!`                                                       |
| rust:fmt      | `pnpm rust:fmt:check` | PASS | `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check` 无输出，`EXIT=0`                 |
| lint          | `pnpm lint`           | PASS | `✖ 4 problems (0 errors, 4 warnings)`（见下注，仅既存警告）                                        |
| typecheck     | `pnpm typecheck`      | PASS | `$ tsc --noEmit` 无输出，`EXIT=0`                                                                  |
| unit (web)    | `pnpm test`           | PASS | `Test Files 11 passed (11)` / `Tests 419 passed (419)`，Duration 38.67s                            |
| unit (rust)   | `pnpm rust:test`      | PASS | 全绿运行 `test result: ok. 605 passed; 0 failed; 2 ignored`（lib）+ 全部集成测试/Doc-tests 均 `ok` |
| unit (python) | `pnpm python:test`    | PASS | `Ran 25 tests in 0.001s` / `OK`                                                                    |
| build         | `pnpm build`          | PASS | `✓ built in 7.15s`（`dist/assets/index-D-wZPhtv.js 482.68 kB`）                                    |
| clippy        | `pnpm rust:clippy`    | PASS | `Finished dev profile ... in 8.27s`                                                                |

注：`pnpm lint` 的 4 条为**既存** react-refresh/only-export-components 警告（`src/app/routes.tsx:24`、`:29`，`src/features/dashboard/DashboardPage.tsx:392`、`:400`），0 errors，属 PASS；`pnpm python:test` 命令为 `uv run --project python python -m unittest discover -s python/tests -t python -v`。

### 环境插曲 / 已知风险（最终）

1. **预置 Windows 进程树测试间歇性 flake，与 F20 无关**：`src-tauri/tests/process_tree_ownership.rs` 的 `terminate_tree_scopes_to_one_owned_tree_without_any_release` / `killing_one_session_tree_never_touches_another`，以及 `src-tauri/src/pty/input_tests.rs` 的 `worker_failure_closes_input_and_keeps_control_paths_alive` / `terminal::tests::natural_exit_finishes_the_session_and_emits_exit_code`，在 Windows 真机重负载下会间歇性失败。决定性证据：
   - `git diff 3ebcb2e HEAD -- src-tauri/src/pty/ src-tauri/tests/process_tree_ownership.rs` 为**空**，F20 从未改动这些文件、pty 模块或 `src-tauri/Cargo.toml`/`Cargo.lock`；
   - 同一时刻交替 A/B 对照：`BASELINE(3ebcb2e，无 F20 代码)` 单独跑 `--test process_tree_ownership` 也 `FAILED. 7 passed; 1 failed` 一次，`HEAD(2050c3d)` 两次全过 → 失败不依赖 F20 代码；
   - CI 的 rust job 跑在 `ubuntu-latest`，而 `process_tree_ownership.rs` 是 `#![cfg(windows)]`，CI 根本不执行该测试。
   - `pnpm rust:test` 首次全绿（`2050c3d` 合并当次），复跑 4 次里第 4 次拿到干净全绿 `EXIT=0`；F20 自身 `src-tauri/tests/sidecar_lifecycle.rs` 真机冷启动 6 用例独立跑始终稳定 `ok`。结论：属既存环境敏感型 flake，不阻断 F20 交付，记录于此以便后续单独治理。
2. `src-tauri/src/error.rs` F20 sidecar error-variant 块原先多一空行的 rustfmt 差异已在 `50637fc` 一并修复，最终 `rust:fmt:check` 全绿。
3. **Linux CI 首轮失败，根因是测试探针而非生产代码**：最终 PR #59 的 `Rust Control Plane` job 在 `ubuntu-latest` 报 `crash_is_detected_as_degraded_and_restart_recovers ... FAILED`（`外部 kill 后进程 7390 仍存活`）。Linux 的 `kill -0 <pid>` 无法区分存活与僵尸：崩溃恢复用例从外部 `kill -9` sidecar 子进程，Rust 父进程 `SidecarProcess` 尚未 `try_wait()` 回收，子进程保持僵尸态，`kill -0` 一直成功，`wait_until_dead` 5s 超时。Windows 的 `tasklist` 无僵尸语义故本地必过。修复：`#[cfg(not(windows))] process_alive()` 先读 `/proc/<pid>/status`，把 `State: Z (zombie)` 判为已死亡；生产代码 `SidecarProcess::is_alive()` 用 `try_wait()` 本就正确，未改。等价片段经 `rustc --edition 2021` 编译验证，Windows 侧 `cargo test --test sidecar_lifecycle` 6/6 全绿 + clippy/fmt 全绿；最终以 CI ubuntu 全绿为权威证据。

## 已知风险

1. SSH 被拦 → 用 HTTPS / `gh`，循环重试 + 远程核对（skill `windows-host-git-ci` §3/§4）。
2. 提交被吞 → 每次 commit 后 `git log --oneline -1` 核对；被吞走 tag + packed-refs + 按 sha 推送（§2）。
3. 基线错位 → 一律基于 `3ebcb2e`，不碰本地 `main`。
4. `git reset --hard` 被宿主安全网拦截 → 用 `git stash` + `git merge --ff-only` 代替（已实测）。
5. 打包态 Python 资源定位未定 → F20 只保证 dev + env 覆盖，写入 CONTEXT「尚未实现」。
6. 前端 invoke spy 收 3 个参数 → 断言 `mock.calls[0]?.[0]`，别断言完整参数列表。
7. 并行写冲突 → 严格按切片写作用域，S5 等 S2 合并；`src-tauri/src/error.rs` 只归 S2，`src-tauri/src/commands.rs` / `lib.rs` 只归 S5。
8. 勿提交 `build-release-040/` 与 stash 中的 v0.4.0 证据回填。
