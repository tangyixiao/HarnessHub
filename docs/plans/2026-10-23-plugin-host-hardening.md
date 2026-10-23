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

### Task 2：RED

`cargo test --manifest-path src-tauri/Cargo.toml --test real_plugins usage_import -- --test-threads=1`

- RED：`3 passed; 4 failed`。两个身份错配都返回 `Ok(6)`；启动失败和坏载荷后状态仍为 `ready`，期望 `degraded`。
- 正向对照：合法空结果、SQLite 导入失败不降级、原有幂等导入均通过。

完整验收待实测后填写。
