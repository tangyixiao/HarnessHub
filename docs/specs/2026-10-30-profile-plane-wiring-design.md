# Profile Plane IPC/UI 接线收尾（E14-E17）Spec

## Problem

E15（projection.rs）/ E16（credentials/）只有 Rust 库层，**没有 IPC 与 UI**；
E17 的 `config_doctor` 命令已注册但**没有 TS 面与 UI**，且缺 spec 承诺的
`credentials_reachable` 检查项。v0.4 里程碑（E18/E19 建在其上）要求这些
能力真实可用，不是库函数存在。

## Locked Goal

Settings 页从「只读展示」升级为完整的配置平面：

1. **投影（E15）**：claude 支持编辑 `env.*` 键 → confirm（文案含目标文件路径）
   → `profile_project` 走 read→parse→merge→backup→temp→verify→rename 管线
   → 展示 ProjectionReport；备份列表 + 显式恢复。
2. **凭据（E16）**：登记（label + 值，值只进不出）/ 列表（仅句柄元数据）/
   删除（确认后先删 store 值再删行）。
3. **体检（E17）**：harness 按钮 → CheckItem 列表（✓/⚠/✗ + message + suggestion）；
   补上 `credentials_reachable` 检查（命令层合并，diagnose 保持无 DB 纯函数）。
4. **capability**：`HarnessCapabilities` 增加 `profiles` 位（ADR-0011 修订：
   投影已实现 + 测试回归才为 true）；claude = true，其余 false。

## Acceptance Criteria

- [ ] IPC：`profile_project(harness_id, fields)` / `profile_backups(harness_id)` /
      `profile_restore(harness_id, backup_id)`（仅 claude；其余 harness 明确报错）。
- [ ] IPC：`credential_register(harness_id, label, value)` /
      `credential_list(harness_id)` / `credential_remove(id)`（列表与记录**永不含值**）。
- [ ] `config_doctor` 增加 `credentials_reachable`：有句柄且 store 值在 = ok；
      有句柄值不在（孤儿）= error；无句柄 = warn。命令层合并进 CheckItem 列表。
- [ ] Rust DTO 契约测试：`ProjectionReport` / `BackupInfo` / `CredentialRefRecord` /
      `CheckItem` 的 JSON 键名锁定（camelCase，AGENTS.md 跨 IPC 纪律）。
- [ ] `HarnessCapabilities` 契约测试新增 `profiles` 键；claude 适配器翻 true 有注释证据。
- [ ] TS：`normalizeProjectionReport` / `normalizeBackupInfo` /
      `normalizeCredentialRef` / `normalizeCheckItem` + fixture 测试（同形状 payload
      锁定解析 + 安全默认值）；typed wrappers 走 `invokeCommand`。
- [ ] Settings 页三个新区块（投影 / 凭据 / 体检），全部显式 confirm，
      渲染路径零 IPC（按钮触发才调用）。
- [ ] 迁移：无（0012 已存在）。

## Non-goals

- codex / gemini 投影（E15 范围本来就只有 claude；逐个加）。
- OS keychain 真机验收（`FileSecretStore` 兜底不变）。
- Profile 多套存储与切换（后续 E 迭代）。

## Decision

- `profile_restore` 在命令层按 id 查备份列表再调用 `projection::restore`，
  不把 `BackupInfo` 当 IPC 参数传（防止前端伪造 target_path 任意写文件——
  恢复目标永远来自后端自己的备份目录）。
- `credentials_reachable` 在命令层合并：`diagnose(home, harness_id)` 保持
  无 DB 依赖的纯函数，凭据检查需要 `Connection`，放命令层。

## Verification

全矩阵单测 + 契约 + 前端 fixture + 全量门（format / lint / typecheck /
test / rust:fmt / clippy / rust:test / build / python:test）。
