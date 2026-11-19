"""F22 S1 — mcp registry 状态机测试（spec §3.2 / §3.3 / §3.4）。

事实来源：docs/specs/2026-11-09-f22-mcp-client-server-design.md。
TDD：本文件先于实现提交，RED 证据须为真实失败输出。

全部用假传输注入（transport_factory 是注入缝），不起真子进程、不发真网络。
状态机：disconnected → connecting → ready | degraded。
"""

from __future__ import annotations

import threading
import time
import unittest

from harness_hub_runtime.mcp.protocol import McpProtocolError
from harness_hub_runtime.mcp.registry import McpConnectionRegistry, McpNotReadyError
from harness_hub_runtime.mcp.transports import McpTimeoutError, McpTransportError

INIT_RESULT = {
    "protocolVersion": "2025-06-18",
    "serverInfo": {"name": "weather-server", "version": "1.2.0"},
}

STDIO_PARAMS = {
    "name": "weather",
    "transport": "stdio",
    "command": "uvx",
    "args": ["mcp-server-weather"],
    "env": {"WEATHER_API_KEY": "dummy-secret-value"},
}

HTTP_PARAMS = {"name": "remote", "transport": "http", "url": "https://mcp.example.com"}


class FakeTransport:
    """按脚本回放的假传输；记录请求/通知/关闭，供状态机断言。"""

    def __init__(self, results: dict[str, list] | None = None) -> None:
        self.results = results or {}
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


def ready_script() -> dict[str, list]:
    return {"initialize": [dict(INIT_RESULT)]}


class RecordingRegistry(McpConnectionRegistry):
    """记录每次 connect 传给 factory 的 params，便于断言注入缝。"""

    def __init__(self) -> None:
        super().__init__()
        self.created: list[FakeTransport] = []
        self.factory_params: list[dict] = []


def make_factory(registry: RecordingRegistry, results_by_call: list[dict[str, list]]):
    def factory(params: dict) -> FakeTransport:
        registry.factory_params.append(params)
        transport = FakeTransport(results_by_call.pop(0) if results_by_call else {})
        registry.created.append(transport)
        return transport

    return factory


class ConnectReadyTest(unittest.TestCase):
    def test_connect_happy_path_state_ready(self) -> None:
        registry = RecordingRegistry()
        status = registry.connect(STDIO_PARAMS, make_factory(registry, [ready_script()]))

        self.assertEqual(status["name"], "weather")
        self.assertEqual(status["transport"], "stdio")
        self.assertEqual(status["state"], "ready")
        self.assertEqual(status["protocolVersion"], "2025-06-18")
        self.assertEqual(status["serverInfo"], {"name": "weather-server", "version": "1.2.0"})
        self.assertEqual(status["toolCount"], 0)
        self.assertIsNone(status["lastError"])
        self.assertEqual(status["envKeys"], ["WEATHER_API_KEY"])

        # initialize 成功后必须发 notifications/initialized（spec §3.3）。
        transport = registry.created[0]
        self.assertIn(("notifications/initialized", {}), transport.notifications)

    def test_env_values_never_leak_into_status(self) -> None:
        registry = RecordingRegistry()
        status = registry.connect(STDIO_PARAMS, make_factory(registry, [ready_script()]))
        self.assertNotIn("dummy-secret-value", str(status))
        self.assertNotIn("dummy-secret-value", str(registry.status()))

    def test_factory_receives_params(self) -> None:
        registry = RecordingRegistry()
        registry.connect(STDIO_PARAMS, make_factory(registry, [ready_script()]))
        self.assertEqual(registry.factory_params[0], STDIO_PARAMS)

    def test_status_snapshot_sorted_by_name(self) -> None:
        registry = RecordingRegistry()
        registry.connect(STDIO_PARAMS, make_factory(registry, [ready_script()]))
        http_params = dict(HTTP_PARAMS)
        registry.connect(http_params, make_factory(registry, [ready_script()]))
        names = [server["name"] for server in registry.status()]
        self.assertEqual(names, ["remote", "weather"])

    def test_connecting_state_visible_during_handshake(self) -> None:
        """握手进行中，status() 快照应能看到 connecting 状态。"""
        slow_transport = FakeTransport({})
        slow_original = slow_transport.send_request

        def slow_send_request(method, params, timeout=10.0):
            if method == "initialize":
                time.sleep(0.3)
                return dict(INIT_RESULT)
            return slow_original(method, params, timeout)

        slow_transport.send_request = slow_send_request  # type: ignore[method-assign]

        registry = McpConnectionRegistry()
        worker = threading.Thread(
            target=lambda: registry.connect(
                {"name": "slow", "transport": "http", "url": "https://x"},
                lambda p: slow_transport,
            ),
            daemon=True,
        )
        worker.start()

        observed = None
        deadline = time.monotonic() + 2.0
        while time.monotonic() < deadline:
            for server in registry.status():
                if server["name"] == "slow" and server["state"] == "connecting":
                    observed = server["state"]
                    break
            if observed is not None:
                break
            time.sleep(0.02)
        worker.join(timeout=5.0)

        self.assertIsNotNone(observed, "握手期间应能看到 connecting 快照")
        final = [s for s in registry.status() if s["name"] == "slow"]
        self.assertEqual(final[0]["state"], "ready")


class ConnectDegradedTest(unittest.TestCase):
    def _registry(self) -> RecordingRegistry:
        return RecordingRegistry()

    def test_factory_failure_degrades_without_raising(self) -> None:
        registry = self._registry()

        def bad_factory(params: dict) -> FakeTransport:
            raise OSError("spawn failed")

        status = registry.connect(STDIO_PARAMS, bad_factory)
        self.assertEqual(status["state"], "degraded")
        self.assertIn("spawn failed", status["lastError"])
        self.assertIsNone(status["protocolVersion"])

    def test_version_mismatch_degrades_with_both_versions(self) -> None:
        registry = self._registry()
        script = {"initialize": [{"protocolVersion": "1999-01-01", "serverInfo": {"name": "s", "version": "1"}}]}
        status = registry.connect(STDIO_PARAMS, make_factory(registry, [script]))
        self.assertEqual(status["state"], "degraded")
        self.assertIn("1999-01-01", status["lastError"])
        self.assertIn("2025-06-18", status["lastError"])
        # 协商失败的传输要被关掉。
        self.assertTrue(registry.created[0].closed)

    def test_handshake_transport_error_degrades(self) -> None:
        registry = self._registry()
        script = {"initialize": [McpTransportError("process exited")]}
        status = registry.connect(STDIO_PARAMS, make_factory(registry, [script]))
        self.assertEqual(status["state"], "degraded")
        self.assertIn("process exited", status["lastError"])

    def test_handshake_protocol_error_degrades(self) -> None:
        registry = self._registry()
        script = {"initialize": [McpProtocolError("missing protocolVersion")]}
        status = registry.connect(STDIO_PARAMS, make_factory(registry, [script]))
        self.assertEqual(status["state"], "degraded")
        self.assertIn("missing protocolVersion", status["lastError"])

    def test_same_name_reconnect_closes_old_transport(self) -> None:
        registry = self._registry()
        registry.connect(STDIO_PARAMS, make_factory(registry, [ready_script()]))
        registry.connect(STDIO_PARAMS, make_factory(registry, [ready_script()]))
        self.assertTrue(registry.created[0].closed, "重连前旧传输应被关闭")
        self.assertFalse(registry.created[1].closed)
        self.assertEqual(len(registry.status()), 1)
        self.assertEqual(registry.status()[0]["state"], "ready")


class ConnectParamValidationTest(unittest.TestCase):
    """参数非法 → ValueError（rpc 层映射为 -32602），不得进入状态机。"""

    def _factory(self, registry: RecordingRegistry):
        return make_factory(registry, [ready_script()])

    def test_bad_name_rejected(self) -> None:
        registry = RecordingRegistry()
        for bad in ("", "has space", "bad/name", "中文名"):
            with self.assertRaises(ValueError):
                registry.connect({**STDIO_PARAMS, "name": bad}, self._factory(registry))

    def test_missing_command_rejected(self) -> None:
        registry = RecordingRegistry()
        with self.assertRaises(ValueError):
            registry.connect({"name": "a", "transport": "stdio"}, self._factory(registry))

    def test_missing_url_rejected(self) -> None:
        registry = RecordingRegistry()
        with self.assertRaises(ValueError):
            registry.connect({"name": "a", "transport": "http"}, self._factory(registry))

    def test_unknown_transport_rejected(self) -> None:
        registry = RecordingRegistry()
        with self.assertRaises(ValueError):
            registry.connect({"name": "a", "transport": "carrier-pigeon"}, self._factory(registry))

    def test_non_dict_params_rejected(self) -> None:
        registry = RecordingRegistry()
        with self.assertRaises(ValueError):
            registry.connect("not-a-dict", self._factory(registry))  # type: ignore[arg-type]


class ToolsTest(unittest.TestCase):
    def test_tools_list_pages_and_refreshes_tool_count(self) -> None:
        registry = RecordingRegistry()
        connect_script = {"initialize": [dict(INIT_RESULT)]}
        registry.connect(STDIO_PARAMS, make_factory(registry, [connect_script]))
        transport = registry.created[0]
        transport.results["tools/list"] = [
            {"tools": [{"name": "get_forecast", "description": "d", "inputSchema": {"properties": {"location": {}}}}], "nextCursor": "p2"},
            {"tools": [{"name": "list_cities"}]},
        ]
        tools = registry.tools_list("weather")
        self.assertEqual([t["name"] for t in tools], ["get_forecast", "list_cities"])
        self.assertEqual(tools[0]["inputSchemaKeys"], ["location"])
        # 工具数在 tools.list 后刷新（spec §3.3）。
        self.assertEqual(registry.status()[0]["toolCount"], 2)

    def test_tools_list_unknown_name_raises_not_ready_with_name(self) -> None:
        registry = RecordingRegistry()
        with self.assertRaises(McpNotReadyError) as ctx:
            registry.tools_list("ghost")
        self.assertIn("ghost", str(ctx.exception))

    def test_tools_list_after_degrade_raises_not_ready(self) -> None:
        registry = RecordingRegistry()
        registry.connect(STDIO_PARAMS, make_factory(registry, [{"initialize": [McpTransportError("dead")]}]))
        with self.assertRaises(McpNotReadyError):
            registry.tools_list("weather")

    def test_tools_call_happy_path(self) -> None:
        registry = RecordingRegistry()
        registry.connect(STDIO_PARAMS, make_factory(registry, [ready_script()]))
        transport = registry.created[0]
        transport.results["tools/call"] = [
            {"content": [{"type": "text", "text": "晴"}], "isError": False}
        ]
        result = registry.tools_call("weather", "get_forecast", {"location": "Shanghai"})
        self.assertFalse(result["isError"])
        self.assertEqual(result["textPreview"], "晴")
        self.assertEqual(transport.requests[-1], ("tools/call", {"name": "get_forecast", "arguments": {"location": "Shanghai"}}))

    def test_tools_call_timeout_degrades_and_keeps_last_error(self) -> None:
        registry = RecordingRegistry()
        registry.connect(STDIO_PARAMS, make_factory(registry, [ready_script()]))
        transport = registry.created[0]
        transport.results["tools/call"] = [McpTimeoutError("read timed out")]
        with self.assertRaises(McpNotReadyError) as ctx:
            registry.tools_call("weather", "get_forecast", {})
        self.assertIn("weather", str(ctx.exception))
        status = registry.status()[0]
        self.assertEqual(status["state"], "degraded")
        self.assertIn("read timed out", status["lastError"])

    def test_tools_call_tool_level_is_error_is_data_not_degrade(self) -> None:
        registry = RecordingRegistry()
        registry.connect(STDIO_PARAMS, make_factory(registry, [ready_script()]))
        transport = registry.created[0]
        transport.results["tools/call"] = [
            {"content": [{"type": "text", "text": "unknown tool"}], "isError": True}
        ]
        result = registry.tools_call("weather", "nope", {})
        self.assertTrue(result["isError"])
        self.assertEqual(registry.status()[0]["state"], "ready")

    def test_tools_call_bad_arguments_rejected(self) -> None:
        registry = RecordingRegistry()
        registry.connect(STDIO_PARAMS, make_factory(registry, [ready_script()]))
        with self.assertRaises(ValueError):
            registry.tools_call("weather", "get_forecast", [1, 2, 3])  # type: ignore[arg-type]


class DisconnectAndCloseAllTest(unittest.TestCase):
    def test_disconnect_unknown_name_idempotent(self) -> None:
        registry = RecordingRegistry()
        self.assertEqual(registry.disconnect("ghost"), {"state": "disconnected", "name": "ghost"})

    def test_disconnect_closes_transport_and_clears_entry(self) -> None:
        registry = RecordingRegistry()
        registry.connect(STDIO_PARAMS, make_factory(registry, [ready_script()]))
        result = registry.disconnect("weather")
        self.assertEqual(result, {"state": "disconnected", "name": "weather"})
        self.assertTrue(registry.created[0].closed)
        self.assertEqual(registry.status(), [])
        # 再断一次仍幂等。
        self.assertEqual(registry.disconnect("weather"), {"state": "disconnected", "name": "weather"})

    def test_close_all_closes_everything_best_effort(self) -> None:
        registry = RecordingRegistry()
        registry.connect(STDIO_PARAMS, make_factory(registry, [ready_script()]))
        registry.connect(HTTP_PARAMS, make_factory(registry, [ready_script()]))
        registry.close_all()
        self.assertTrue(registry.created[0].closed)
        self.assertTrue(registry.created[1].closed)
        self.assertEqual(registry.status(), [])
        # 幂等。
        registry.close_all()

    def test_close_all_survives_broken_transport(self) -> None:
        registry = RecordingRegistry()
        registry.connect(STDIO_PARAMS, make_factory(registry, [ready_script()]))

        def broken_close() -> None:
            raise RuntimeError("close exploded")

        registry.created[0].close = broken_close  # type: ignore[method-assign]
        registry.close_all()  # 不得抛异常
        self.assertEqual(registry.status(), [])


if __name__ == "__main__":
    unittest.main()
