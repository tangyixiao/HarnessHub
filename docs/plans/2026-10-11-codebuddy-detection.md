# CodeBuddy CLI 检测接入实施计划

规格：[codebuddy-detection-design](../specs/2026-10-11-codebuddy-detection-design.md)。
基线：`9e6deb4`（PR #8 合并后），分支 `feat/codebuddy-detection`。

## Task 1：RED —— 注册与契约断言先行

- `two_harness_inventory.rs`：生产注册/冷启动断言从三 Harness 改为四
  （顺序 codex、claude、gemini、codebuddy），保留原有矩阵与首次发现时间检查。
- `harness-colors.test.ts`：已知色相集合加入 codebuddy（5 → 6 个互异色相）。
- 运行确认失败（无适配器、无身份色）。

## Task 2：适配器与 fixture（RED → GREEN）

- `fixtures/codebuddy/`：version.txt（真实输出 `2.161.0`）+ summary.json
  （全 false capabilities，与 Rust 序列化同形）。
- `codebuddy.rs`：先加单测（detect / 未知版本 / 目录存在与缺失 / 全 false
  能力 / launch 未实现 / fixture 契约），确认编译失败后补最小实现。
- 组合根 `lib.rs` 注册第四个适配器（home 直发现，无 env 覆盖）。
- `ipc.test.ts` 消费同一 fixture（normalize 契约 + 安全默认值）。
- Expected：Rust/TS 全绿。

## Task 3：身份色与真机

- `harness-colors.ts` 增加 codebuddy 色相；测试转绿。
- 新增 `codebuddy_detection.rs` 真机只读检测（未安装显式 SKIP）。
- 本机真机验证 `codebuddy --version` 与数据目录发现。

## Task 4：门禁、文档与提交

- 同步 `CONTEXT.md`、`CONTEXT-MAP.md`、`lib.rs` 注释、`adapters/mod.rs`。
- 全部完成门命令 + `git diff --check`；真实 GUI 验证 Harnesses 页四行 +
  Terminal 无 CodeBuddy 入口。
- 一件事一提交（docs + feat）；不推送限制解除后按用户流程 PR + 合并。

## 已知风险

1. `--version` 的输出解析：`2.161.0` 为纯数字串，复用现有解析即可。
2. 真机测试在本机可跑（已安装）；CI 无 codebuddy 时必须显式 SKIP。
3. 别名 shim（codebuddy-code / cbc）不参与检测：只认 `codebuddy` 一个名字。

## 执行记录

- Task 1：完成。RED 确认 —— 注册/冷启动/追加同步三条断言失败
  （`left: ["codex","claude","gemini"]` vs `right: […,"codebuddy"]`）；
  身份色集合测试失败（5 ≠ 6）。
- Task 2：完成。适配器 8 条单测全绿（detect / 版本未知 / 目录存在与缺失 /
  home 缺失 / 全 false / launch 拒绝 / fixture 契约）；fixture 的 dataPaths
  按平台无关原则置空（目录行为由 FakeHostProbe 单测覆盖，与 gemini fixture
  同惯例）；TS 契约 +1（134 passed）。
- Task 3：完成。`harness-colors.ts` 增加 `codebuddy: 190`（青蓝）；
  真机检测通过：`codebuddy.cmd` / `2.161.0` / `C:\Users\tangy\.codebuddy` /
  capabilities 全 false（`codebuddy_detection.rs` 1 passed）。
- Task 4：门禁全绿（2026-10-11 实测）：format / rust fmt / lint（0 errors /
  4 既有 warnings）/ typecheck / `pnpm test`（134，CodeBuddy fixture 契约 +1）/
  `rust:test`（21 个 binary 全 ok）/ python（OK）/ clippy -D warnings
  （修复一个测试模块内缺失的 `PathBuf` import 后通过）/ build /
  `git diff --check`。GUI 验证：Harnesses 页四行齐全（CodeBuddy 2.161.0、
  `~/.codebuddy` 发现、全 false 能力矩阵、青蓝头像），Terminal 下拉只有
  codex / claude / gemini（CodeBuddy 被能力过滤正确排除）。
  截图 `codebuddy-harnesses.png`（仓库外）。
- 提交：docs(spec) + feat 两个提交；推送 + PR + 合并。
