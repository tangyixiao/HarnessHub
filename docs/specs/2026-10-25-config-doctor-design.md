# Config Doctor（ROADMAP E17）Spec

## Problem

规划 §4.26：用户需要回答「我的配置现在健康吗」——配置文件可解析吗？
凭据可达吗？投影后有没有漂移？**只读诊断，默认只给修复建议；
只有显式 Fix 才修改**（本阶段只做诊断 + 建议，Fix 走 E15 已有的投影/恢复）。

## Locked Goal

一个 `config_doctor(harness_id)` IPC：对单个 harness 输出一组 `CheckItem`
（status: ok / warn / error + 人话 message + 可选建议），Settings 页逐项展示
（✓ / ⚠ / ✗）。

## Acceptance Criteria

- [ ] `src-tauri/src/profile/doctor.rs`：`CheckItem { check, status, message,
      suggestion }`（camelCase；status ∈ {ok, warn, error}）。
- [ ] 检查项（claude / codex / gemini 各自适配，复用 E14 解析器）：
  - `config_parseable`：E14 discover 成功 = ok；语法错误 = error；
    文件不存在 = warn（未初始化）。
  - `credentials_reachable`：E16 credential_refs 有该 harness 的句柄 = ok；
    无句柄 = warn（未登记凭据）。store 侧孤儿（句柄在值不在）= error。
  - `provider_pointed`：E14 fields 含 BASE_URL（claude）/ model_provider
    （codex）/ key set（gemini）= ok；否则 = warn（用默认 provider）。
- [ ] IPC `config_doctor(harness_id) -> Vec<CheckItem>`；**纯读**（ADR-0012）。
- [ ] 前端 Settings 页「配置体检」区：harness 按钮 → CheckItem 列表
  （✓/⚠/✗ 图标 + message + suggestion）。无 Fix 按钮（本阶段）。
- [ ] 测试：doctor 逻辑（每种 status 至少一条触发路径）+ 契约 + 前端测试。

## Non-goals

- 不做自动修复（Fix 是显式动作，走 E15 投影/恢复，后续迭代）；
- 不做跨 harness 体检报告聚合；不做 MCP / Skills 检查（E18）。

## Decision

复用 E14/E16 的现有读路径做体检——不重复实现配置解析；
孤儿检测（句柄在 store 不在）是唯一需要额外 IO 的检查。

## Verification

单测 + 契约 + 前端 + 全量门。
