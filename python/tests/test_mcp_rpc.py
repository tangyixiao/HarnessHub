"""F22 S1 — rpc 方法表扩展测试（spec §3.1–§3.6，F20 握手零改动锁定）。

事实来源：docs/specs/2026-11-09-f22-mcp-client-server-design.md。
TDD：本文件先于实现提交，RED 证据须为真实失败输出。

F20 纪律（docs/specs/2026-11-04-f20-sidecar-lifecycle-design.md §3.1）：
initialize 带 requiredCapabilities ["ping","runtime.info","shutdown"] 必须继续通过。
"""

from __future__ import annotations

import io
import json
import unittest

import harness_hub_runtime.mcp as mcp_pkg
from harness_hub_runtime.mcp.registry import McpNotReadyError
from harness_hub_runtime.rpc import server as rpc_server
from harness_hub_runtime.rpc.server import (
    DEFAULT_METHODS,
    GATEWAY_STATIC_CAPABILITIES,
    GATEWAY_STATIC_METHODS,
    INVALID_REQUEST,
    METHOD_NOT_FOUND,
    MCP_NOT_READY,
    RpcError,
    handle_request,
    serve,
)

# F20 三项不变；F22 只允许在末尾追加。
F20_REQUIRED_CAPABILITIES = ["ping", "runtime.info", "shutdown"]
CAPABILITIES_BEFORE_F22 = [
    "ping",
    "runtime.info",
    "shutdown",
    "gateway.status",
    "gateway.routing",
    "gateway.streaming",
    "gateway.errors",
    "tokenization.precedence",
    "tokenization.estimate",
    "tokenization.revision",
]
MCP_METHODS = ["mcp.status", "mcp.connect", "mcp.disconnect", "mcp.tools.list", "mcp.tools.call"]
METHODS_BEFORE_F22 = [
    "ping",
    "runtime.info",
    "initialize",
    "shutdown",
    "gateway.status",
    "gateway.providers",
    "gateway.route",
    "gateway.normalize.stream",
    "gateway.normalize.error",
    "tokenization.estimate",
    "tokenization.precedence",
    "tokenization.revision",
]
GATEWAY_STATIC_METHODS_BEFORE_F22 = list(METHODS_BEFORE_F22)


class FakeMcpRegistry:
    """rpc 层接线测试用的假注册表（真实状态机行为在 test_mcp_registry.py）。"""

    def __init__(self) -> None:
        self.status_result: list = []
        self.connect_result: dict = {}
        self.connect_error: Exception | None = None
        self.disconnect_result: dict = {}
        self.tools_list_result: list = []
        self.tools_list_error: Exception | None = None
        self.tools_call_result: dict = {}
        self.tools_call_error: Exception | None = None
        self.calls: list[str] = []
        self.close_all_called = 0

    def status(self) -> list:
        return self.status_result

    def connect(self, params, transport_factory=None) -> dict:
        self.calls.append(("connect", params))
        if self.connect_error is not None:
            raise self.connect_error
        return self.connect_result

    def disconnect(self, name: str) -> dict:
        self.calls.append(("disconnect", name))
        return self.disconnect_result

    def tools_list(self, name: str) -> list:
        self.calls.append(("tools_list", name))
        if self.tools_list_error is not None:
            raise self.tools_list_error
        return self.tools_list_result

    def tools_call(self, name: str, tool: str, arguments: dict) -> dict:
        self.calls.append(("tools_call", name, tool, arguments))
        if self.tools_call_error is not None:
            raise self.tools_call_error
        return self.tools_call_result

    def close_all(self) -> None:
        self.close_all_called += 1


class MethodTableContractTest(unittest.TestCase):
    """spec §3.1：追加不重排；GATEWAY_STATIC_* 不动。"""

    def test_capabilities_appended_in_order(self) -> None:
        self.assertEqual(
            rpc_server.SUPPORTED_CAPABILITIES, CAPABILITIES_BEFORE_F22 + ["mcp.client"]
        )

    def test_gateway_static_capabilities_untouched(self) -> None:
        self.assertEqual(GATEWAY_STATIC_CAPABILITIES, CAPABILITIES_BEFORE_F22 + ["mcp.client"])

    def test_gateway_static_methods_untouched(self) -> None:
        self.assertEqual(GATEWAY_STATIC_METHODS, GATEWAY_STATIC_METHODS_BEFORE_F22)

    def test_default_methods_appended_at_end(self) -> None:
        self.assertEqual(list(DEFAULT_METHODS.keys()), METHODS_BEFORE_F22 + MCP_METHODS)

    def test_mcp_protocol_versions_match_package_constant(self) -> None:
        self.assertEqual(list(mcp_pkg.PROTOCOL_VERSIONS), ["2025-06-18", "2025-03-26", "2024-11-05"])


class F20HandshakeUnchangedTest(unittest.TestCase):
    def test_initialize_with_f20_required_capabilities_still_passes(self) -> None:
        response = handle_request(
            {
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {"protocolVersion": "1.0", "requiredCapabilities": F20_REQUIRED_CAPABILITIES},
            },
            DEFAULT_METHODS,
        )
        assert response is not None
        self.assertNotIn("error", response)
        result = response["result"]
        # F20 三项是扩展后能力清单的子集。
        for cap in F20_REQUIRED_CAPABILITIES:
            self.assertIn(cap, result["capabilities"])

    def test_old_methods_still_dispatch(self) -> None:
        response = handle_request({"jsonrpc": "2.0", "id": 2, "method": "ping"}, DEFAULT_METHODS)
        assert response is not None
        self.assertEqual(response["result"], {"pong": True})

    def test_unknown_method_still_method_not_found(self) -> None:
        response = handle_request({"jsonrpc": "2.0", "id": 3, "method": "mcp.nope"}, DEFAULT_METHODS)
        assert response is not None
        self.assertEqual(response["error"]["code"], METHOD_NOT_FOUND)


class McpHandlerWiringTest(unittest.TestCase):
    def setUp(self) -> None:
        self._previous = rpc_server._MCP_REGISTRY
        self.fake = FakeMcpRegistry()
        rpc_server._MCP_REGISTRY = self.fake

    def tearDown(self) -> None:
        rpc_server._MCP_REGISTRY = self._previous

    def _call(self, method: str, params: dict) -> dict:
        response = handle_request({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}, DEFAULT_METHODS)
        assert response is not None
        return response

    def test_status_returns_servers_and_frozen_versions(self) -> None:
        self.fake.status_result = [{"name": "weather", "state": "ready"}]
        response = self._call("mcp.status", {})
        self.assertNotIn("error", response)
        self.assertEqual(response["result"]["servers"], [{"name": "weather", "state": "ready"}])
        self.assertEqual(
            response["result"]["protocolVersions"], ["2025-06-18", "2025-03-26", "2024-11-05"]
        )

    def test_connect_returns_status_shape(self) -> None:
        self.fake.connect_result = {"name": "weather", "state": "ready", "toolCount": 0}
        response = self._call(
            "mcp.connect",
            {"name": "weather", "transport": "stdio", "command": "uvx", "args": ["x"]},
        )
        self.assertEqual(response["result"], {"name": "weather", "state": "ready", "toolCount": 0})

    def test_connect_invalid_name_is_32602(self) -> None:
        self.fake.connect_error = ValueError("name 不匹配 ^[A-Za-z0-9._-]+$")
        response = self._call("mcp.connect", {"name": "bad name", "transport": "stdio", "command": "x"})
        self.assertEqual(response["error"]["code"], INVALID_REQUEST)

    def test_disconnect_passes_through(self) -> None:
        self.fake.disconnect_result = {"state": "disconnected", "name": "ghost"}
        response = self._call("mcp.disconnect", {"name": "ghost"})
        self.assertEqual(response["result"], {"state": "disconnected", "name": "ghost"})

    def test_tools_list_returns_normalized_array(self) -> None:
        self.fake.tools_list_result = [{"name": "get_forecast", "description": "d", "inputSchemaKeys": []}]
        response = self._call("mcp.tools.list", {"name": "weather"})
        self.assertEqual(response["result"][0]["name"], "get_forecast")

    def test_tools_list_not_ready_is_32002_with_server_name(self) -> None:
        self.fake.tools_list_error = McpNotReadyError("server ghost 未就绪")
        response = self._call("mcp.tools.list", {"name": "ghost"})
        self.assertEqual(response["error"]["code"], MCP_NOT_READY)
        self.assertIn("ghost", response["error"]["message"])

    def test_tools_call_returns_normalized_result(self) -> None:
        self.fake.tools_call_result = {"isError": False, "contentCount": 1, "contentTypes": ["text"], "textPreview": "ok", "truncated": False}
        response = self._call(
            "mcp.tools.call",
            {"name": "weather", "tool": "get_forecast", "arguments": {"location": "Shanghai"}, "policyDecision": "readonly"},
        )
        self.assertEqual(response["result"]["textPreview"], "ok")

    def test_tools_call_policy_decision_missing_is_32602(self) -> None:
        response = self._call("mcp.tools.call", {"name": "weather", "tool": "t", "arguments": {}})
        self.assertEqual(response["error"]["code"], INVALID_REQUEST)

    def test_tools_call_policy_decision_wrong_value_is_32602(self) -> None:
        for bad in ("allow", "readwrite", "", "RO"):
            response = self._call(
                "mcp.tools.call", {"name": "weather", "tool": "t", "arguments": {}, "policyDecision": bad}
            )
            self.assertEqual(response["error"]["code"], INVALID_REQUEST, f"policyDecision={bad!r} 应被拒绝")

    def test_tools_call_policy_decision_readonly_passes(self) -> None:
        self.fake.tools_call_result = {"isError": False, "contentCount": 0, "contentTypes": [], "textPreview": None, "truncated": False}
        response = self._call(
            "mcp.tools.call",
            {"name": "weather", "tool": "t", "arguments": {}, "policyDecision": "readonly"},
        )
        self.assertNotIn("error", response)

    def test_tools_call_not_ready_is_32002_with_server_name(self) -> None:
        self.fake.tools_call_error = McpNotReadyError("server weather 未就绪")
        response = self._call(
            "mcp.tools.call",
            {"name": "weather", "tool": "t", "arguments": {}, "policyDecision": "readonly"},
        )
        self.assertEqual(response["error"]["code"], MCP_NOT_READY)
        self.assertIn("weather", response["error"]["message"])


class RawContentRejectionTest(unittest.TestCase):
    """spec §2 红线 5：新增 RPC 方法一律过 _reject_raw_content。"""

    def setUp(self) -> None:
        self._previous = rpc_server._MCP_REGISTRY
        rpc_server._MCP_REGISTRY = FakeMcpRegistry()

    def tearDown(self) -> None:
        rpc_server._MCP_REGISTRY = self._previous

    def test_all_five_methods_reject_raw_content_keys(self) -> None:
        valid_connect = {"name": "w", "transport": "stdio", "command": "x"}
        base = {
            "mcp.status": {},
            "mcp.connect": valid_connect,
            "mcp.disconnect": {"name": "w"},
            "mcp.tools.list": {"name": "w"},
            "mcp.tools.call": {"name": "w", "tool": "t", "policyDecision": "readonly"},
        }
        for method, params in base.items():
            for raw_key in ("prompt", "messages", "response", "content"):
                poisoned = dict(params)
                poisoned[raw_key] = "raw text"
                response = handle_request(
                    {"jsonrpc": "2.0", "id": 1, "method": method, "params": poisoned}, DEFAULT_METHODS
                )
                assert response is not None
                self.assertEqual(
                    response["error"]["code"], INVALID_REQUEST, f"{method} 的 {raw_key} 应被拒绝"
                )

    def test_non_dict_params_is_invalid_request(self) -> None:
        for method in ("mcp.status", "mcp.connect", "mcp.disconnect", "mcp.tools.list", "mcp.tools.call"):
            response = handle_request({"jsonrpc": "2.0", "id": 1, "method": method, "params": [1]}, DEFAULT_METHODS)
            assert response is not None
            self.assertEqual(response["error"]["code"], INVALID_REQUEST, method)


class ServeExitCleanupTest(unittest.TestCase):
    def test_serve_exit_calls_registry_close_all(self) -> None:
        previous = rpc_server._MCP_REGISTRY
        fake = FakeMcpRegistry()
        rpc_server._MCP_REGISTRY = fake
        try:
            lines = [
                json.dumps({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
                json.dumps({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
            ]
            stdout = io.StringIO()
            serve(io.StringIO("\n".join(lines) + "\n"), stdout)
            responses = [json.loads(line) for line in stdout.getvalue().splitlines() if line.strip()]
            self.assertEqual(len(responses), 2)
            self.assertEqual(fake.close_all_called, 1, "serve 退出路径必须 best-effort close_all")
        finally:
            rpc_server._MCP_REGISTRY = previous


class McpErrorExportTest(unittest.TestCase):
    def test_rpc_error_code_constants(self) -> None:
        self.assertEqual(rpc_server.INVALID_REQUEST, -32600)
        self.assertEqual(MCP_NOT_READY, -32002)

    def test_mcp_not_ready_error_is_exception(self) -> None:
        self.assertTrue(issubclass(McpNotReadyError, Exception))


if __name__ == "__main__":
    unittest.main()
