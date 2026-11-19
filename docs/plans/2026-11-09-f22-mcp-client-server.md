# F22 执行计划（MCP Client / Server）

> 规格：`docs/specs/2026-11-09-f22-mcp-client-server-design.md`（契约唯一事实来源）。

## 切片

1. **S1 `feat/f22/py-mcp-client`**：`mcp/` 五模块（dto/protocol/transports/registry）
   - `rpc/server.py` 方法表扩展 + `python/tests/test_mcp_{dto,protocol,transports,registry,rpc}.py`。
     TDD：先失败测试。红线：env/headers 值不落任何持久层；`_reject_raw_content` 全方法覆盖；
     F20 握手零改动（`REQUIRED_CAPABILITIES` 仍是三项）。
2. **S2 `feat/f22/rust-mcp-bridge`**：`mcp/policy.rs`（策略 v1 冻结规则）+
   `mcp/client_bridge.rs`（SidecarManager 调用 + 状态缓存）+ 5 个薄命令 + serde 契约测试
   - `src-tauri/tests/mcp_bridge.rs`。不得碰 python/** 与 server/**。
3. **S3 `feat/f22/hub-mcp-server`**：`mcp/server/{mod,tools}.rs` headless stdio 服务端
   - `main.rs` argv 分流 + `lib.rs::run_mcp_server` + `src-tauri/tests/mcp_server_stdio.rs`
     真机 spawn 证据（`CARGO_BIN_EXE_harness-hub`）。
4. **S4 `feat/f22/web`**：`ipc.ts` normalize + `McpRuntimeCard` + fixture tests。

合并顺序 S1 → S2 → (S3 ∥ S4)。每切片 PR 附证据表（本地门真实输出 + CI run 链接）。

## 验收对照（§8.3）

- [ ] MCP Client registry（S1 registry 状态机 + S2 状态快照）
- [ ] stdio（S1 StdioTransport + S2 connect 接线）
- [ ] Streamable HTTP（S1 HttpTransport + SSE + session 头）
- [ ] Tool discovery（S1 tools.list 归一 + S2 IPC + S4 UI）
- [ ] MCP permission policy（S2 policy.rs 默认只读规则 + deny 结构化返回）
- [ ] Hub MCP Server 只读工具（S3 五工具 + 真机 stdio 测试）
