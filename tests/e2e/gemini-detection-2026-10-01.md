# Gemini CLI 检测阶段验收（2026-10-01）

基线 `906102e`，分支 `codex/gemini-detection`。以下均为本轮在 Windows 当前源码上
执行的结果；不复用旧 Task 7 的门禁输出。

## TDD 证据

先运行新增断言，未接实现时：

```text
pnpm test -- src/features/terminal/TerminalPage.test.tsx
Tests  2 failed | 22 passed (24)
expected document not to contain element, found <option value="gemini@local">

cargo test --manifest-path src-tauri/Cargo.toml --test two_harness_inventory
test result: FAILED. 3 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out
left: ["codex", "claude"]
right: ["codex", "claude", "gemini"]
```

Gemini 单测先加入，缺失实现时确认 `cannot find type GeminiCliAdapter`；随后补最小
adapter、生产注册与 capability 过滤。初次前端重跑发现旧 Terminal fixture 的
`capabilities: {}` 没有表达已实现的 Codex 能力，将它补齐后相关 101 条测试通过。
最终增加安全默认值用例并执行全部门禁。

## 真机检测

`gemini.cmd --version` 退出码 0，stdout：

```text
0.59.0
```

`cargo test --manifest-path src-tauri/Cargo.toml --test gemini_detection -- --nocapture`
实际检测 binary 为 `D:\npm-global\gemini.cmd`，版本 `0.59.0`，发现本机 home 下
`.gemini` 目录，capabilities 全 false：

```text
test the_real_machine_detects_gemini_without_claiming_runtime_support ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.19s
```

这个真机用例没有走 SKIP。缺少 Gemini 的机器会显式打印 SKIP，不可当作真机通过。

## 全部门禁

下列命令本轮实际执行，退出码均为 0：

| 命令                  | 结果                                                             |
| --------------------- | ---------------------------------------------------------------- |
| `pnpm format:check`   | `All matched files use Prettier code style!`                     |
| `pnpm rust:fmt:check` | 通过，无错误输出                                                 |
| `pnpm lint`           | `4 problems (0 errors, 4 warnings)`                              |
| `pnpm typecheck`      | `tsc --noEmit`，无错误输出                                       |
| `pnpm test`           | 6 files，129 tests passed                                        |
| `pnpm rust:test`      | 295 unit + 54 integration passed；各 binary 0 failed / 0 ignored |
| `pnpm python:test`    | 12 tests，OK                                                     |
| `pnpm build`          | `✓ built in 3.11s`                                               |
| `pnpm rust:clippy`    | `Finished dev profile ... in 5.25s`，`-D warnings` 通过          |
| `git diff --check`    | 通过，无错误输出                                                 |

输出节选：

```text
Test Files  6 passed (6)
     Tests  129 passed (129)

test result: ok. 295 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.72s

Ran 12 tests in 0.003s
OK
```

Rust 集成总数 54 由 14 个测试 binary 的实际通过数相加得到。完整 Rust 输出保存在
本机临时目录 `harnesshub-gemini-20261001/rust-test.log`，不提交日志。
lint 的 4 条 warning 来自已有 `routes.tsx` 与 `DashboardPage.tsx` 的 Fast Refresh
导出规则，本次没有修改这两个文件。构建同时输出 Vite 的 PLUGIN_TIMINGS 性能提示。

## 自审与证据边界

- 生产冷启动用空库 + `ensure_local_target` + 生产注册表 + `reconcile_harnesses`，
  断言三条定义与安装；再次同步幂等并保留既有 Codex `first_detected_at`。
- 新 adapter 复用 `HostProbe`，没有新探测抽象、schema migration 或 IPC DTO。
- Rust serialization 与 TS normalize 读取同一 `fixtures/gemini/summary.json`；
  Harnesses 页面测试显示版本与全 false 矩阵，Terminal 回归测试覆盖缺任一必要能力。
- 本次只证明检测、注册、存储同步与组件行为。没有启动 Gemini TUI、验证认证、
  请求模型或导入 Usage；Gemini 的九项能力仍未实现，不宣称完整支持。
- 没有进行桌面 GUI 人工验收或独立代理复审；本次 review 是作者自审。
- 既有未跟踪 handoff 保留在工作区，不进入提交。未推送、创建 PR、合并或改 tag。
