"""F22 S1 — mcp dto 冻结契约测试（spec §3.2 / §3.5 / §3.6 / §3.7）。

事实来源：docs/specs/2026-11-09-f22-mcp-client-server-design.md。
TDD：本文件先于实现提交，RED 证据须为真实失败输出。

安全闸：快照只回 envKeys **名字**，任何 env/headers 值绝不出现（spec §2 红线 1）；
所有字符串截断纪律在此锁定。
"""

from __future__ import annotations

import dataclasses
import unittest

from harness_hub_runtime.mcp import PROTOCOL_VERSIONS
from harness_hub_runtime.mcp.dto import (
    MAX_DESCRIPTION,
    MAX_LAST_ERROR,
    MAX_SCHEMA_KEYS,
    MAX_SERVER_INFO,
    MAX_TEXT_PREVIEW,
    SERVER_STATES,
    McpServerInfo,
    McpServerStatus,
    McpToolCallResult,
    McpToolDescriptor,
)

# spec §3.2：协议版本冻结支持集，降序（首选在前）。
EXPECTED_VERSIONS = ("2025-06-18", "2025-03-26", "2024-11-05")


class ConstantsTest(unittest.TestCase):
    def test_protocol_versions_exact_values_and_order(self) -> None:
        self.assertEqual(PROTOCOL_VERSIONS, EXPECTED_VERSIONS)
        # 首选 2025-06-18 必须在第一位（协商缺省取首选）。
        self.assertEqual(PROTOCOL_VERSIONS[0], "2025-06-18")

    def test_server_states_exact_values(self) -> None:
        self.assertEqual(SERVER_STATES, ("disconnected", "connecting", "ready", "degraded"))

    def test_safety_limits(self) -> None:
        self.assertEqual(MAX_LAST_ERROR, 256)
        self.assertEqual(MAX_SERVER_INFO, 120)
        self.assertEqual(MAX_DESCRIPTION, 200)
        self.assertEqual(MAX_SCHEMA_KEYS, 32)
        self.assertEqual(MAX_TEXT_PREVIEW, 2048)


class McpServerStatusTest(unittest.TestCase):
    def test_to_json_keys_and_order_ready_shape(self) -> None:
        status = McpServerStatus(
            name="weather",
            transport="stdio",
            state="ready",
            protocol_version="2025-06-18",
            server_info=McpServerInfo(name="weather-server", version="1.2.0"),
            tool_count=3,
            last_error=None,
            env_keys=("WEATHER_API_KEY",),
        )
        self.assertEqual(
            status.to_json(),
            {
                "name": "weather",
                "transport": "stdio",
                "state": "ready",
                "protocolVersion": "2025-06-18",
                "serverInfo": {"name": "weather-server", "version": "1.2.0"},
                "toolCount": 3,
                "lastError": None,
                "envKeys": ["WEATHER_API_KEY"],
            },
        )
        self.assertEqual(
            list(status.to_json().keys()),
            ["name", "transport", "state", "protocolVersion", "serverInfo", "toolCount", "lastError", "envKeys"],
        )
        self.assertEqual(
            list(status.to_json()["serverInfo"].keys()), ["name", "version"]
        )

    def test_to_json_degraded_shape_keeps_keys(self) -> None:
        """未完成握手的 server 也要保持同一键形状（值为 null）。"""
        status = McpServerStatus(
            name="remote",
            transport="http",
            state="degraded",
            protocol_version=None,
            server_info=McpServerInfo(name=None, version=None),
            tool_count=0,
            last_error="HTTP 404",
            env_keys=(),
        )
        data = status.to_json()
        self.assertIsNone(data["protocolVersion"])
        self.assertEqual(data["serverInfo"], {"name": None, "version": None})
        self.assertEqual(data["envKeys"], [])

    def test_frozen(self) -> None:
        status = McpServerStatus(
            name="a",
            transport="stdio",
            state="disconnected",
            protocol_version=None,
            server_info=McpServerInfo(name=None, version=None),
            tool_count=0,
            last_error=None,
            env_keys=(),
        )
        with self.assertRaises(dataclasses.FrozenInstanceError):
            status.name = "b"  # type: ignore[misc]

    def test_last_error_truncated_to_256(self) -> None:
        status = McpServerStatus(
            name="a",
            transport="stdio",
            state="degraded",
            protocol_version=None,
            server_info=McpServerInfo(name=None, version=None),
            tool_count=0,
            last_error="x" * 300,
            env_keys=(),
        )
        self.assertEqual(len(status.to_json()["lastError"]), 256)

    def test_server_info_name_version_truncated_to_120(self) -> None:
        info = McpServerInfo(name="n" * 200, version="v" * 200)
        data = info.to_json()
        self.assertEqual(len(data["name"]), 120)
        self.assertEqual(len(data["version"]), 120)

    def test_env_keys_only_names_never_values(self) -> None:
        """spec §2 红线 1：envKeys 只有名字，值绝不进入快照。"""
        status = McpServerStatus(
            name="weather",
            transport="stdio",
            state="ready",
            protocol_version="2025-06-18",
            server_info=McpServerInfo(name="s", version="1"),
            tool_count=0,
            last_error=None,
            env_keys=("SECRET_VALUE_SHOULD_NOT_APPEAR",),
        )
        payload = str(status.to_json())
        self.assertIn("SECRET_VALUE_SHOULD_NOT_APPEAR", payload)
        # 结构上 envKeys 是纯名字列表，没有 value 字段。
        self.assertNotIn("envValues", payload)


class McpToolDescriptorTest(unittest.TestCase):
    def test_to_json_keys_and_order(self) -> None:
        tool = McpToolDescriptor(
            name="get_forecast", description="查天气", input_schema_keys=("location",)
        )
        self.assertEqual(
            tool.to_json(),
            {"name": "get_forecast", "description": "查天气", "inputSchemaKeys": ["location"]},
        )
        self.assertEqual(
            list(tool.to_json().keys()), ["name", "description", "inputSchemaKeys"]
        )

    def test_description_truncated_to_200(self) -> None:
        tool = McpToolDescriptor(name="t", description="d" * 300, input_schema_keys=())
        self.assertEqual(len(tool.to_json()["description"]), 200)

    def test_input_schema_keys_capped_at_32(self) -> None:
        tool = McpToolDescriptor(
            name="t", description="", input_schema_keys=tuple(f"k{i}" for i in range(40))
        )
        self.assertEqual(len(tool.to_json()["inputSchemaKeys"]), 32)

    def test_frozen(self) -> None:
        tool = McpToolDescriptor(name="t", description="", input_schema_keys=())
        with self.assertRaises(dataclasses.FrozenInstanceError):
            tool.name = "x"  # type: ignore[misc]


class McpToolCallResultTest(unittest.TestCase):
    def test_to_json_keys_and_order(self) -> None:
        result = McpToolCallResult(
            is_error=False,
            content_count=1,
            content_types=("text",),
            text_preview="hello",
            truncated=False,
        )
        self.assertEqual(
            result.to_json(),
            {
                "isError": False,
                "contentCount": 1,
                "contentTypes": ["text"],
                "textPreview": "hello",
                "truncated": False,
            },
        )
        self.assertEqual(
            list(result.to_json().keys()),
            ["isError", "contentCount", "contentTypes", "textPreview", "truncated"],
        )

    def test_text_preview_truncated_to_2048_sets_flag(self) -> None:
        result = McpToolCallResult(
            is_error=False,
            content_count=1,
            content_types=("text",),
            text_preview="x" * 3000,
            truncated=False,
        )
        data = result.to_json()
        self.assertEqual(len(data["textPreview"]), 2048)
        self.assertTrue(data["truncated"])

    def test_text_preview_at_limit_not_truncated(self) -> None:
        result = McpToolCallResult(
            is_error=False,
            content_count=1,
            content_types=("text",),
            text_preview="x" * 2048,
            truncated=False,
        )
        data = result.to_json()
        self.assertEqual(len(data["textPreview"]), 2048)
        self.assertFalse(data["truncated"])

    def test_text_preview_none_allowed(self) -> None:
        result = McpToolCallResult(
            is_error=True, content_count=0, content_types=(), text_preview=None, truncated=False
        )
        data = result.to_json()
        self.assertIsNone(data["textPreview"])
        self.assertFalse(data["truncated"])

    def test_frozen(self) -> None:
        result = McpToolCallResult(
            is_error=False, content_count=0, content_types=(), text_preview=None, truncated=False
        )
        with self.assertRaises(dataclasses.FrozenInstanceError):
            result.is_error = True  # type: ignore[misc]


if __name__ == "__main__":
    unittest.main()
