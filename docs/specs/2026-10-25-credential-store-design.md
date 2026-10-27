# Credential Store + Identity Registry（ROADMAP E16）Spec

## Problem

规划 §4.17（ADR-0012）：主 SQLite 只保存 `credential_ref`；真实 Secret 进 OS
Secret Store。E14 只读发现了配置中的 Provider 指向；Hub 需要一个凭据登记层：
用户可以**登记**一个凭据（名字 + 值）到 OS 存储，SQLite 里只留句柄。

## Locked Goal

用户登记凭据（如 `deepseek-api-key`）→ 值进 OS Credential Store（Windows
Credential Manager / 跨平台抽象），SQLite 的 `credential_refs` 表只存
（ref_id、harness、标签、时间戳）；列表接口**绝不返回值**，只返回元数据；
删除 = 从两处同时移除。**本阶段不做轮换 / 不做注入到 harness 启动环境**
（那是 E15 投影 + 启动器的组合，后续迭代）。

## Acceptance Criteria

- [ ] migration 0012：`credential_refs`（id PK、label、harness_id、
      store_kind、created_at；label + harness_id UNIQUE）。
- [ ] `src-tauri/src/credentials/`：
  - `store.rs`：`SecretStore` trait（`set` / `delete` / `exists`）+
    `FileSecretStore`（开发/CI 兜底：`~/.harness-hub/secrets/<ref>`，0600 权限，
    **明确注释生产应换 OS store**；Windows 的 real keychain 经 `windows-sys`
    CredWrite 属后续真机验收项）。
  - `registry.rs`：`register(label, harness_id, value) -> RefRecord`（值进
    store，ref 落 SQLite，两步非原子但失败可清理）、`list(harness_id) ->
    Vec<RefRecord>`（无值）、`remove(id)`（先删 store 再删行）。
- [ ] IPC：`credential_register` / `credential_list` / `credential_remove`。
  值**只进不出**——没有任何接口能读回 secret 明文。
- [ ] 测试：两步失败清理（store 成功 SQLite 失败 → 删 store）、
  remove 先 store 后行、list 无值、label 重复拒绝、迁移裸 Connection。
- [ ] 前端 Settings 页「凭据登记」区（flash 子 agent）：登记表单（label +
  值 + harness 选择）、列表（只显示 label/harness/时间）、删除按钮。
- [ ] capability：不动。

## Non-goals

- 不做真实 OS keychain 集成（FileSecretStore 是显式兜底，标注生产风险）；
- 不做注入启动环境 / 不做轮换 / 不做多值 label。

## Decision

值**只进不出**是本模块唯一不变量——所有 API 不提供读取明文的路径，
测试断言序列化不含值。

## Verification

单测（迁移 / 清理 / 轮换 / round-trip）+ 契约 + 前端 + 全量门。
