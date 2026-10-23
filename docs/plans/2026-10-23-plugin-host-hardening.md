# 插件宿主边界修复计划

- 日期：2026-10-23
- 规格：`docs/specs/2026-10-23-plugin-host-hardening-design.md`
- 当前基线：`8f7de37`，`feat/plugins-host`，PR #13；两项修复分别走独立 PR。
- 保留未跟踪的 `docs/handoffs/`，不纳入提交。

## Task 1：stderr 硬上限

1. 先添加单行超长、无换行实时捕获、非法 UTF-8、多字节跨块与末尾行选择的失败测试。
2. 固定大小块读取，字节环形缓冲只留最后 64 KiB，显示时容错解码。
3. 修正旧 flood 测试实际未通过 capability 协商、未触发输出的问题。
4. 运行插件 supervisor 测试，独立提交测试与实现。

## Task 2：Usage 身份与失败状态

1. 从空库经生产 refresh 注册插件；先复现目录错配、manifest id 改动与调用失败仍 ready。
2. 在 spawn 前核对登记路径与 manifest id；匹配后的插件失败标记 degraded。
3. 覆盖健康插件不受错配影响、空结果合法、refresh 恢复，以及现有幂等导入。
4. 更新 SDK 的这两项行为并独立提交。

## Task 3：完整验收与 PR

执行 format、Rust fmt、lint、typecheck、web/Rust/Python tests、build、clippy；逐项保留真实输出与失败信息。确认 diff 范围，推送现有 PR #13 并核对 CI。不合并 PR。

## 执行证据

### Task 1：RED → GREEN

`cargo test --manifest-path src-tauri/Cargo.toml --lib plugins::supervisor::tests -- --test-threads=1`

- RED：`6 passed; 3 failed`。单行缓存达到 `196616` 字节；无换行与非法 UTF-8 两例报 `stderr 尾部未交付`。
- GREEN：`9 passed; 0 failed`（11.13s）。旧 flood 夹具改为协商通过的 `usage/import`，强制断言 `flooded = true`，确认确实产生了输出。

### Task 2：RED → GREEN

`cargo test --manifest-path src-tauri/Cargo.toml --test real_plugins usage_import -- --test-threads=1`

- RED：`3 passed; 4 failed`。两个身份错配都返回 `Ok(6)`；启动失败和坏载荷后状态仍为 `ready`，期望 `degraded`。
- 正向对照：合法空结果、SQLite 导入失败不降级、原有幂等导入均通过。
- 首次 GREEN：`7 passed; 0 failed`（3.57s）。
- 独立审阅指出不能把零事件当空报告；新增零 token 非空会话用例得到 `0 passed; 1 failed`（审计行 0，期望 1），修正为依据 `sessions_seen` 早退。
- 补充真实方法调用后崩溃、显式 refresh 恢复的闭环。最终 `real_plugins`：`20 passed; 0 failed`（6.04s），含 6 条 common 辅助测试；全部从生产路径建立登记。

### 完成门（2026-10-23，本地首次完整运行）

原始日志：`test-results/2026-10-23-hardening/`（忽略的本机产物）。Rust 编译设 `CARGO_BUILD_JOBS=2`，测试使用原始 `pnpm rust:test`，未降低测试并发或跳过失败。

| Gate                  | 真实输出（退出码均为 0）                                                               |
| --------------------- | -------------------------------------------------------------------------------------- |
| `pnpm format:check`   | `All matched files use Prettier code style!`                                           |
| `pnpm rust:fmt:check` | 无输出，静默通过                                                                       |
| `pnpm lint`           | `4 problems (0 errors, 4 warnings)`，既有 Fast Refresh 告警                            |
| `pnpm typecheck`      | `tsc --noEmit`，无诊断                                                                 |
| `pnpm test`           | `Test Files 9 passed (9)` / `Tests 179 passed (179)`                                   |
| `pnpm rust:test`      | lib `438 passed; 0 failed`；所有集成测试目标 `0 failed`，`real_plugins` 为 `20 passed` |
| `pnpm python:test`    | `Ran 12 tests in 0.001s` / `OK`                                                        |
| `pnpm build`          | `1909 modules transformed` / `built in 8.29s`                                          |
| `pnpm rust:clippy`    | `Finished dev profile ... in 3m 00s`，`-D warnings`                                    |

注意：外部 CLI 集成测试有环境前置条件，测试命令成功不等于所有 Harness 的认证/chat 或 GUI 均验收通过。插件的新增用例有本机 Python，定向运行实际启动了子进程。

独立 PR：stderr 为 #14，Usage 身份与降级为 #15，均基于插件宿主 #13。
