"""`harness_hub_runtime.rpc` 的单元测试。

运行方式（仓库根目录）：

    uv run --project python python -m unittest discover -s python/tests -t python -v
"""

from __future__ import annotations

import io
import json
import unittest

import harness_hub_runtime
from harness_hub_runtime.rpc import server as rpc_server
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

# F20 线协议契约（docs/specs/2026-11-04-f20-sidecar-lifecycle-design.md §3.1）
PROTOCOL_VERSION = "1.0"
CAPABILITIES = ["ping", "runtime.info", "shutdown"]
METHODS = ["ping", "runtime.info", "initialize", "shutdown"]
RUNTIME_INFO_KEYS = {
    "name",
    "version",
    "transport",
    "python",
    "protocolVersion",
    "providers",
    "capabilities",
    "methods",
}


def call(request: dict) -> dict:
    """断言请求一定产生响应（非通知），并返回该响应。"""
    response = handle_request(request, DEFAULT_METHODS)
    assert response is not None, "该请求应当产生响应"
    return response


def serve_responses(lines: list[str]) -> list[dict]:
    """跑一遍 serve() 并解析写出的所有响应行。"""
    stdout = io.StringIO()
    serve(io.StringIO("\n".join(lines) + "\n"), stdout)
    return [json.loads(line) for line in stdout.getvalue().splitlines() if line.strip()]


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


class F20HandshakeTests(unittest.TestCase):
    """F20 线协议契约：docs/specs/2026-11-04-f20-sidecar-lifecycle-design.md §3.1 / §3.2 / §3.5。"""

    def initialize(self, **params: object) -> dict:
        request = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "hostVersion": "0.5.0",
                "protocolVersion": "1.0",
                "requiredCapabilities": CAPABILITIES,
                **params,
            },
        }
        return call(request)

    def test_package_version_is_0_5_0(self) -> None:
        self.assertEqual(harness_hub_runtime.__version__, "0.5.0")

    def test_protocol_constants_are_locked(self) -> None:
        self.assertEqual(rpc_server.PROTOCOL_VERSION, "1.0")
        self.assertEqual(rpc_server.SUPPORTED_CAPABILITIES, CAPABILITIES)

    def test_initialize_and_shutdown_are_registered_methods(self) -> None:
        self.assertIn("initialize", DEFAULT_METHODS)
        self.assertIn("shutdown", DEFAULT_METHODS)

    def test_runtime_info_returns_full_f20_shape(self) -> None:
        result = call({"jsonrpc": "2.0", "id": 1, "method": "runtime.info"})["result"]

        self.assertEqual(set(result), RUNTIME_INFO_KEYS)
        self.assertEqual(result["name"], "harness-hub-runtime")
        self.assertEqual(result["version"], "0.5.0")
        self.assertEqual(result["transport"], "stdio-jsonrpc")
        self.assertEqual(result["protocolVersion"], "1.0")
        self.assertEqual(result["providers"], [])
        self.assertEqual(result["capabilities"], CAPABILITIES)
        self.assertEqual(result["methods"], METHODS)

    def test_initialize_returns_four_contract_keys(self) -> None:
        response = self.initialize()

        self.assertNotIn("error", response)
        self.assertEqual(
            set(response["result"]), {"protocolVersion", "runtime", "capabilities", "methods"}
        )

        self.assertEqual(response["result"]["protocolVersion"], "1.0")
        self.assertEqual(response["result"]["capabilities"], CAPABILITIES)
        self.assertEqual(response["result"]["methods"], METHODS)

    def test_initialize_runtime_identical_to_runtime_info(self) -> None:
        """`runtime` 必须与 `runtime.info` 结果形状完全一致（同一构造函数）。"""
        init_runtime = self.initialize()["result"]["runtime"]
        info_runtime = call({"jsonrpc": "2.0", "id": 2, "method": "runtime.info"})["result"]

        self.assertEqual(init_runtime, info_runtime)
        self.assertEqual(set(init_runtime), RUNTIME_INFO_KEYS)

    def test_initialize_accepts_compatible_minor_version(self) -> None:
        """只比较主版本（`.` 前第一段），次版本不同不算不兼容。"""
        response = self.initialize(protocolVersion="1.9")

        self.assertNotIn("error", response)
        self.assertEqual(response["result"]["protocolVersion"], "1.0")

    def test_initialize_unknown_required_capability_is_invalid_request(self) -> None:
        response = self.initialize(requiredCapabilities=["ping", "does.not.exist"])

        self.assertEqual(response["error"]["code"], INVALID_REQUEST)
        self.assertIn("does.not.exist", response["error"]["message"])
        self.assertNotIn("result", response)

    def test_initialize_protocol_major_mismatch_is_invalid_request(self) -> None:
        response = self.initialize(protocolVersion="2.0")

        self.assertEqual(response["error"]["code"], INVALID_REQUEST)
        self.assertIn("protocolVersion", response["error"]["message"])
        self.assertIn("2.0", response["error"]["message"])


class F20ShutdownTests(unittest.TestCase):
    """F20 线协议契约：§3.4 shutdown / EOF。"""

    def test_shutdown_request_returns_stopping_and_exits_serve_loop(self) -> None:
        responses = serve_responses(
            [
                json.dumps({"jsonrpc": "2.0", "id": 1, "method": "shutdown"}),
                json.dumps({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
            ]
        )

        self.assertEqual(len(responses), 1, "shutdown 之后必须退出循环，ping 不应被处理")
        self.assertEqual(responses[0]["id"], 1)
        self.assertEqual(responses[0]["result"], {"stopping": True})

    def test_shutdown_notification_exits_serve_loop_without_response(self) -> None:
        responses = serve_responses(
            [
                json.dumps({"jsonrpc": "2.0", "method": "shutdown"}),
                json.dumps({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
            ]
        )

        self.assertEqual(responses, [], "shutdown 通知既不应回包，也应退出循环")

    def test_serve_continues_after_parse_error_then_stops_on_shutdown(self) -> None:
        responses = serve_responses(
            [
                "{not json",
                json.dumps({"jsonrpc": "2.0", "id": 7, "method": "shutdown"}),
                json.dumps({"jsonrpc": "2.0", "id": 8, "method": "ping"}),
            ]
        )

        self.assertEqual(len(responses), 2)
        self.assertEqual(responses[0]["error"]["code"], PARSE_ERROR)
        self.assertEqual(responses[1]["result"], {"stopping": True})

    def test_serve_returns_on_stdin_eof_without_raising(self) -> None:
        stdout = io.StringIO()

        serve(io.StringIO(""), stdout)

        self.assertEqual(stdout.getvalue(), "")


if __name__ == "__main__":
    unittest.main()
