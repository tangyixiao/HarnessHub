"""F21 S1 — provider error 归一化（spec §4.3，冻结）。

`normalize_error()` 把各 provider 的错误对象收敛成脱敏的 `ProviderError`。
优先级从上到下 first-match-wins；`message` 只取首 120 字符并移除换行；
`httpStatus` 只接受 100..599 的 int；对畸形输入永不抛异常。
零依赖：只用 dataclasses / typing / __future__。
"""

from __future__ import annotations

from typing import Any

from .dto import ProviderError


def _is_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _status(error: dict[str, Any]) -> int | None:
    for key in ("status", "httpStatus"):
        value = error.get(key)
        if _is_int(value):
            return value
    return None


def _message(error: dict[str, Any]) -> str:
    raw: Any = error.get("message")
    if not isinstance(raw, str):
        nested = error.get("error")
        if isinstance(nested, dict):
            raw = nested.get("message")
    if not isinstance(raw, str):
        return ""
    return raw.replace("\n", "").replace("\r", "")[:120]


def _type_text(error: dict[str, Any]) -> str:
    value = error.get("type")
    return value if isinstance(value, str) else ""


def _kind(error: dict[str, Any], status: int | None, type_text: str) -> tuple[str, bool]:
    """返回 (kind, retryable)，严格按 §4.3 表格自上而下 first-match-wins。"""
    if status in (401, 403) or "authentication" in type_text:
        return "auth", False
    if status == 429 or "rate_limit" in type_text:
        return "rate_limit", True
    if status == 400 or "invalid_request" in type_text:
        return "invalid_request", False
    if any(token in type_text for token in ("context_length", "max_tokens", "too_long")):
        return "context_length", False
    if "timeout" in type_text or "timed_out" in type_text:
        return "timeout", True
    if any(token in type_text for token in ("network", "connection", "ECONN")):
        return "network", True
    if (status is not None and 500 <= status <= 599) or "server" in type_text or "overloaded" in type_text:
        return "server", True
    return "unknown", False


def normalize_error(provider: str, error: dict) -> ProviderError:
    safe_provider = provider if isinstance(provider, str) else ""
    try:
        if not isinstance(error, dict):
            return ProviderError("unknown", False, "", safe_provider, None)
        status = _status(error)
        type_text = _type_text(error)
        kind, retryable = _kind(error, status, type_text)
        http_status = status if (status is not None and 100 <= status <= 599) else None
        return ProviderError(kind, retryable, _message(error), safe_provider, http_status)
    except BaseException:
        return ProviderError("unknown", False, "", safe_provider, None)
