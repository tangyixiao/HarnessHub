"""`harness_hub_runtime.rpc` 的单元测试。

运行方式（仓库根目录）：

    uv run --project python python -m unittest discover -s python/tests -t python -v
"""

from __future__ import annotations

import importlib.util
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
# Rust 握手的 requiredCapabilities 仍固定为 F20 三项（spec §3.1），不随 F21 能力集扩展。
F20_REQUIRED_CAPABILITIES = ["ping", "runtime.info", "shutdown"]
# F21 冻结清单（docs/specs/2026-11-05-f21-provider-gateway-design.md §3.2），顺序有意义。
CAPABILITIES = [
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
METHODS = [
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
TRUTH_PRECEDENCE = ["provider_reported", "provider_count_api", "local_exact", "estimated"]
LOCAL_LEVELS = ["local_exact", "estimated"]
PROVIDER_IDS = ["litellm", "openai", "anthropic", "google"]


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


def rpc(method: str, params: object = None) -> dict:
    """按方法名派发一条带 params 的请求（F21 新方法统一走线协议）。"""
    return call({"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}})


def _sdk_importable(module: str) -> bool:
    """与 server.py 的 probe 同款判定：find_spec，不 import，所有异常视为不可用。

    这样测试与「可选 SDK 是否安装」解耦：本环境 openai 已装、其余未装，合并 S1 后仍成立。
    """
    try:
        return importlib.util.find_spec(module) is not None
    except BaseException:  # noqa: BLE001 - google.generativeai 会因父包缺失抛 ModuleNotFoundError
        return False


class F21CapabilityTests(unittest.TestCase):
    """§3.2 冻结能力/方法清单，以及 §3.1 F20 握手向后兼容。"""

    def initialize(self, **params: object) -> dict:
        request = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "hostVersion": "0.5.0",
                "protocolVersion": "1.0",
                # Rust 侧仍只要求 F20 三项；F21 能力集是其超集。
                "requiredCapabilities": F20_REQUIRED_CAPABILITIES,
                **params,
            },
        }
        return call(request)

    def test_protocol_version_is_still_1_0(self) -> None:
        self.assertEqual(rpc_server.PROTOCOL_VERSION, "1.0")

    def test_frozen_static_constants_match_contract(self) -> None:
        self.assertEqual(rpc_server.SUPPORTED_CAPABILITIES, CAPABILITIES)
        self.assertEqual(rpc_server.GATEWAY_STATIC_CAPABILITIES, CAPABILITIES)
        self.assertEqual(rpc_server.GATEWAY_STATIC_METHODS, METHODS)
        self.assertEqual(list(DEFAULT_METHODS), METHODS)

    def test_runtime_info_returns_new_capabilities_and_methods(self) -> None:
        result = rpc("runtime.info")["result"]

        self.assertEqual(set(result), RUNTIME_INFO_KEYS)
        self.assertEqual(result["providers"], [])
        self.assertEqual(result["capabilities"], CAPABILITIES)
        self.assertEqual(result["methods"], METHODS)

    def test_initialize_returns_four_keys_with_new_lists(self) -> None:
        response = self.initialize()

        self.assertNotIn("error", response)
        self.assertEqual(
            set(response["result"]), {"protocolVersion", "runtime", "capabilities", "methods"}
        )
        self.assertEqual(response["result"]["protocolVersion"], "1.0")
        self.assertEqual(response["result"]["capabilities"], CAPABILITIES)
        self.assertEqual(response["result"]["methods"], METHODS)

    def test_initialize_runtime_identical_to_runtime_info(self) -> None:
        init_runtime = self.initialize()["result"]["runtime"]
        info_runtime = rpc("runtime.info")["result"]

        self.assertEqual(init_runtime, info_runtime)
        self.assertEqual(set(init_runtime), RUNTIME_INFO_KEYS)

    def test_f20_handshake_three_capabilities_succeeds(self) -> None:
        response = self.initialize(requiredCapabilities=["ping", "runtime.info", "shutdown"])

        self.assertNotIn("error", response)
        self.assertEqual(response["result"]["capabilities"], CAPABILITIES)

    def test_f20_handshake_via_handle_request_directly(self) -> None:
        request = {
            "jsonrpc": "2.0",
            "id": "handshake",
            "method": "initialize",
            "params": {
                "hostVersion": "0.5.0",
                "protocolVersion": "1.0",
                "requiredCapabilities": ["ping", "runtime.info", "shutdown"],
            },
        }
        response = handle_request(request, DEFAULT_METHODS)

        assert response is not None
        self.assertNotIn("error", response)
        self.assertEqual(response["result"]["runtime"]["capabilities"], CAPABILITIES)

    def test_f20_handshake_via_serve_succeeds(self) -> None:
        init_params = {
            "hostVersion": "0.5.0",
            "protocolVersion": "1.0",
            "requiredCapabilities": F20_REQUIRED_CAPABILITIES,
        }
        responses = serve_responses(
            [
                json.dumps(
                    {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": init_params}
                )
            ]
        )

        self.assertEqual(len(responses), 1)
        self.assertNotIn("error", responses[0])
        self.assertEqual(responses[0]["result"]["capabilities"], CAPABILITIES)

    def test_unknown_required_capability_is_invalid_request(self) -> None:
        response = self.initialize(requiredCapabilities=["ping", "gateway.does.not.exist"])

        self.assertEqual(response["error"]["code"], INVALID_REQUEST)
        self.assertIn("gateway.does.not.exist", response["error"]["message"])

    def test_major_version_mismatch_is_invalid_request(self) -> None:
        response = self.initialize(protocolVersion="2.0")

        self.assertEqual(response["error"]["code"], INVALID_REQUEST)
        self.assertIn("2.0", response["error"]["message"])

    def test_minor_version_1_9_is_accepted(self) -> None:
        response = self.initialize(protocolVersion="1.9")

        self.assertNotIn("error", response)
        self.assertEqual(response["result"]["protocolVersion"], "1.0")


class F21GatewayStatusTests(unittest.TestCase):
    """§3.3 gateway.status / §3.4 gateway.providers 冻结形状（自包含、绝不回显原文）。"""

    SDK_MODULES = ["litellm", "openai", "anthropic", "google.generativeai"]
    DESCRIPTOR_IDS = ["litellm", "openai", "anthropic", "google"]
    DESCRIPTOR_MODES = ["unified", "native", "native", "native"]

    def test_status_full_exact_shape(self) -> None:
        response = rpc("gateway.status")

        self.assertNotIn("error", response)
        result = response["result"]
        self.assertEqual(
            set(result),
            {
                "state",
                "mode",
                "protocolVersion",
                "providers",
                "tokenization",
                "capabilities",
                "methods",
                "lastError",
            },
        )
        self.assertEqual(result["state"], "ready")
        self.assertEqual(result["mode"], "unified")
        self.assertEqual(result["protocolVersion"], "1.0")
        self.assertEqual(result["capabilities"], CAPABILITIES)
        self.assertEqual(result["methods"], METHODS)
        self.assertIsNone(result["lastError"])

    def test_status_tokenization_block(self) -> None:
        result = rpc("gateway.status")["result"]
        tok = result["tokenization"]

        self.assertEqual(set(tok), {"precedence", "levels", "revisionBinding"})
        self.assertEqual(tok["precedence"], TRUTH_PRECEDENCE)
        self.assertEqual(tok["levels"], LOCAL_LEVELS)
        self.assertIs(tok["revisionBinding"], True)

    def test_status_providers_descriptors(self) -> None:
        result = rpc("gateway.status")["result"]
        providers = result["providers"]

        self.assertEqual(len(providers), 4)
        for descriptor, module, pid, mode in zip(
            providers, self.SDK_MODULES, self.DESCRIPTOR_IDS, self.DESCRIPTOR_MODES
        ):
            self.assertEqual(set(descriptor), {"id", "mode", "importable", "detail"})
            self.assertEqual(descriptor["id"], pid)
            self.assertEqual(descriptor["mode"], mode)
            expected = _sdk_importable(module)
            self.assertIsInstance(descriptor["importable"], bool)
            self.assertEqual(descriptor["importable"], expected)
            self.assertEqual(
                descriptor["detail"],
                "optional SDK available" if expected else "optional SDK not installed",
            )

    def test_state_is_ready_even_when_sdks_missing(self) -> None:
        # ADR-0005：capability != readiness，可选 SDK 缺失不影响 state。
        self.assertEqual(rpc("gateway.status")["result"]["state"], "ready")

    def test_status_never_contains_raw_content_keys(self) -> None:
        result = rpc("gateway.status")["result"]
        forbidden = {"prompt", "messages", "response", "content"}

        def walk(node: object) -> None:
            if isinstance(node, dict):
                for key, value in node.items():
                    self.assertNotIn(key, forbidden)
                    walk(value)
            elif isinstance(node, list):
                for item in node:
                    walk(item)

        walk(result)

    def test_status_rejects_raw_content_keys(self) -> None:
        for key in ("prompt", "messages", "response", "content"):
            response = rpc("gateway.status", {key: "UNIQUE_MARKER_原文"})
            self.assertEqual(response["error"]["code"], INVALID_REQUEST)
            self.assertIn("原文内容", response["error"]["message"])

    def test_providers_equals_status_providers(self) -> None:
        status = rpc("gateway.status")["result"]
        providers = rpc("gateway.providers")["result"]

        self.assertEqual(providers, status["providers"])

    def test_providers_rejects_raw_content_keys(self) -> None:
        response = rpc("gateway.providers", {"prompt": "UNIQUE_MARKER_原文"})
        self.assertEqual(response["error"]["code"], INVALID_REQUEST)

    def test_status_accepts_arbitrary_dict_params(self) -> None:
        response = rpc("gateway.status", {"someFlag": True, "trace": "abc"})
        self.assertNotIn("error", response)


class F21RouteTests(unittest.TestCase):
    """§3.5 gateway.route 四条规则 + 安全闸。"""

    def route(self, **params: object) -> dict:
        return rpc("gateway.route", params)

    def test_result_key_set(self) -> None:
        response = self.route(provider="openai")
        self.assertNotIn("error", response)
        self.assertEqual(set(response["result"]), {"mode", "provider", "reason", "fallback"})

    def test_native_only_capability(self) -> None:
        for capability in (
            "responses.streaming",
            "anthropic.thinking",
            "google.grounding",
            "provider.raw_request",
        ):
            result = self.route(provider="openai", capability=capability)["result"]
            self.assertEqual(result["mode"], "native")
            self.assertEqual(result["reason"], "native_only_capability")
            self.assertEqual(result["provider"], "openai")
            self.assertEqual(result["fallback"], "litellm")

    def test_explicit_native(self) -> None:
        result = self.route(provider="openai", requestedMode="native")["result"]

        self.assertEqual(result["mode"], "native")
        self.assertEqual(result["reason"], "explicit_native")
        self.assertEqual(result["fallback"], "litellm")

    def test_unknown_provider_is_echoed_verbatim(self) -> None:
        provider = "mystery-provider-xyz"
        result = self.route(provider=provider)["result"]

        self.assertEqual(result["mode"], "unified")
        self.assertEqual(result["reason"], "unknown_provider")
        self.assertEqual(result["provider"], provider)
        self.assertEqual(result["fallback"], "litellm")

    def test_litellm_first_for_known_providers(self) -> None:
        for provider in PROVIDER_IDS:
            result = self.route(provider=provider, requestedMode="unified")["result"]
            self.assertEqual(result["mode"], "unified")
            self.assertEqual(result["reason"], "litellm_first")
            self.assertEqual(result["provider"], provider)
            self.assertEqual(result["fallback"], "litellm")

    def test_missing_provider_defaults_to_empty_string(self) -> None:
        result = self.route()["result"]

        self.assertEqual(result["provider"], "")
        self.assertEqual(result["reason"], "unknown_provider")
        self.assertEqual(result["fallback"], "litellm")

    def test_all_four_reasons_and_fallback_always_litellm(self) -> None:
        cases = [
            {"provider": "openai", "capability": "anthropic.thinking"},
            {"provider": "openai", "requestedMode": "native"},
            {"provider": "mystery-provider"},
            {"provider": "openai", "requestedMode": "unified"},
        ]
        reasons = set()
        for case in cases:
            result = self.route(**case)["result"]
            reasons.add(result["reason"])
            self.assertEqual(result["fallback"], "litellm")

        self.assertEqual(
            reasons,
            {
                "native_only_capability",
                "explicit_native",
                "unknown_provider",
                "litellm_first",
            },
        )

    def test_native_only_takes_priority_over_explicit_unified(self) -> None:
        result = self.route(
            provider="mystery", requestedMode="unified", capability="google.grounding"
        )["result"]

        self.assertEqual(result["mode"], "native")
        self.assertEqual(result["reason"], "native_only_capability")
        self.assertEqual(result["provider"], "mystery")

    def test_raw_content_keys_are_rejected(self) -> None:
        for key in ("prompt", "messages", "response", "content"):
            response = self.route(**{key: "UNIQUE_MARKER_原文"})
            self.assertEqual(response["error"]["code"], INVALID_REQUEST)
            self.assertIn("route 不接受原文内容", response["error"]["message"])


class F21NormalizeTests(unittest.TestCase):
    """§3.6 归一化：冻结键集 + 不回显原始 event/error 对象。"""

    def test_stream_exact_keys_and_unknown_fallback(self) -> None:
        marker = "UNIQUE_STREAM_MARKER_9f3a"
        result = rpc(
            "gateway.normalize.stream",
            {"provider": "openai", "event": {"type": marker, "delta": marker}},
        )["result"]

        self.assertEqual(set(result), {"type", "textDelta", "finishReason", "usage", "rawProvider"})
        self.assertEqual(result["type"], "unknown")
        self.assertIsNone(result["textDelta"])
        self.assertIsNone(result["finishReason"])
        self.assertIsNone(result["usage"])
        self.assertEqual(result["rawProvider"], "openai")
        self.assertNotIn(marker, str(result))

    def test_stream_missing_provider_defaults_to_empty_string(self) -> None:
        result = rpc("gateway.normalize.stream", {"event": {}})["result"]
        self.assertEqual(result["rawProvider"], "")

    def test_error_exact_keys_and_unknown_fallback(self) -> None:
        marker = "UNIQUE_ERROR_MARKER_7b21"
        result = rpc(
            "gateway.normalize.error",
            {"provider": "anthropic", "error": {"message": marker, "stack": marker}},
        )["result"]

        self.assertEqual(set(result), {"kind", "retryable", "message", "provider", "httpStatus"})
        self.assertEqual(result["kind"], "unknown")
        self.assertFalse(result["retryable"])
        self.assertEqual(result["message"], "")
        self.assertEqual(result["provider"], "anthropic")
        self.assertIsNone(result["httpStatus"])
        self.assertNotIn(marker, str(result))

    def test_error_missing_provider_defaults_to_empty_string(self) -> None:
        result = rpc("gateway.normalize.error", {"error": {}})["result"]
        self.assertEqual(result["provider"], "")

    def test_normalize_rejects_non_object_params(self) -> None:
        for bad in ("raw-string", [1, 2, 3], 42):
            response = rpc("gateway.normalize.stream", bad)
            self.assertEqual(response["error"]["code"], INVALID_REQUEST)


class F21TokenizationTests(unittest.TestCase):
    """§3.7 / §5 tokenization 三个方法。"""

    ESTIMATE_KEYS = {
        "inputTokens",
        "outputTokens",
        "totalTokens",
        "truthLevel",
        "tokenizerModel",
        "revision",
        "estimated",
    }

    def test_precedence_exact(self) -> None:
        result = rpc("tokenization.precedence")["result"]

        self.assertEqual(set(result), {"precedence", "levels", "revisionBinding"})
        self.assertEqual(result["precedence"], TRUTH_PRECEDENCE)
        self.assertEqual(result["levels"], LOCAL_LEVELS)
        self.assertIs(result["revisionBinding"], True)

    def test_estimate_gpt4o_120_is_estimated(self) -> None:
        result = rpc("tokenization.estimate", {"textLength": 120, "model": "gpt-4o"})["result"]

        self.assertEqual(set(result), self.ESTIMATE_KEYS)
        self.assertEqual(result["inputTokens"], 30)
        self.assertEqual(result["outputTokens"], 0)
        self.assertEqual(result["totalTokens"], 30)
        self.assertEqual(result["truthLevel"], "estimated")
        self.assertEqual(result["tokenizerModel"], "o200k_base")
        self.assertEqual(result["revision"], "2024-05-13")
        self.assertIs(result["estimated"], True)

    def test_estimate_reported_usage(self) -> None:
        result = rpc(
            "tokenization.estimate",
            {
                "textLength": 120,
                "model": "gpt-4o",
                "reportedUsage": {"inputTokens": 11, "outputTokens": 4},
            },
        )["result"]

        self.assertEqual(set(result), self.ESTIMATE_KEYS)
        self.assertEqual(result["truthLevel"], "provider_reported")
        self.assertEqual(result["inputTokens"], 11)
        self.assertEqual(result["outputTokens"], 4)
        self.assertEqual(result["totalTokens"], 15)
        self.assertEqual(result["tokenizerModel"], "o200k_base")
        self.assertEqual(result["revision"], "2024-05-13")
        self.assertIs(result["estimated"], False)

    def test_estimate_count_api_usage(self) -> None:
        result = rpc(
            "tokenization.estimate",
            {
                "textLength": 120,
                "model": "gpt-4o",
                "countApiUsage": {"inputTokens": 7, "outputTokens": 2},
            },
        )["result"]

        self.assertEqual(result["truthLevel"], "provider_count_api")
        self.assertEqual(result["inputTokens"], 7)
        self.assertEqual(result["outputTokens"], 2)
        self.assertEqual(result["totalTokens"], 9)
        self.assertIs(result["estimated"], False)

    def test_reported_usage_wins_over_count_api_usage(self) -> None:
        result = rpc(
            "tokenization.estimate",
            {
                "textLength": 120,
                "model": "gpt-4o",
                "reportedUsage": {"inputTokens": 11, "outputTokens": 4},
                "countApiUsage": {"inputTokens": 7, "outputTokens": 2},
            },
        )["result"]

        self.assertEqual(result["truthLevel"], "provider_reported")
        self.assertEqual(result["inputTokens"], 11)
        self.assertEqual(result["outputTokens"], 4)

    def test_estimate_rejects_raw_text_keys(self) -> None:
        for key in ("text", "prompt", "content"):
            response = rpc(
                "tokenization.estimate", {"textLength": 120, "model": "gpt-4o", key: "MARKER"}
            )
            self.assertEqual(response["error"]["code"], INVALID_REQUEST)

    def test_estimate_rejects_bad_text_length(self) -> None:
        for bad in ("120", 1.5, -1, None, True, False):
            response = rpc("tokenization.estimate", {"textLength": bad, "model": "gpt-4o"})
            self.assertEqual(response["error"]["code"], INVALID_REQUEST)

    def test_estimate_unknown_model_has_unknown_revision(self) -> None:
        result = rpc("tokenization.estimate", {"textLength": 8, "model": "mystery-model"})["result"]

        self.assertEqual(result["tokenizerModel"], "unknown")
        self.assertEqual(result["revision"], "unknown")
        self.assertEqual(result["truthLevel"], "estimated")

    def test_revision_known_models(self) -> None:
        cases = {
            "gpt-4o": ("o200k_base", "2024-05-13"),
            "gpt-4-turbo": ("cl100k_base", "2023-06-13"),
            "gpt-3.5-turbo": ("cl100k_base", "2023-06-13"),
            "claude-3-5-sonnet": ("anthropic-claude", "2024-06-20"),
            "gemini-1.5-pro": ("sentencepiece-gemini", "2024-09-24"),
        }
        for model, (tokenizer_model, revision) in cases.items():
            result = rpc("tokenization.revision", {"model": model})["result"]

            self.assertEqual(set(result), {"model", "tokenizerModel", "revision", "bound", "reason"})
            self.assertEqual(result["model"], model)
            self.assertEqual(result["tokenizerModel"], tokenizer_model)
            self.assertEqual(result["revision"], revision)
            self.assertIs(result["bound"], True)
            self.assertEqual(result["reason"], "known_model")

    def test_revision_unknown_with_explicit_tokenizer(self) -> None:
        result = rpc(
            "tokenization.revision", {"model": "mystery", "tokenizerModel": "my-own"}
        )["result"]

        self.assertEqual(set(result), {"model", "tokenizerModel", "revision", "bound", "reason"})
        self.assertEqual(result["tokenizerModel"], "my-own")
        self.assertEqual(result["revision"], "unknown")
        self.assertIs(result["bound"], False)
        self.assertEqual(result["reason"], "explicit_unverified")

    def test_revision_unknown_without_tokenizer(self) -> None:
        result = rpc("tokenization.revision", {"model": "mystery"})["result"]

        self.assertEqual(result["tokenizerModel"], "unknown")
        self.assertEqual(result["revision"], "unknown")
        self.assertIs(result["bound"], False)
        self.assertEqual(result["reason"], "unknown_model")


class F21RegressionTests(unittest.TestCase):
    """F20 线协议回归：方法表扩展后 serve/shutdown/错误处理不变。"""

    def test_all_frozen_methods_are_registered_in_order(self) -> None:
        self.assertEqual(list(DEFAULT_METHODS), METHODS)

    def test_all_f20_methods_still_registered(self) -> None:
        for method in ("ping", "runtime.info", "initialize", "shutdown"):
            self.assertIn(method, DEFAULT_METHODS)

    def test_ping_is_unchanged(self) -> None:
        self.assertEqual(rpc("ping")["result"], {"pong": True})

    def test_unknown_method_still_method_not_found(self) -> None:
        self.assertEqual(rpc("does.not.exist")["error"]["code"], METHOD_NOT_FOUND)

    def test_params_must_be_object(self) -> None:
        for bad in ("raw-string", [1, 2, 3], 42):
            self.assertEqual(rpc("gateway.status", bad)["error"]["code"], INVALID_REQUEST)

    def test_serve_stops_on_shutdown_request(self) -> None:
        responses = serve_responses(
            [
                json.dumps({"jsonrpc": "2.0", "id": 1, "method": "gateway.status"}),
                json.dumps({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
                json.dumps({"jsonrpc": "2.0", "id": 3, "method": "ping"}),
            ]
        )

        self.assertEqual([response["id"] for response in responses], [1, 2])
        self.assertEqual(responses[1]["result"], {"stopping": True})

    def test_shutdown_notification_produces_no_response(self) -> None:
        responses = serve_responses(
            [
                json.dumps({"jsonrpc": "2.0", "method": "shutdown"}),
                json.dumps({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
            ]
        )

        self.assertEqual(responses, [])


if __name__ == "__main__":
    unittest.main()
