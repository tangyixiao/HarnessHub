# GitHub Copilot CLI 检测（ROADMAP D12 第一格）Spec

## Problem

ROADMAP D12：按 Adapter 成本逐步增加 harness。GitHub Copilot CLI 在本机已安装
（`D:\npm-global\copilot.cmd`），ccusage 已把它列为数据源之一，但 Hub 完全看不到它。

## Locked Goal

Copilot CLI 出现在 Harnesses 页：安装状态、binary 路径、版本、数据目录；
能力矩阵全 false（ADR-0005 零宣称），Terminal 不提供启动入口。

## Acceptance Criteria

- [x] `src-tauri/src/harness/adapters/copilot.rs`：detect（PATH `copilot` +
      `~/.copilot` 存在性）；未装 → `installed=false`、data_paths 仍如实列出。
- [x] 版本解析：真机输出 `GitHub Copilot CLI 1.0.91.`（尾点 + 次行 update 提示）
      ——`parse_version` 剥首尾非字母数字饰边，回归测试锁定。
- [x] fixture：`fixtures/copilot/{version 输出原文（内嵌于测试）, summary.json}`；
      contract test（summary 序列化 == fixture）。
- [x] 真机集成测试 `tests/copilot_detection.rs`（本机过；未装机器 SKIP）。
- [x] 注册进 `build_harness_registry()`；能力矩阵断言（two_harness_inventory 矩阵
      同步更新，copilot 各能力保持 false）。

## Non-goals

- 不做 launch / terminal / usage（后续按真机取证逐级加）。
- 不读 `~/.copilot` 任何内容（只做目录存在性发现）。

## Decision

照 CodeBuddy 检测模板（最小准入：fixture + contract + 真机测试，ADR-0022）。
发现到的意外事实：`1.0.91.` 尾点污染 semver 提取——修在共享 `parse_version`
（所有 harness 受益），不在适配器里特判。

## Evidence

- 真机：`copilot --version` → `GitHub Copilot CLI 1.0.91.`（次行 update 提示）；
  `~/.copilot` 含 ide / logs / servers / session-state / skills。
- `cargo test --lib copilot`：7 passed；真机集成测试通过（binary/version/data 全中）。

## Verification

全量质量门 + CI。
