"""F22 S1 — mcp protocol 核心测试（spec §3.3 / §3.5 / §3.6）。

事实来源：docs/specs/2026-11-09-f22-mcp-client-server-design.md。
TDD：本文件先于实现提交，RED 证据须为真实失败输出。

全部用假传输（FakeTransport）注入，不发真网络、不起真子进程。
"""

from __future__ import annotations

import unittest

from harness_hub_runtime.mcp import PROTOCOL_VERSIONS
from harness_hub_runtime.mcp.protocol import (
    CLIENT_NAME,
    INITIALIZED_NOTIFICATION,
    TOOLS_MAX,
    McpProtocolError,
    call_tool_via,
    check_negotiated_version,
    initialize_params,
    list_tools_via,
    server_info_from,
    tool_call_result_from_raw,
    tool_from_raw,
)
from harness_hub_runtime.mcp.transports import McpTransportError


class FakeTransport:
    """记录调用并按脚本回放的假传输（冻结接口 send_request/send_notification/close）。"""

    def __init__(self, results: dict[str, list]) -> None:
        self.results = results
        self.requests: list[tuple[str, dict]] = []
        self.notifications: list[tuple[str, dict]] = []
        self.closed = False

    def send_request(self, method: str, params: dict, timeout: float = 10.0) -> dict:
        self.requests.append((method, params))
        queue = self.results.get(method)
        if not queue:
            raise McpTransportError(f"脚本未覆盖方法：{method}")
        item = queue.pop(0)
        if isinstance(item, Exception):
            raise item
        return item

    def send_notification(self, method: str, params: dict) -> None:
        self.notifications.append((method, params))

    def close(self) -> None:
        self.closed = True


class InitializeParamsTest(unittest.TestCase):
    def test_default_uses_preferred_version(self) -> None:
        params = initialize_params(None)
        self.assertEqual(params["protocolVersion"], PROTOCOL_VERSIONS[0])
        self.assertEqual(
            list(params.keys()), ["protocolVersion", "capabilities", "clientInfo"]
        )
        self.assertEqual(params["clientInfo"]["name"], CLIENT_NAME)

    def test_explicit_version_used_as_is(self) -> None:
        self.assertEqual(initialize_params("2024-11-05")["protocolVersion"], "2024-11-05")


class NegotiationTest(unittest.TestCase):
    def test_supported_version_passes_through(self) -> None:
        for version in PROTOCOL_VERSIONS:
            self.assertEqual(
                check_negotiated_version({"protocolVersion": version}, "2025-06-18"), version
            )

    def test_unsupported_version_raises_with_both_versions(self) -> None:
        with self.assertRaises(McpProtocolError) as ctx:
            check_negotiated_version({"protocolVersion": "1999-01-01"}, "2025-06-18")
        message = str(ctx.exception)
        self.assertIn("1999-01-01", message)
        self.assertIn("2025-06-18", message)

    def test_missing_version_raises(self) -> None:
        with self.assertRaises(McpProtocolError):
            check_negotiated_version({"capabilities": {}}, "2025-06-18")


class InitializedNotificationTest(unittest.TestCase):
    def test_method_and_shape(self) -> None:
        # spec §3.3：initialize 成功后必须发 notifications/initialized（无 id，不回包）。
        self.assertEqual(INITIALIZED_NOTIFICATION[0], "notifications/initialized")
        self.assertIsInstance(INITIALIZED_NOTIFICATION[1], dict)


class ServerInfoTest(unittest.TestCase):
    def test_extracts_name_and_version(self) -> None:
        info = server_info_from(
            {"serverInfo": {"name": "weather-server", "version": "1.2.0"}}
        )
        self.assertEqual(info, ("weather-server", "1.2.0"))

    def test_missing_server_info_returns_none(self) -> None:
        self.assertIsNone(server_info_from({}))
        self.assertIsNone(server_info_from({"serverInfo": "junk"}))


class ToolFromRawTest(unittest.TestCase):
    def test_normalizes_top_level_schema_keys_only(self) -> None:
        raw = {
            "name": "get_forecast",
            "description": "d",
            "inputSchema": {
                "type": "object",
                "properties": {"location": {"type": "string"}, "days": {}},
            },
        }
        tool = tool_from_raw(raw)
        self.assertIsNotNone(tool)
        self.assertEqual(tool.name, "get_forecast")
        self.assertEqual(tool.input_schema_keys, ("location", "days"))

    def test_missing_or_invalid_name_rejected(self) -> None:
        self.assertIsNone(tool_from_raw({"description": "no name"}))
        self.assertIsNone(tool_from_raw("not-a-dict"))

    def test_missing_input_schema_gives_empty_keys(self) -> None:
        tool = tool_from_raw({"name": "t"})
        self.assertEqual(tool.input_schema_keys, ())
        self.assertEqual(tool.description, "")


class ListToolsViaTest(unittest.TestCase):
    def test_pagination_cursor_exhausted(self) -> None:
        transport = FakeTransport(
            {
                "tools/list": [
                    {"tools": [{"name": "a"}], "nextCursor": "page2"},
                    {"tools": [{"name": "b"}, {"name": "c"}], "nextCursor": ""},
                ]
            }
        )
        tools = list_tools_via(transport, timeout=5.0)
        self.assertEqual([t.name for t in tools], ["a", "b", "c"])
        # 第二页用了游标；游标耗尽后停止。
        self.assertEqual(transport.requests, [("tools/list", {}), ("tools/list", {"cursor": "page2"})])

    def test_cap_at_512_tools(self) -> None:
        self.assertEqual(TOOLS_MAX, 512)
        # 无尽游标 + 每页 600 个工具：到 512 就停。
        transport = FakeTransport(
            {"tools/list": [{"tools": [{"name": f"t{i}"} for i in range(600)], "nextCursor": "x"}]}
        )
        tools = list_tools_via(transport)
        self.assertEqual(len(tools), 512)
        self.assertEqual(transport.requests, [("tools/list", {})])

    def test_transport_error_propagates(self) -> None:
        transport = FakeTransport({"tools/list": [McpTransportError("boom")]})
        with self.assertRaises(McpTransportError):
            list_tools_via(transport)


class ToolCallResultTest(unittest.TestCase):
    def test_only_text_blocks_concatenated_into_preview(self) -> None:
        raw = {
            "content": [
                {"type": "text", "text": "part1"},
                {"type": "image", "data": "..."},
                {"type": "text", "text": "part2"},
            ],
            "isError": False,
        }
        result = tool_call_result_from_raw(raw)
        self.assertEqual(result.to_json()["contentCount"], 3)
        self.assertEqual(result.to_json()["contentTypes"], ["text", "image", "text"])
        self.assertIn("part1", result.to_json()["textPreview"])
        self.assertIn("part2", result.to_json()["textPreview"])

    def test_no_text_blocks_gives_null_preview(self) -> None:
        raw = {"content": [{"type": "image", "data": "..."}], "isError": False}
        result = tool_call_result_from_raw(raw)
        self.assertIsNone(result.to_json()["textPreview"])
        self.assertFalse(result.to_json()["truncated"])

    def test_is_error_passthrough(self) -> None:
        raw = {"content": [{"type": "text", "text": "boom"}], "isError": True}
        result = tool_call_result_from_raw(raw)
        self.assertTrue(result.to_json()["isError"])

    def test_long_text_truncated_with_flag(self) -> None:
        raw = {"content": [{"type": "text", "text": "x" * 3000}], "isError": False}
        result = tool_call_result_from_raw(raw)
        data = result.to_json()
        self.assertEqual(len(data["textPreview"]), 2048)
        self.assertTrue(data["truncated"])

    def test_missing_content_tolerated(self) -> None:
        result = tool_call_result_from_raw({})
        data = result.to_json()
        self.assertEqual(data["contentCount"], 0)
        self.assertIsNone(data["textPreview"])


class CallToolViaTest(unittest.TestCase):
    def test_params_shape_and_normalization(self) -> None:
        transport = FakeTransport(
            {"tools/call": [{"content": [{"type": "text", "text": "ok"}], "isError": False}]}
        )
        result = call_tool_via(transport, "get_forecast", {"location": "Shanghai"}, timeout=30.0)
        self.assertEqual(transport.requests, [("tools/call", {"name": "get_forecast", "arguments": {"location": "Shanghai"}})])
        self.assertEqual(result.to_json()["textPreview"], "ok")

    def test_non_dict_arguments_sent_as_empty_object(self) -> None:
        transport = FakeTransport({"tools/call": [{"content": []}]})
        call_tool_via(transport, "t", "junk")  # type: ignore[arg-type]
        self.assertEqual(transport.requests[0][1]["arguments"], {})


if __name__ == "__main__":
    unittest.main()
