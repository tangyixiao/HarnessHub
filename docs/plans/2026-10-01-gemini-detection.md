# Gemini CLI 检测接入实施计划

规格：[gemini-detection-design](../specs/2026-10-01-gemini-detection-design.md)。
基线：`906102e`，分支 `codex/gemini-detection`；既有未跟踪 handoff 保留且不提交。

## Task 1：生产注册与前端选择契约（RED）

- 更新 `two_harness_inventory.rs` 的生产注册/冷启动断言为三 Harness，保留原有
  Codex/Claude 矩阵与首次发现时间检查。
- 增加 Terminal 已安装但能力不足的 fixture 用例。
- 运行 `pnpm test -- src/features/terminal/TerminalPage.test.tsx` 与
  `cargo test --manifest-path src-tauri/Cargo.toml --test two_harness_inventory`。
- Expected：过滤测试失败；注册/冷启动数量断言失败，证明新行为尚不存在。

## Task 2：检测、fixture、最小实现（RED → GREEN）

- 新增 `fixtures/gemini/` 脱敏版本输出及 HarnessSummary JSON。
- `gemini_cli.rs` 先加测试，覆盖 detect、未知版本、目录覆盖、全 false 能力与
  未实现 launch；先运行并确认缺失实现错误，再补实现。
- 复用 `HostProbe`；组合根注入配置根并注册第三个 adapter。
- 在 Terminal 原有过滤条件增加 launch/terminal，不改启动编排。
- Rust/TS 读取同一 JSON fixture；Harnesses 页面验证展示；真机 integration test
  仅探测版本、不读取认证或发送请求。
- Expected：相关 Rust/TS 测试全绿；真机已安装时打印实际路径与版本。

## Task 3：门禁、证据与本地提交

- 同步 `CONTEXT.md`、`CONTEXT-MAP.md`、adapter 模块注释的检测阶段说明。
- 运行 format、rust fmt、lint、typecheck、TS/Rust/Python tests、build、clippy；
  `git diff --check`；审阅实际 diff，记录失败/跳过/既有 warning 边界。
- Expected：全部退出码 0；有失败就继续修复并重跑相关门禁。
- 只暂存本任务文件，制作一件事的本地 `feat:` 提交；不推送、不开 PR、不合并或改 tag。

## 自审

本计划复用冻结接口与现有 schema，无新增 runtime/IPC 抽象；冷启动不使用 seeded
fixture；目录仅发现不读内容；检测阶段不会进入 Terminal，能力不受机器安装影响。

## 执行记录

- Task 1：完成；Terminal 两条断言先失败，注册/冷启动/追加同步三条断言先失败。
- Task 2：完成；缺失 GeminiCliAdapter 的编译失败已确认后实现；共享 fixture、
  adapter 单测、组件测试与生产冷启动通过；真机识别 `gemini.cmd`、`0.59.0`。
- Task 3：门禁通过：TS 129、Rust 295 unit + 54 integration、Python 12；格式、lint
  （0 errors / 4 个既有 warnings）、类型检查、build、clippy、diff check 均退出 0。
- 夹具修正：旧 Terminal 测试省略 capabilities，改为明确表达已实现的能力；没有
  放宽生产过滤规则。作者自审，没有独立代理 review。
- 真实输出与限制见 [验收记录](../../tests/e2e/gemini-detection-2026-10-01.md)。
