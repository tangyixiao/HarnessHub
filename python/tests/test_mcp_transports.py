"""F22 S1 — mcp transports 测试（spec §3.7 / §2 红线 2）。

事实来源：docs/specs/2026-11-09-f22-mcp-client-server-design.md。
TDD：本文件先于实现提交，RED 证据须为真实失败输出。

* StdioTransport：真实 python 子进程（`[sys.executable, "-c", 脚本]`）做端到端，
  绝不经 shell、argv 必须是列表。
* HttpTransport：SSE 解析用纯字节单测；urlopen 打桩，不发真网络。
"""

from __future__ import annotations

import json
import sys
import unittest
from unittest import mock

from harness_hub_runtime.mcp.transports import (
    HTTP_TIMEOUT,
    McpProtocolError,
    McpTimeoutError,
    McpTransportError,
    HttpTransport,
    StdioTransport,
    decode_response_body,
    parse_sse_messages,
)

# 一行分隔 JSON-RPC 的最小回显 server：请求原样回显 + env 探针。
# TEST_MCP_MARKER 只是为了验证「继承 os.environ + 覆盖」行为，不是任何真实凭据。
ECHO_SCRIPT = """
import json, os, sys
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" in req:
        resp = {
            "jsonrpc": "2.0",
            "id": req["id"],
            "result": {
                "method": req.get("method"),
                "params": req.get("params"),
                "marker": os.environ.get("TEST_MCP_MARKER"),
                "hasPath": bool(os.environ.get("PATH")),
            },
        }
        sys.stdout.write(json.dumps(resp) + "\\n")
        sys.stdout.flush()
"""

# 慢响应 server：每条请求先睡 3s 再回，用于触发 client 读超时。
SLOW_SCRIPT = """
import json, sys, time
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    time.sleep(3)
    if "id" in req:
        resp = {"jsonrpc": "2.0", "id": req["id"], "result": {"ok": True}}
        sys.stdout.write(json.dumps(resp) + "\\n")
        sys.stdout.flush()
"""


class StdioTransportTest(unittest.TestCase):
    def test_end_to_end_roundtrip_and_env_inheritance(self) -> None:
        transport = StdioTransport(
            [sys.executable, "-c", ECHO_SCRIPT], env={"TEST_MCP_MARKER": "hello"}
        )
        try:
            result = transport.send_request("ping", {"a": 1}, timeout=15.0)
            self.assertEqual(result["method"], "ping")
            self.assertEqual(result["params"], {"a": 1})
            # env 继承宿主 + 覆盖（spec §2 红线 1：值单次传递）。
            self.assertEqual(result["marker"], "hello")
            self.assertTrue(result["hasPath"])
        finally:
            transport.close()

    def test_send_notification_never_expects_response(self) -> None:
        transport = StdioTransport([sys.executable, "-c", ECHO_SCRIPT])
        try:
            # 通知（无 id）不回包；不应阻塞或抛异常。
            transport.send_notification("notifications/initialized", {})
        finally:
            transport.close()

    def test_request_timeout_raises(self) -> None:
        transport = StdioTransport([sys.executable, "-c", SLOW_SCRIPT])
        try:
            with self.assertRaises(McpTimeoutError):
                transport.send_request("slow", {}, timeout=0.5)
        finally:
            transport.close()

    def test_command_must_be_list_argv_never_shell(self) -> None:
        with self.assertRaises(ValueError):
            StdioTransport("python -c print(1)")  # type: ignore[arg-type]
        with self.assertRaises(ValueError):
            StdioTransport([])
        with self.assertRaises(ValueError):
            StdioTransport([sys.executable, 42])  # type: ignore[list-item]

    def test_close_terminates_process_and_is_idempotent(self) -> None:
        transport = StdioTransport([sys.executable, "-c", ECHO_SCRIPT])
        proc = transport._process
        transport.close()
        self.assertIsNotNone(proc)
        assert proc is not None
        self.assertIsNotNone(proc.poll(), "close() 后子进程应已终止")
        # 幂等：再关一次不抛异常。
        transport.close()

    def test_close_on_already_dead_process_never_raises(self) -> None:
        transport = StdioTransport([sys.executable, "-c", "import sys; sys.exit(0)"])
        transport._process.wait(timeout=10)
        transport.close()  # 进程已退出，仍不得抛异常

    def test_windows_creation_no_window_flag(self) -> None:
        transport = StdioTransport([sys.executable, "-c", ECHO_SCRIPT])
        try:
            if sys.platform == "win32":
                self.assertEqual(transport._creationflags, 0x08000000)
            else:
                self.assertEqual(transport._creationflags, 0)
        finally:
            transport.close()


class SseParsingTest(unittest.TestCase):
    """SSE 最小子集：纯字节解析，不发网络（spec §3.7）。"""

    def test_single_data_line_event(self) -> None:
        raw = (
            b'event: message\r\n'
            b'data: {"jsonrpc":"2.0","id":1,"result":{"ok":true}}\r\n'
            b"\r\n"
        )
        messages = parse_sse_messages(raw)
        self.assertEqual(messages, [{"jsonrpc": "2.0", "id": 1, "result": {"ok": True}}])

    def test_multi_data_lines_joined_with_newline(self) -> None:
        raw = b"data: {\"a\":\ndata: 1}\n\n"
        self.assertEqual(parse_sse_messages(raw), [{"a": 1}])

    def test_colon_comment_lines_ignored(self) -> None:
        raw = b": keep-alive ping\ndata: {\"b\":2}\n\n"
        self.assertEqual(parse_sse_messages(raw), [{"b": 2}])

    def test_non_json_data_lines_skipped(self) -> None:
        raw = b"data: not-json\ndata: {\"c\":3}\n\n"
        self.assertEqual(parse_sse_messages(raw), [{"c": 3}])

    def test_data_without_space_after_colon(self) -> None:
        raw = b"data:{\"d\":4}\n\n"
        self.assertEqual(parse_sse_messages(raw), [{"d": 4}])

    def test_decode_dispatch_sse_and_json_object(self) -> None:
        sse = b"event: message\ndata: {\"id\":1,\"result\":{}}\n\n"
        self.assertEqual(
            decode_response_body("text/event-stream", sse), [{"id": 1, "result": {}}]
        )
        single = b'{"jsonrpc":"2.0","id":2,"result":{"x":1}}'
        self.assertEqual(
            decode_response_body("application/json", single),
            [{"jsonrpc": "2.0", "id": 2, "result": {"x": 1}}],
        )
        # content_type 带参数（charset）也要能识别 SSE。
        self.assertEqual(
            decode_response_body("text/event-stream; charset=utf-8", sse),
            [{"id": 1, "result": {}}],
        )
        self.assertEqual(decode_response_body("application/json", b""), [])


class _FakeHeaders:
    """模拟 http.client 响应头的大小写不敏感 .get()。"""

    def __init__(self, mapping: dict[str, str]) -> None:
        self._items = {key.lower(): value for key, value in mapping.items()}

    def get(self, name: str, default: str | None = None) -> str | None:
        return self._items.get(name.lower(), default)


class FakeResponse:
    def __init__(self, body: bytes, content_type: str = "application/json", headers: dict[str, str] | None = None) -> None:
        self._body = body
        self.status = 200
        all_headers = {"Content-Type": content_type}
        all_headers.update(headers or {})
        self.headers = _FakeHeaders(all_headers)

    def read(self) -> bytes:
        return self._body

    def close(self) -> None:
        pass

    def __enter__(self) -> "FakeResponse":
        return self

    def __exit__(self, *exc: object) -> bool:
        return False


class HttpTransportTest(unittest.TestCase):
    def _transport(self, **kwargs: object) -> HttpTransport:
        return HttpTransport("https://mcp.example.com/rpc", **kwargs)  # type: ignore[arg-type]

    def test_send_request_returns_result_and_keeps_session(self) -> None:
        captured: list = []
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "result": {"pong": True}}).encode()

        def fake_urlopen(request, **kwargs):
            captured.append((request, kwargs))
            if len(captured) == 1:
                return FakeResponse(body, headers={"Mcp-Session-Id": "sess-1"})
            return FakeResponse(
                json.dumps({"jsonrpc": "2.0", "id": 2, "result": {"second": True}}).encode()
            )

        transport = self._transport()
        with mock.patch("harness_hub_runtime.mcp.transports.urlopen", side_effect=fake_urlopen):
            result = transport.send_request("ping", {}, timeout=5.0)
            self.assertEqual(result, {"pong": True})
            second = transport.send_request("ping", {}, timeout=5.0)
            self.assertEqual(second, {"second": True})

        self.assertEqual(len(captured), 2)
        first_request, first_kwargs = captured[0]
        second_request, _ = captured[1]
        # 默认 HTTP 超时与单次调用超时。
        self.assertEqual(first_kwargs.get("timeout"), 5.0)
        # 第一条请求还没有 session，第二条必须回传 Mcp-Session-Id。
        first_headers = {k.lower(): v for k, v in first_request.header_items()}
        second_headers = {k.lower(): v for k, v in second_request.header_items()}
        self.assertNotIn("mcp-session-id", first_headers)
        self.assertEqual(second_headers.get("mcp-session-id"), "sess-1")
        self.assertEqual(second_headers.get("accept"), "application/json, text/event-stream")

    def test_default_timeout_is_10s(self) -> None:
        self.assertEqual(HTTP_TIMEOUT, 10.0)
        captured: list = []

        def fake_urlopen(request, **kwargs):
            captured.append(kwargs)
            return FakeResponse(json.dumps({"jsonrpc": "2.0", "id": 1, "result": {}}).encode())

        transport = self._transport()
        with mock.patch("harness_hub_runtime.mcp.transports.urlopen", side_effect=fake_urlopen):
            transport.send_request("ping", {}, timeout=HTTP_TIMEOUT)
        self.assertEqual(captured[0].get("timeout"), 10.0)

    def test_jsonrpc_error_raises_protocol_error(self) -> None:
        def fake_urlopen(request, **kwargs):
            return FakeResponse(
                json.dumps(
                    {"jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "nope"}}
                ).encode()
            )

        transport = self._transport()
        with mock.patch("harness_hub_runtime.mcp.transports.urlopen", side_effect=fake_urlopen):
            with self.assertRaises(McpProtocolError):
                transport.send_request("missing", {}, timeout=5.0)

    def test_404_retries_once_without_session_header(self) -> None:
        from urllib.error import HTTPError

        captured: list = []
        success = FakeResponse(json.dumps({"jsonrpc": "2.0", "id": 1, "result": {}}).encode())

        responses = [
            FakeResponse(b"", headers={"Mcp-Session-Id": "old"}),  # 第一次建立 session
        ]

        def fake_urlopen(request, **kwargs):
            captured.append(request)
            if len(captured) == 1:
                return responses[0]
            if len(captured) == 2:
                raise HTTPError("https://mcp.example.com/rpc", 404, "Not Found", None, None)
            return success

        transport = self._transport()
        with mock.patch("harness_hub_runtime.mcp.transports.urlopen", side_effect=fake_urlopen):
            transport.send_request("initialize", {}, timeout=5.0)  # 建立 session
            result = transport.send_request("tools/list", {}, timeout=5.0)  # 404 → 重试
            self.assertEqual(result, {})

        self.assertEqual(len(captured), 3)
        second_headers = {k.lower(): v for k, v in captured[1].header_items()}
        third_headers = {k.lower(): v for k, v in captured[2].header_items()}
        self.assertIn("mcp-session-id", second_headers)
        # 重试必须去掉 session 头（server 不支持 session 时的降级路径）。
        self.assertNotIn("mcp-session-id", third_headers)

    def test_5xx_never_retried_and_raises_transport_error(self) -> None:
        from urllib.error import HTTPError

        calls = 0

        def fake_urlopen(request, **kwargs):
            nonlocal calls
            calls += 1
            raise HTTPError("https://mcp.example.com/rpc", 500, "boom", None, None)

        transport = self._transport()
        with mock.patch("harness_hub_runtime.mcp.transports.urlopen", side_effect=fake_urlopen):
            with self.assertRaises(McpTransportError):
                transport.send_request("ping", {}, timeout=5.0)
        self.assertEqual(calls, 1)

    def test_notification_posts_without_id_and_tolerates_empty_body(self) -> None:
        captured: list = []

        def fake_urlopen(request, **kwargs):
            captured.append(request)
            return FakeResponse(b"", content_type="application/json")  # 202 Accepted 语义

        transport = self._transport()
        with mock.patch("harness_hub_runtime.mcp.transports.urlopen", side_effect=fake_urlopen):
            transport.send_notification("notifications/initialized", {})

        payload = json.loads(captured[0].data)
        self.assertEqual(payload["method"], "notifications/initialized")
        self.assertNotIn("id", payload)

    def test_close_is_noop_and_never_raises(self) -> None:
        transport = self._transport()
        transport.close()
        transport.close()


if __name__ == "__main__":
    unittest.main()
