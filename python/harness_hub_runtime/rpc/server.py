"""stdio JSON-RPC 2.0 服务端。

为什么是 stdio 而不是 localhost HTTP（docs/adr/0001、docs/CONTEXT.md）：
桌面应用没有理由常驻开放一个本机端口；stdio 让 sidecar 的生命周期完全跟随父进程，
也天然避免了端口冲突与未授权访问。

报文约定：**一行一个 JSON 对象**（line-delimited）。不用 Content-Length 头，
因为通信双方都由本项目控制，行协议更容易调试与测试。

错误码沿用 JSON-RPC 2.0 标准：
    -32700 解析错误 / -32600 请求非法 / -32601 方法不存在 / -32603 内部错误
"""

from __future__ import annotations

import importlib.util  # 仅用 find_spec 探测 SDK 是否可发现，绝不 import
import json
import math
import sys
from typing import Any, Callable, Iterable, TextIO

from harness_hub_runtime import __version__

Json = dict[str, Any]
Handler = Callable[[Json], Any]
MethodTable = dict[str, Handler]

PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INTERNAL_ERROR = -32603

# F20 握手协议（docs/specs/2026-11-04-f20-sidecar-lifecycle-design.md §3.1）。
# 协商只比较主版本（`.` 前第一段）；能力清单是 Rust 侧 requiredCapabilities 的超集校验依据。
PROTOCOL_VERSION = "1.0"

# F21 冻结清单（docs/specs/2026-11-05-f21-provider-gateway-design.md §3.2），顺序有意义。
# Rust 握手仍只要求 F20 三项；本清单是其超集，故 F20 握手天然继续通过。
SUPPORTED_CAPABILITIES = [
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

# gateway.status/providers 直接读这两份静态常量，自包含、不依赖 S1/S2（spec §10）。
GATEWAY_STATIC_CAPABILITIES = SUPPORTED_CAPABILITIES
GATEWAY_STATIC_METHODS = [
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

PROVIDER_IDS = ("litellm", "openai", "anthropic", "google")
NATIVE_ONLY_CAPABILITIES = (
    "responses.streaming",
    "anthropic.thinking",
    "google.grounding",
    "provider.raw_request",
)
RAW_CONTENT_KEYS = ("prompt", "messages", "response", "content")
TRUTH_PRECEDENCE = ["provider_reported", "provider_count_api", "local_exact", "estimated"]
LOCAL_LEVELS = ["local_exact", "estimated"]
# (provider descriptor id, mode, SDK 模块名)；google 的 SDK 名是 google.generativeai。
SDK_PROBES = (
    ("litellm", "unified", "litellm"),
    ("openai", "native", "openai"),
    ("anthropic", "native", "anthropic"),
    ("google", "native", "google.generativeai"),
)
# (模型名前缀, tokenizerModel, revision)，冻结表 spec §5.3。
TOKENIZER_REVISIONS = (
    ("gpt-4o", "o200k_base", "2024-05-13"),
    ("gpt-4", "cl100k_base", "2023-06-13"),
    ("gpt-3.5", "cl100k_base", "2023-06-13"),
    ("claude-", "anthropic-claude", "2024-06-20"),
    ("gemini-", "sentencepiece-gemini", "2024-09-24"),
)


def _major(version: str) -> str:
    """取版本号的主版本段，`"1.9"` -> `"1"`。"""
    return version.split(".", 1)[0]


class RpcError(Exception):
    """处理器主动抛出的、需要映射成 JSON-RPC 错误对象的异常。"""

    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


def _error_response(request_id: Any, code: int, message: str) -> Json:
    return {"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}}


def handle_request(request: Any, methods: MethodTable) -> Json | None:
    """处理单条请求。

    返回 `None` 表示这是一条通知（notification，没有 `id`），按 JSON-RPC 规范不应回包。
    """
    if not isinstance(request, dict) or request.get("jsonrpc") != "2.0":
        request_id = request.get("id") if isinstance(request, dict) else None
        return _error_response(request_id, INVALID_REQUEST, "不是合法的 JSON-RPC 2.0 请求")

    request_id = request.get("id")
    method = request.get("method")
    handler = methods.get(method) if isinstance(method, str) else None

    if handler is None:
        response = _error_response(request_id, METHOD_NOT_FOUND, f"未知方法：{method}")
    else:
        params = request.get("params") or {}
        try:
            response = {"jsonrpc": "2.0", "id": request_id, "result": handler(params)}
        except RpcError as error:
            response = _error_response(request_id, error.code, error.message)
        except Exception as error:  # noqa: BLE001 - 兜底：sidecar 不能因为单个方法崩溃
            response = _error_response(request_id, INTERNAL_ERROR, str(error))

    # 通知不回包，但处理器仍会被执行。
    return None if request_id is None else response


def build_runtime_info() -> Json:
    """`runtime.info` 与 `initialize.result.runtime` 共用的唯一构造函数（F20 §3.2）。

    禁止在别处手写同一份形状：Rust 侧会逐字段比对两处结果。
    """
    return {
        "name": "harness-hub-runtime",
        "version": __version__,
        "transport": "stdio-jsonrpc",
        "python": sys.version.split()[0],
        "protocolVersion": PROTOCOL_VERSION,
        # Phase 8 起这里会列出 LiteLLM / 官方 SDK / MCP 等已就绪的 Provider。
        "providers": [],
        "capabilities": list(SUPPORTED_CAPABILITIES),
        "methods": list(DEFAULT_METHODS),
    }


def _require_params(params: Any) -> Json:
    if not isinstance(params, dict):
        raise RpcError(INVALID_REQUEST, "params 必须是对象")
    return params


def _reject_raw_content(params: Json, message: str) -> None:
    """安全闸：任何方法都不得接收 prompt/response/session 原文（local-first）。"""
    for key in RAW_CONTENT_KEYS:
        if key in params:
            raise RpcError(INVALID_REQUEST, message)


def _lazy(modname: str, attrname: str) -> Any | None:
    """尝试从 S1/S2 兄弟包拿一个实现；缺模块/缺属性/任何异常都返回 None。

    本 worktree 尚未合入 S1(providers)/S2(tokenization)，因此这里通常返回 None，
    走各自的自包含降级路径；Lead 合并后同一份 server.py 会自动升级到真实实现（spec §10）。
    """
    try:
        import importlib

        module = importlib.import_module(modname)
        return getattr(module, attrname)
    except BaseException:  # noqa: BLE001 - ImportError/AttributeError/任何异常都优雅降级
        return None


def _lazy_any(candidates: tuple[tuple[str, str], ...]) -> Any | None:
    """依次尝试若干 (模块, 属性) 候选，返回第一个可用的实现。"""
    for modname, attrname in candidates:
        found = _lazy(modname, attrname)
        if found is not None:
            return found
    return None


def _sdk_importable(module: str) -> bool:
    """用 find_spec 探测可选 SDK 是否可发现，绝不 import；任何异常都视为不可用。

    google.generativeai 在父包 google 缺失时会抛 ModuleNotFoundError，必须兜住。
    """
    try:
        return importlib.util.find_spec(module) is not None
    except BaseException:  # noqa: BLE001 - 父包缺失/探测失败都算未安装
        return False


def _provider_descriptors() -> list[Json]:
    """四个 provider 描述符；S1 的 probe_all 可用则优先，否则自包含 find_spec 探测。"""
    probe_all = _lazy_any(
        (
            ("harness_hub_runtime.providers.probe", "probe_all"),
            ("harness_hub_runtime.providers", "probe_all"),
        )
    )
    if probe_all is not None:
        try:
            raw = probe_all()
            return [
                {
                    "id": item.get("id"),
                    "mode": item.get("mode"),
                    "importable": bool(item.get("importable")),
                    "detail": item.get("detail"),
                }
                for item in raw
            ]
        except BaseException:  # noqa: BLE001 - 真实实现异常也退回探测
            pass

    descriptors: list[Json] = []
    for pid, mode, module in SDK_PROBES:
        importable = _sdk_importable(module)
        descriptors.append(
            {
                "id": pid,
                "mode": mode,
                "importable": importable,
                "detail": "optional SDK available" if importable else "optional SDK not installed",
            }
        )
    return descriptors


def _tokenization_block() -> Json:
    """gateway.status 的 tokenization 块；S2 truth 模块可用则优先。"""
    precedence = _lazy("harness_hub_runtime.tokenization.truth", "TRUTH_PRECEDENCE")
    levels = _lazy("harness_hub_runtime.tokenization.truth", "LOCAL_LEVELS")
    return {
        "precedence": list(precedence) if precedence is not None else list(TRUTH_PRECEDENCE),
        "levels": list(levels) if levels is not None else list(LOCAL_LEVELS),
        "revisionBinding": True,
    }


def runtime_info(_params: Json) -> Json:
    """返回 sidecar 自身信息，用于握手与排障。"""
    return build_runtime_info()


def ping(_params: Json) -> Json:
    return {"pong": True}


def initialize(params: Json) -> Json:
    """F20 握手（§3.1）：校验主版本与 requiredCapabilities，返回协商结果。"""
    client_version = params.get("protocolVersion")
    if isinstance(client_version, str) and _major(client_version) != _major(PROTOCOL_VERSION):
        raise RpcError(
            INVALID_REQUEST,
            f"protocolVersion 主版本不兼容：{client_version}（sidecar 为 {PROTOCOL_VERSION}）",
        )

    required = params.get("requiredCapabilities") or []
    missing = [cap for cap in required if cap not in SUPPORTED_CAPABILITIES]
    if missing:
        raise RpcError(INVALID_REQUEST, f"缺少的能力：{', '.join(missing)}")

    return {
        "protocolVersion": PROTOCOL_VERSION,
        "runtime": build_runtime_info(),
        "capabilities": list(SUPPORTED_CAPABILITIES),
        "methods": list(DEFAULT_METHODS),
    }


def shutdown(_params: Json) -> Json:
    """停止信号；`serve()` 会据此退出读取循环（§3.4）。"""
    return {"stopping": True}


def gateway_status(params: Json) -> Json:
    """§3.3 自包含状态快照；绝不读取/回显任何原文内容。"""
    params = _require_params(params)
    _reject_raw_content(params, "gateway.status 不接受原文内容")
    return {
        "state": "ready",  # ADR-0005：capability != readiness，可选 SDK 缺失不影响 ready
        "mode": "unified",
        "protocolVersion": PROTOCOL_VERSION,
        "providers": _provider_descriptors(),
        "tokenization": _tokenization_block(),
        "capabilities": list(GATEWAY_STATIC_CAPABILITIES),
        "methods": list(GATEWAY_STATIC_METHODS),
        "lastError": None,
    }


def gateway_providers(params: Json) -> Json:
    """§3.4 与 gateway.status 完全相同的 provider 描述符数组。"""
    params = _require_params(params)
    _reject_raw_content(params, "gateway.providers 不接受原文内容")
    return _provider_descriptors()


def _route_fallback(
    provider: str, capability: Any, requested_mode: Any
) -> tuple[str, str, str]:
    """自包含路由规则（spec §3.5），返回 (mode, reason, fallback)。"""
    if capability in NATIVE_ONLY_CAPABILITIES:
        return "native", "native_only_capability", "litellm"
    if requested_mode == "native":
        return "native", "explicit_native", "litellm"
    if provider not in PROVIDER_IDS:
        return "unified", "unknown_provider", "litellm"
    return "unified", "litellm_first", "litellm"


def gateway_route(params: Json) -> Json:
    """§3.5 路由决策；S1 providers.routing.route 可用则优先，否则自包含实现。"""
    params = _require_params(params)
    _reject_raw_content(params, "route 不接受原文内容")

    provider = params.get("provider", "")
    if not isinstance(provider, str):
        provider = ""
    capability = params.get("capability")
    requested_mode = params.get("requestedMode")

    route = _lazy_any(
        (
            ("harness_hub_runtime.providers.routing.route", "route"),
            ("harness_hub_runtime.providers.routing", "route"),
            ("harness_hub_runtime.providers", "route"),
        )
    )
    if route is not None:
        try:
            decision = route(provider, capability, requested_mode)
            result = decision.to_json() if hasattr(decision, "to_json") else decision
            return {
                "mode": result.get("mode", "unified"),
                "provider": result.get("provider", provider),
                "reason": result.get("reason", "litellm_first"),
                "fallback": result.get("fallback", "litellm"),
            }
        except BaseException:  # noqa: BLE001 - 真实实现异常也退回自包含规则
            pass

    mode, reason, fallback = _route_fallback(provider, capability, requested_mode)
    return {"mode": mode, "provider": provider, "reason": reason, "fallback": fallback}


def gateway_normalize_stream(params: Json) -> Json:
    """§3.6 流式事件归一化；S1 可用则优先，否则 unknown 降级形状（不回显原始 event）。"""
    params = _require_params(params)
    provider = params.get("provider", "")
    if not isinstance(provider, str):
        provider = ""
    event = params.get("event", {})

    normalize = _lazy_any(
        (
            ("harness_hub_runtime.providers.streaming.normalize_stream_event", "normalize_stream_event"),
            ("harness_hub_runtime.providers.streaming", "normalize_stream_event"),
            ("harness_hub_runtime.providers", "normalize_stream_event"),
        )
    )
    if normalize is not None:
        try:
            result = normalize(event, provider).to_json()
            return {
                "type": result.get("type", "unknown"),
                "textDelta": result.get("textDelta"),
                "finishReason": result.get("finishReason"),
                "usage": result.get("usage"),
                "rawProvider": result.get("rawProvider", provider),
            }
        except BaseException:  # noqa: BLE001 - 真实实现异常也退回降级
            pass

    return {
        "type": "unknown",
        "textDelta": None,
        "finishReason": None,
        "usage": None,
        "rawProvider": provider,
    }


def gateway_normalize_error(params: Json) -> Json:
    """§3.6 错误归一化；S1 可用则优先，否则 unknown 降级形状（不回显原始 error）。"""
    params = _require_params(params)
    provider = params.get("provider", "")
    if not isinstance(provider, str):
        provider = ""
    error = params.get("error", {})

    normalize = _lazy_any(
        (
            ("harness_hub_runtime.providers.errors.normalize_error", "normalize_error"),
            ("harness_hub_runtime.providers.errors", "normalize_error"),
            ("harness_hub_runtime.providers", "normalize_error"),
        )
    )
    if normalize is not None:
        try:
            result = normalize(error, provider).to_json()
            return {
                "kind": result.get("kind", "unknown"),
                "retryable": bool(result.get("retryable", False)),
                "message": result.get("message", ""),
                "provider": result.get("provider", provider),
                "httpStatus": result.get("httpStatus"),
            }
        except BaseException:  # noqa: BLE001 - 真实实现异常也退回降级
            pass

    return {
        "kind": "unknown",
        "retryable": False,
        "message": "",
        "provider": provider,
        "httpStatus": None,
    }


def tokenization_precedence(params: Json) -> Json:
    """§3.7 真值优先级；S2 truth 模块可用则优先其常量。"""
    _require_params(params)
    return _tokenization_block()


def _revision_for(model: str) -> tuple[str, str]:
    """spec §5.3 冻结表：返回 (tokenizerModel, revision)，未知 -> (unknown, unknown)。"""
    if isinstance(model, str):
        for prefix, tokenizer_model, revision in TOKENIZER_REVISIONS:
            if model.startswith(prefix):
                return tokenizer_model, revision
    return "unknown", "unknown"


def _usage_tokens(usage: Any) -> tuple[int, int] | None:
    """仅接受含 int inputTokens/outputTokens 的 dict（bool 不算 int）。"""
    if not isinstance(usage, dict):
        return None
    input_tokens = usage.get("inputTokens")
    output_tokens = usage.get("outputTokens")
    if (
        isinstance(input_tokens, int)
        and not isinstance(input_tokens, bool)
        and isinstance(output_tokens, int)
        and not isinstance(output_tokens, bool)
    ):
        return input_tokens, output_tokens
    return None


def tokenization_estimate(params: Json) -> Json:
    """§5.2 token 估算；绝不接收原文，只吃 textLength 等元数据。"""
    params = _require_params(params)
    _reject_raw_content(params, "tokenization.estimate 不接受原文内容")
    # text/prompt/content 也单列拒绝（安全闸更显式）。
    for key in ("text", "prompt", "content"):
        if key in params:
            raise RpcError(INVALID_REQUEST, "tokenization.estimate 不接受原文内容")

    text_length = params.get("textLength")
    if not isinstance(text_length, int) or isinstance(text_length, bool) or text_length < 0:
        raise RpcError(INVALID_REQUEST, "textLength 必须是非负整数")

    model = params.get("model", "")
    if not isinstance(model, str):
        model = ""
    tokenizer_model, revision = _revision_for(model)

    reported = _usage_tokens(params.get("reportedUsage"))
    counted = _usage_tokens(params.get("countApiUsage"))

    # 优先级：provider_reported > provider_count_api > estimated。
    # F21 零依赖，local_exact 恒不可用（spec §5.2），留作 S2 合并后的占位。
    if reported is not None:
        input_tokens, output_tokens = reported
        truth_level, estimated = "provider_reported", False
    elif counted is not None:
        input_tokens, output_tokens = counted
        truth_level, estimated = "provider_count_api", False
    else:
        estimate_fn = _lazy_any(
            (
                ("harness_hub_runtime.tokenization.estimate", "estimate"),
                ("harness_hub_runtime.tokenization", "estimate"),
            )
        )
        if estimate_fn is not None:
            try:
                result = estimate_fn(text_length, model)
                data = result.to_json() if hasattr(result, "to_json") else result
                input_tokens = int(data.get("inputTokens", max(1, math.ceil(text_length / 4))))
                output_tokens = int(data.get("outputTokens", 0))
            except BaseException:  # noqa: BLE001 - 真实实现异常也退回自包含估算
                input_tokens, output_tokens = max(1, math.ceil(text_length / 4)), 0
        else:
            input_tokens, output_tokens = max(1, math.ceil(text_length / 4)), 0
        truth_level, estimated = "estimated", True

    return {
        "inputTokens": input_tokens,
        "outputTokens": output_tokens,
        "totalTokens": input_tokens + output_tokens,
        "truthLevel": truth_level,
        "tokenizerModel": tokenizer_model,
        "revision": revision,
        "estimated": estimated,
    }


def tokenization_revision(params: Json) -> Json:
    """§5.3 revision 绑定；S2 bind_revision 可用则优先，否则冻结表逻辑。"""
    params = _require_params(params)
    model = params.get("model", "")
    if not isinstance(model, str):
        model = ""
    tokenizer_model, revision = _revision_for(model)
    explicit = params.get("tokenizerModel")

    bind = _lazy_any(
        (
            ("harness_hub_runtime.tokenization.revision", "bind_revision"),
            ("harness_hub_runtime.tokenization", "bind_revision"),
        )
    )
    if bind is not None:
        try:
            result = bind(model, explicit)
            data = result.to_json() if hasattr(result, "to_json") else result
            return {
                "model": data.get("model", model),
                "tokenizerModel": data.get("tokenizerModel", tokenizer_model),
                "revision": data.get("revision", revision),
                "bound": bool(data.get("bound", False)),
                "reason": data.get("reason", "unknown_model"),
            }
        except BaseException:  # noqa: BLE001 - 真实实现异常也退回冻结表
            pass

    if revision != "unknown":
        return {
            "model": model,
            "tokenizerModel": tokenizer_model,
            "revision": revision,
            "bound": True,
            "reason": "known_model",
        }
    if isinstance(explicit, str) and explicit:
        return {
            "model": model,
            "tokenizerModel": explicit,
            "revision": "unknown",
            "bound": False,
            "reason": "explicit_unverified",
        }
    return {
        "model": model,
        "tokenizerModel": "unknown",
        "revision": "unknown",
        "bound": False,
        "reason": "unknown_model",
    }


DEFAULT_METHODS: MethodTable = {
    "ping": ping,
    "runtime.info": runtime_info,
    "initialize": initialize,
    "shutdown": shutdown,
    "gateway.status": gateway_status,
    "gateway.providers": gateway_providers,
    "gateway.route": gateway_route,
    "gateway.normalize.stream": gateway_normalize_stream,
    "gateway.normalize.error": gateway_normalize_error,
    "tokenization.estimate": tokenization_estimate,
    "tokenization.precedence": tokenization_precedence,
    "tokenization.revision": tokenization_revision,
}


def serve(
    stdin: Iterable[str],
    stdout: TextIO,
    methods: MethodTable | None = None,
) -> None:
    """从 `stdin` 逐行读取请求，把响应写到 `stdout`。"""
    table = DEFAULT_METHODS if methods is None else methods

    for line in stdin:
        line = line.strip()
        if not line:
            continue

        try:
            request = json.loads(line)
        except json.JSONDecodeError as error:
            response: Json | None = _error_response(None, PARSE_ERROR, f"JSON 解析失败：{error}")
            stopping = False
        else:
            response = handle_request(request, table)
            # shutdown（请求或通知）都当作停止信号：先回包（若有），再退出读取循环（§3.4）。
            stopping = isinstance(request, dict) and request.get("method") == "shutdown"

        if response is not None:
            stdout.write(json.dumps(response, ensure_ascii=False) + "\n")
            stdout.flush()

        if stopping:
            break


def main() -> int:
    serve(sys.stdin, sys.stdout)
    return 0
