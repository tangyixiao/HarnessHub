"""`harness_hub_runtime.rpc` 的单元测试。

运行方式（仓库根目录）：

    uv run --project python python -m unittest discover -s python/tests -t python -v
"""

from __future__ import annotations

import io
import json
import unittest

from harness_hub_runtime.rpc.server import (
    DEFAULT_METHODS,
    INTERNAL_ERROR,
    INVALID_REQUEST,
    METHOD_NOT_FOUND,
    PARSE_ERROR,
    RpcError,
    handle_request,
    serve,
)


def call(request: dict) -> dict:
    """断言请求一定产生响应（非通知），并返回该响应。"""
    response = handle_request(request, DEFAULT_METHODS)
    assert response is not None, "该请求应当产生响应"
    return response


class HandleRequestTests(unittest.TestCase):
    def test_ping_returns_pong(self) -> None:
        response = call({"jsonrpc": "2.0", "id": 1, "method": "ping"})

        self.assertEqual(response["jsonrpc"], "2.0")
        self.assertEqual(response["id"], 1)
        self.assertEqual(response["result"], {"pong": True})
        self.assertNotIn("error", response)

    def test_runtime_info_reports_transport_and_no_providers(self) -> None:
        result = call({"jsonrpc": "2.0", "id": "abc", "method": "runtime.info"})["result"]

        self.assertEqual(result["name"], "harness-hub-runtime")
        self.assertEqual(result["transport"], "stdio-jsonrpc")
        self.assertEqual(result["providers"], [])

    def test_unknown_method_returns_method_not_found(self) -> None:
        response = call({"jsonrpc": "2.0", "id": 2, "method": "does.not.exist"})

        self.assertEqual(response["error"]["code"], METHOD_NOT_FOUND)

    def test_missing_jsonrpc_version_is_invalid_request(self) -> None:
        response = call({"id": 3, "method": "ping"})

        self.assertEqual(response["error"]["code"], INVALID_REQUEST)
        self.assertEqual(response["id"], 3)

    def test_non_object_request_is_invalid_request(self) -> None:
        response = handle_request(["not", "an", "object"], DEFAULT_METHODS)

        assert response is not None
        self.assertEqual(response["error"]["code"], INVALID_REQUEST)
        self.assertIsNone(response["id"])

    def test_notification_produces_no_response(self) -> None:
        response = handle_request({"jsonrpc": "2.0", "method": "ping"}, DEFAULT_METHODS)

        self.assertIsNone(response, "通知（无 id）不应回包")

    def test_handler_raising_rpc_error_maps_to_error_object(self) -> None:
        def failing(_params: dict) -> dict:
            raise RpcError(-32000, "provider 未配置")

        response = handle_request(
            {"jsonrpc": "2.0", "id": 4, "method": "boom"},
            {"boom": failing},
        )

        assert response is not None
        self.assertEqual(response["error"], {"code": -32000, "message": "provider 未配置"})

    def test_unexpected_exception_becomes_internal_error(self) -> None:
        def exploding(_params: dict) -> dict:
            raise RuntimeError("内部炸了")

        response = handle_request(
            {"jsonrpc": "2.0", "id": 5, "method": "boom"},
            {"boom": exploding},
        )

        assert response is not None
        self.assertEqual(response["error"]["code"], INTERNAL_ERROR)
        self.assertIn("内部炸了", response["error"]["message"])


class ServeTests(unittest.TestCase):
    def run_serve(self, lines: list[str]) -> list[dict]:
        stdin = io.StringIO("\n".join(lines) + "\n")
        stdout = io.StringIO()
        serve(stdin, stdout)
        return [json.loads(line) for line in stdout.getvalue().splitlines() if line.strip()]

    def test_serve_processes_multiple_requests_in_order(self) -> None:
        responses = self.run_serve(
            [
                json.dumps({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
                json.dumps({"jsonrpc": "2.0", "id": 2, "method": "runtime.info"}),
            ]
        )

        self.assertEqual([response["id"] for response in responses], [1, 2])

    def test_serve_ignores_blank_lines(self) -> None:
        responses = self.run_serve(["", "   ", json.dumps({"jsonrpc": "2.0", "id": 1, "method": "ping"})])

        self.assertEqual(len(responses), 1)

    def test_serve_reports_parse_error_for_malformed_json(self) -> None:
        responses = self.run_serve(["{not json"])

        self.assertEqual(responses[0]["error"]["code"], PARSE_ERROR)

    def test_serve_continues_after_a_bad_line(self) -> None:
        responses = self.run_serve(
            [
                "{not json",
                json.dumps({"jsonrpc": "2.0", "id": 9, "method": "ping"}),
            ]
        )

        self.assertEqual(len(responses), 2)
        self.assertEqual(responses[1]["result"], {"pong": True})


if __name__ == "__main__":
    unittest.main()
