"""F21 S1 — providers 契约层测试（冻结契约）。

事实来源：docs/specs/2026-11-05-f21-provider-gateway-design.md §3.5 / §3.6 / §4 / §8。
TDD：本文件先于实现提交，RED 证据须为真实失败输出。

安全闸：所有用例只使用本地构造的元数据 / 短字符串，不含任何 prompt / response 原文；
断言「归一化结果不回显原始输入对象」。
"""

from __future__ import annotations

import dataclasses
import json
import unittest
from typing import Any
from unittest import mock

from harness_hub_runtime.providers import (
    ERROR_KINDS,
    MODES,
    MODEL_EVENT_TYPES,
    NATIVE_ONLY_CAPABILITIES,
    PROVIDER_IDS,
    MAX_SHORT,
    MAX_TEXT_DELTA,
    ModelEvent,
    ProviderDescriptor,
    ProviderError,
    RouteDecision,
    normalize_error,
    normalize_stream_event,
    probe,
    probe_all,
    route,
)


class ConstantsTest(unittest.TestCase):
    def test_constants_exact_values(self) -> None:
        self.assertEqual(PROVIDER_IDS, ("litellm", "openai", "anthropic", "google"))
        self.assertEqual(MODES, ("unified", "native"))
        self.assertEqual(
            MODEL_EVENT_TYPES, ("start", "delta", "usage", "done", "error", "unknown")
        )
        self.assertEqual(
            ERROR_KINDS,
            (
                "auth",
                "rate_limit",
                "invalid_request",
                "context_length",
                "server",
                "timeout",
                "network",
                "unknown",
            ),
        )
        self.assertEqual(
            NATIVE_ONLY_CAPABILITIES,
            (
                "responses.streaming",
                "anthropic.thinking",
                "google.grounding",
                "provider.raw_request",
            ),
        )

    def test_safety_limits(self) -> None:
        self.assertEqual(MAX_TEXT_DELTA, 256)
        self.assertEqual(MAX_SHORT, 64)

    def test_all_constants_are_tuples(self) -> None:
        for const in (PROVIDER_IDS, MODES, MODEL_EVENT_TYPES, ERROR_KINDS, NATIVE_ONLY_CAPABILITIES):
            self.assertIsInstance(const, tuple)


class DtoTest(unittest.TestCase):
    def test_provider_descriptor_to_json_keys_and_order(self) -> None:
        d = ProviderDescriptor(id="litellm", mode="unified", importable=False, detail="nope")
        self.assertEqual(d.to_json(), {"id": "litellm", "mode": "unified", "importable": False, "detail": "nope"})
        self.assertEqual(list(d.to_json().keys()), ["id", "mode", "importable", "detail"])

    def test_model_event_to_json_keys_and_order(self) -> None:
        e = ModelEvent(type="delta", text_delta="hi", finish_reason=None, usage=None, raw_provider="openai")
        self.assertEqual(
            e.to_json(),
            {"type": "delta", "textDelta": "hi", "finishReason": None, "usage": None, "rawProvider": "openai"},
        )
        self.assertEqual(
            list(e.to_json().keys()), ["type", "textDelta", "finishReason", "usage", "rawProvider"]
        )

    def test_provider_error_to_json_keys_and_order(self) -> None:
        e = ProviderError(kind="rate_limit", retryable=True, message="m", provider="openai", http_status=429)
        self.assertEqual(
            e.to_json(),
            {"kind": "rate_limit", "retryable": True, "message": "m", "provider": "openai", "httpStatus": 429},
        )
        self.assertEqual(
            list(e.to_json().keys()), ["kind", "retryable", "message", "provider", "httpStatus"]
        )

    def test_route_decision_to_json_keys_and_order(self) -> None:
        r = RouteDecision(mode="native", provider="openai", reason="explicit_native", fallback="litellm")
        self.assertEqual(
            r.to_json(),
            {"mode": "native", "provider": "openai", "reason": "explicit_native", "fallback": "litellm"},
        )
        self.assertEqual(list(r.to_json().keys()), ["mode", "provider", "reason", "fallback"])

    def test_dto_to_json_is_json_serializable(self) -> None:
        for obj in (
            ProviderDescriptor("litellm", "unified", False, ""),
            ModelEvent("delta", "x", None, None, "openai"),
            ProviderError("unknown", False, "", "openai", None),
            RouteDecision("unified", "openai", "litellm_first", "litellm"),
        ):
            self.assertIsInstance(json.dumps(obj.to_json()), str)

    def test_all_dtos_are_frozen(self) -> None:
        samples = (
            (ProviderDescriptor("litellm", "unified", False, ""), "id"),
            (ModelEvent("delta", "x", None, None, "openai"), "type"),
            (ProviderError("unknown", False, "", "openai", None), "kind"),
            (RouteDecision("unified", "openai", "litellm_first", "litellm"), "mode"),
        )
        for obj, field in samples:
            with self.subTest(obj=type(obj).__name__):
                self.assertTrue(dataclasses.is_dataclass(obj))
                with self.assertRaises(dataclasses.FrozenInstanceError):
                    setattr(obj, field, "mutated")

    def test_dto_signatures(self) -> None:
        # 构造参数形状锁定（字段名、顺序即契约）。
        self.assertEqual(
            [f.name for f in dataclasses.fields(ProviderDescriptor)],
            ["id", "mode", "importable", "detail"],
        )
        self.assertEqual(
            [f.name for f in dataclasses.fields(ModelEvent)],
            ["type", "text_delta", "finish_reason", "usage", "raw_provider"],
        )
        self.assertEqual(
            [f.name for f in dataclasses.fields(ProviderError)],
            ["kind", "retryable", "message", "provider", "http_status"],
        )
        self.assertEqual(
            [f.name for f in dataclasses.fields(RouteDecision)],
            ["mode", "provider", "reason", "fallback"],
        )


class StreamingTest(unittest.TestCase):
    def test_openai_delta(self) -> None:
        e = normalize_stream_event("openai", {"choices": [{"delta": {"content": "hi"}}]})
        self.assertEqual(e.type, "delta")
        self.assertEqual(e.text_delta, "hi")
        self.assertIsNone(e.finish_reason)
        self.assertIsNone(e.usage)
        self.assertEqual(e.raw_provider, "openai")

    def test_anthropic_delta(self) -> None:
        e = normalize_stream_event("anthropic", {"type": "content_block_delta", "delta": {"text": "yo"}})
        self.assertEqual(e.type, "delta")
        self.assertEqual(e.text_delta, "yo")

    def test_google_delta(self) -> None:
        e = normalize_stream_event("google", {"candidates": [{"content": {"parts": [{"text": "gm"}]}}]})
        self.assertEqual(e.type, "delta")
        self.assertEqual(e.text_delta, "gm")

    def test_start_markers(self) -> None:
        for payload in ({"type": "message_start"}, {"type": "start"}):
            with self.subTest(payload=payload):
                e = normalize_stream_event("anthropic", payload)
                self.assertEqual(e.type, "start")
                self.assertIsNone(e.text_delta)
                self.assertIsNone(e.finish_reason)
                self.assertIsNone(e.usage)

    def test_error_key_wins_first(self) -> None:
        # error 键优先级最高，即使同时带 delta 特征。
        e = normalize_stream_event("openai", {"error": "boom", "choices": [{"delta": {"content": "x"}}]})
        self.assertEqual(e.type, "error")
        self.assertIsNone(e.text_delta)
        self.assertIsNone(e.finish_reason)
        self.assertIsNone(e.usage)

    def test_usage_openai_integer_filtering(self) -> None:
        e = normalize_stream_event("openai", {"usage": {"prompt_tokens": 10, "completion_tokens": 3}})
        self.assertEqual(e.type, "usage")
        self.assertEqual(e.usage, {"inputTokens": 10, "outputTokens": 3})

    def test_usage_anthropic_names(self) -> None:
        e = normalize_stream_event("anthropic", {"usage": {"input_tokens": 7, "output_tokens": 2}})
        self.assertEqual(e.type, "usage")
        self.assertEqual(e.usage, {"inputTokens": 7, "outputTokens": 2})

    def test_usage_google_metadata(self) -> None:
        e = normalize_stream_event("google", {"usageMetadata": {"promptTokenCount": 5, "candidatesTokenCount": 9}})
        self.assertEqual(e.type, "usage")
        self.assertEqual(e.usage, {"inputTokens": 5, "outputTokens": 9})

    def test_usage_bad_values_become_none_and_sanitized(self) -> None:
        cases = (
            {"usage": {"prompt_tokens": "ten", "completion_tokens": 3}},
            {"usage": None},
            {"usage": "junk"},
            {"usage": {"prompt_tokens": 12}},  # 缺一个 -> None
            {"usage": {"prompt_tokens": 1.5, "completion_tokens": 2}},
        )
        for payload in cases:
            with self.subTest(payload=payload):
                e = normalize_stream_event("openai", payload)
                self.assertEqual(e.type, "usage")
                self.assertIsNone(e.usage)
        # 含 secret / 长文本也不能回显到结果里（脱敏）。
        payload = {"usage": {"prompt_tokens": 10, "completion_tokens": 3, "secret": "do-not-leak"}}
        out = normalize_stream_event("openai", payload).to_json()
        self.assertNotIn("secret", json.dumps(out))

    def test_usage_never_carries_raw_object(self) -> None:
        payload = {"usage": {"prompt_tokens": 10, "completion_tokens": 3, "secret": "do-not-leak"}}
        out = normalize_stream_event("openai", payload).to_json()
        self.assertNotIn("secret", json.dumps(out))

    def test_done_openai_finish_reason(self) -> None:
        e = normalize_stream_event("openai", {"choices": [{"finish_reason": "stop"}]})
        self.assertEqual(e.type, "done")
        self.assertEqual(e.finish_reason, "stop")
        self.assertIsNone(e.text_delta)

    def test_done_anthropic_message_stop(self) -> None:
        e = normalize_stream_event("anthropic", {"type": "message_stop"})
        self.assertEqual(e.type, "done")
        self.assertIsNone(e.finish_reason)

    def test_text_delta_truncated_to_256(self) -> None:
        long = "A" * 1000
        e = normalize_stream_event("openai", {"choices": [{"delta": {"content": long}}]})
        self.assertEqual(e.text_delta, "A" * MAX_TEXT_DELTA)
        self.assertEqual(len(e.text_delta), 256)

    def test_finish_reason_truncated_to_64(self) -> None:
        e = normalize_stream_event("openai", {"choices": [{"finish_reason": "F" * 100}]})
        self.assertEqual(len(e.finish_reason), MAX_SHORT)

    def test_raw_provider_truncated_to_64(self) -> None:
        e = normalize_stream_event("P" * 100, {"type": "start"})
        self.assertEqual(len(e.raw_provider), 64)

    def test_unknown_event(self) -> None:
        e = normalize_stream_event("openai", {"nope": 1})
        self.assertEqual(e.type, "unknown")
        self.assertIsNone(e.text_delta)
        self.assertIsNone(e.finish_reason)
        self.assertIsNone(e.usage)

    def test_malformed_inputs_never_raise(self) -> None:
        bad: list[Any] = [
            None,
            [],
            "str",
            {"choices": "bad"},
            {"choices": []},
            {"choices": [None]},
            {"choices": [{"delta": "bad"}]},
            {"candidates": "bad"},
            {"candidates": [{"content": {"parts": "bad"}}]},
            {"nested": {"junk": [1, 2, {"deep": None}]}},
            123,
            {"usage": {"prompt_tokens": object()}},  # usage 键存在 -> "usage"，但非法值 -> usage=None
            {"type": "content_block_delta", "delta": "bad"},
            {"error": {"message": "x" * 500}},  # error 键存在 -> "error"（不回显原文）
        ]
        for payload in bad:
            with self.subTest(payload=repr(payload)[:40]):
                e = normalize_stream_event("openai", payload)
                expected = "usage" if isinstance(payload, dict) and "usage" in payload else (
                    "error" if isinstance(payload, dict) and "error" in payload else "unknown"
                )
                self.assertEqual(e.type, expected)
                self.assertIsNone(e.text_delta)
                self.assertIsNone(e.finish_reason)
                self.assertIsNone(e.usage)

    def test_never_returns_input_object(self) -> None:
        payload = {"choices": [{"delta": {"content": "hi"}}], "secret": "do-not-leak"}
        e = normalize_stream_event("openai", payload)
        self.assertIsNot(e, payload)
        self.assertNotIn("secret", json.dumps(e.to_json()))
        self.assertNotIn("choices", json.dumps(e.to_json()))

    def test_model_event_is_frozen_instance(self) -> None:
        e = normalize_stream_event("openai", {"type": "start"})
        self.assertIsInstance(e, ModelEvent)
        self.assertTrue(dataclasses.is_dataclass(e))


class ErrorsTest(unittest.TestCase):
    def test_kind_auth(self) -> None:
        for payload in ({"status": 401}, {"httpStatus": 403}, {"type": "authentication_error"}):
            with self.subTest(payload=payload):
                e = normalize_error("openai", payload)
                self.assertEqual(e.kind, "auth")
                self.assertFalse(e.retryable)

    def test_kind_rate_limit(self) -> None:
        e = normalize_error("openai", {"status": 429, "message": "rate limited"})
        self.assertEqual(e.kind, "rate_limit")
        self.assertTrue(e.retryable)

    def test_kind_invalid_request(self) -> None:
        e = normalize_error("openai", {"status": 400, "type": "invalid_request_error"})
        self.assertEqual(e.kind, "invalid_request")
        self.assertFalse(e.retryable)

    def test_kind_context_length(self) -> None:
        for token in ("context_length", "max_tokens", "too_long"):
            with self.subTest(token=token):
                e = normalize_error("openai", {"type": token})
                self.assertEqual(e.kind, "context_length")
                self.assertFalse(e.retryable)

    def test_kind_timeout(self) -> None:
        for token in ("timeout", "timed_out"):
            with self.subTest(token=token):
                e = normalize_error("openai", {"type": token})
                self.assertEqual(e.kind, "timeout")
                self.assertTrue(e.retryable)

    def test_kind_network(self) -> None:
        for token in ("network", "connection", "ECONNRESET"):
            with self.subTest(token=token):
                e = normalize_error("openai", {"type": token})
                self.assertEqual(e.kind, "network")
                self.assertTrue(e.retryable)

    def test_kind_server(self) -> None:
        for payload in ({"status": 500}, {"status": 599}, {"type": "server_error"}, {"type": "overloaded"}):
            with self.subTest(payload=payload):
                e = normalize_error("openai", payload)
                self.assertEqual(e.kind, "server")
                self.assertTrue(e.retryable)

    def test_kind_unknown(self) -> None:
        e = normalize_error("openai", {"status": 418, "message": "teapot"})
        self.assertEqual(e.kind, "unknown")
        self.assertFalse(e.retryable)

    def test_priority_first_match_wins(self) -> None:
        # 冻结表「优先级从上到下」的实测：rate_limit 行(429) 在 server 行(5xx) 之前；
        # auth 行(401) 在 rate_limit 行之前。故混合输入由更靠前的行胜出。
        self.assertEqual(normalize_error("openai", {"status": 401, "type": "rate_limit"}).kind, "auth")
        self.assertEqual(normalize_error("openai", {"status": 429, "type": "server"}).kind, "rate_limit")
        self.assertEqual(normalize_error("openai", {"status": 500, "type": "timeout"}).kind, "timeout")

    def test_message_extraction_and_sanitizing(self) -> None:
        e = normalize_error("openai", {"message": "line1\nline2", "status": 429})
        self.assertEqual(e.message, "line1line2")
        self.assertNotIn("\n", e.message)

    def test_message_from_nested_error(self) -> None:
        e = normalize_error("openai", {"error": {"message": "nested msg"}})
        self.assertEqual(e.message, "nested msg")

    def test_message_truncated_to_120(self) -> None:
        e = normalize_error("openai", {"message": "M" * 500})
        self.assertEqual(len(e.message), 120)

    def test_message_absent_or_nonstring_is_empty(self) -> None:
        for payload in ({}, {"message": 123}, {"error": "bad"}, {"error": {"message": None}}):
            with self.subTest(payload=payload):
                self.assertEqual(normalize_error("openai", payload).message, "")

    def test_http_status_valid(self) -> None:
        e = normalize_error("openai", {"httpStatus": 429})
        self.assertEqual(e.http_status, 429)
        self.assertEqual(e.to_json()["httpStatus"], 429)

    def test_http_status_out_of_range_or_bad_type_is_none(self) -> None:
        for val in (99, 600, "429", 429.0, True, None):
            with self.subTest(val=val):
                self.assertIsNone(normalize_error("openai", {"httpStatus": val}).http_status)

    def test_provider_echoed(self) -> None:
        e = normalize_error("weird-provider", {})
        self.assertEqual(e.provider, "weird-provider")

    def test_malformed_inputs_never_raise(self) -> None:
        bad: list[Any] = [None, [], "str", 123, {"status": object()}, {"error": [1, 2]}, {"type": None}]
        for payload in bad:
            with self.subTest(payload=repr(payload)[:40]):
                e = normalize_error("openai", payload)
                self.assertIsInstance(e, ProviderError)
                self.assertIn(e.kind, ERROR_KINDS)

    def test_never_embeds_input_object(self) -> None:
        payload = {"status": 429, "message": "rate limited", "secret": "do-not-leak", "stack": ["a", "b"]}
        out = normalize_error("openai", payload).to_json()
        self.assertNotIn("secret", json.dumps(out))
        self.assertNotIn("stack", json.dumps(out))
        self.assertNotIn("do-not-leak", json.dumps(out))

    def test_provider_error_is_frozen(self) -> None:
        e = normalize_error("openai", {})
        with self.assertRaises(dataclasses.FrozenInstanceError):
            e.kind = "auth"  # type: ignore[misc]


class RoutingTest(unittest.TestCase):
    def test_native_only_capability(self) -> None:
        for cap in NATIVE_ONLY_CAPABILITIES:
            with self.subTest(cap=cap):
                r = route("openai", "unified", cap)
                self.assertEqual(r.mode, "native")
                self.assertEqual(r.reason, "native_only_capability")
                self.assertEqual(r.provider, "openai")
                self.assertEqual(r.fallback, "litellm")

    def test_explicit_native(self) -> None:
        r = route("openai", "native", None)
        self.assertEqual(r.mode, "native")
        self.assertEqual(r.reason, "explicit_native")
        self.assertEqual(r.fallback, "litellm")

    def test_unknown_provider(self) -> None:
        r = route("mystery", "unified", None)
        self.assertEqual(r.mode, "unified")
        self.assertEqual(r.reason, "unknown_provider")
        self.assertEqual(r.provider, "mystery")  # 原样回显
        self.assertEqual(r.fallback, "litellm")

    def test_litellm_first(self) -> None:
        for pid in PROVIDER_IDS:
            with self.subTest(pid=pid):
                r = route(pid, "unified", None)
                self.assertEqual(r.mode, "unified")
                self.assertEqual(r.reason, "litellm_first")
                self.assertEqual(r.fallback, "litellm")

    def test_priority_capability_beats_explicit_native(self) -> None:
        r = route("mystery", "native", "responses.streaming")
        self.assertEqual(r.reason, "native_only_capability")

    def test_fallback_always_litellm_string(self) -> None:
        results = [
            route("openai", "unified", "responses.streaming"),
            route("openai", "native", None),
            route("mystery", "unified", None),
            route("litellm", "unified", None),
        ]
        for r in results:
            self.assertEqual(r.fallback, "litellm")
            self.assertIsInstance(r.fallback, str)
            self.assertIsInstance(r.to_json(), dict)

    def test_to_json_keys(self) -> None:
        r = route("openai", "unified", None)
        self.assertEqual(list(r.to_json().keys()), ["mode", "provider", "reason", "fallback"])

    def test_route_decision_is_frozen(self) -> None:
        r = route("openai", "unified", None)
        with self.assertRaises(dataclasses.FrozenInstanceError):
            r.mode = "native"  # type: ignore[misc]


class LazyImportTest(unittest.TestCase):
    def test_probe_real_stdlib(self) -> None:
        self.assertTrue(probe("importlib"))

    def test_probe_missing_module(self) -> None:
        self.assertFalse(probe("definitely_not_real_xyz"))

    def test_probe_bad_inputs_never_raise(self) -> None:
        for bad in ("", None, ".relative", 123, [], "google.generativeai"):
            with self.subTest(bad=repr(bad)[:30]):
                result = probe(bad)  # type: ignore[arg-type]
                self.assertIsInstance(result, bool)

    def test_probe_never_imports_module(self) -> None:
        # find_spec 只查 spec，不执行模块；用一个必定触发 import 时报错的假模块名无法证明，
        # 改为断言 find_spec 被调用、且返回 spec 时也算 True（不执行）。
        with mock.patch("harness_hub_runtime.providers.lazy_import.importlib.util.find_spec", return_value=object()) as m:
            self.assertTrue(probe("whatever"))
            m.assert_called_once_with("whatever")

    def test_probe_false_when_find_spec_raises(self) -> None:
        with mock.patch(
            "harness_hub_runtime.providers.lazy_import.importlib.util.find_spec",
            side_effect=ValueError("boom"),
        ):
            self.assertFalse(probe("whatever"))

    def test_probe_false_when_no_spec(self) -> None:
        with mock.patch("harness_hub_runtime.providers.lazy_import.importlib.util.find_spec", return_value=None):
            self.assertFalse(probe("whatever"))

    def test_probe_all_shape(self) -> None:
        descriptors = probe_all()
        self.assertEqual(len(descriptors), 4)
        self.assertEqual([d.id for d in descriptors], ["litellm", "openai", "anthropic", "google"])
        self.assertEqual([d.mode for d in descriptors], ["unified", "native", "native", "native"])
        for d in descriptors:
            self.assertIsInstance(d, ProviderDescriptor)
            self.assertIsInstance(d.importable, bool)
            self.assertIsInstance(d.detail, str)
            self.assertTrue(dataclasses.is_dataclass(d))
            with self.assertRaises(dataclasses.FrozenInstanceError):
                d.id = "x"  # type: ignore[misc]

    def test_probe_all_no_third_party_sdk_env(self) -> None:
        # 冻结契约：干净环境（无可选 SDK）时，4 个描述符全部 importable=False，
        # detail 恒为 "optional SDK not installed"。用 mock 固定环境，避免依赖本机实际装了什么。
        with mock.patch(
            "harness_hub_runtime.providers.lazy_import.importlib.util.find_spec",
            return_value=None,
        ):
            descriptors = probe_all()
        self.assertEqual(len(descriptors), 4)
        for d in descriptors:
            self.assertFalse(d.importable)
            self.assertEqual(d.detail, "optional SDK not installed")

    def test_probe_all_available_sdk_env(self) -> None:
        # 另一档：SDK 存在时 importable=True / "optional SDK available"。
        with mock.patch(
            "harness_hub_runtime.providers.lazy_import.importlib.util.find_spec",
            return_value=object(),
        ):
            descriptors = probe_all()
        for d in descriptors:
            self.assertTrue(d.importable)
            self.assertEqual(d.detail, "optional SDK available")

    def test_probe_all_uses_exact_module_names(self) -> None:
        # 锁定 §4.1 冻结模块名与顺序。
        with mock.patch(
            "harness_hub_runtime.providers.lazy_import.importlib.util.find_spec",
            return_value=None,
        ) as m:
            probe_all()
        self.assertEqual(
            [c.args[0] for c in m.call_args_list],
            ["litellm", "openai", "anthropic", "google.generativeai"],
        )


class ReexportTest(unittest.TestCase):
    def test_all_names_reexported(self) -> None:
        import harness_hub_runtime.providers as pkg

        expected = {
            "PROVIDER_IDS",
            "MODES",
            "MODEL_EVENT_TYPES",
            "ERROR_KINDS",
            "NATIVE_ONLY_CAPABILITIES",
            "MAX_TEXT_DELTA",
            "MAX_SHORT",
            "ProviderDescriptor",
            "ModelEvent",
            "ProviderError",
            "RouteDecision",
            "normalize_stream_event",
            "normalize_error",
            "route",
            "probe",
            "probe_all",
        }
        self.assertTrue(expected.issubset(set(pkg.__all__)))
        for name in expected:
            self.assertTrue(hasattr(pkg, name), name)


if __name__ == "__main__":
    unittest.main()
