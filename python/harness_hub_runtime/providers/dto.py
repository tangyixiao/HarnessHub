"""F21 S1 — providers 契约层的常量与冻结 DTO（spec §3.6 / §4.1）。

全部为 `@dataclass(frozen=True)`；`to_json()` 锁定跨边界的 camelCase 键名与顺序。
零依赖：只用 dataclasses / typing / __future__。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

# 冻结常量（顺序即契约，spec §3.6）。
PROVIDER_IDS = ("litellm", "openai", "anthropic", "google")
MODES = ("unified", "native")
MODEL_EVENT_TYPES = ("start", "delta", "usage", "done", "error", "unknown")
ERROR_KINDS = (
    "auth",
    "rate_limit",
    "invalid_request",
    "context_length",
    "server",
    "timeout",
    "network",
    "unknown",
)
NATIVE_ONLY_CAPABILITIES = (
    "responses.streaming",
    "anthropic.thinking",
    "google.grounding",
    "provider.raw_request",
)


@dataclass(frozen=True)
class ProviderDescriptor:
    id: str
    mode: str
    importable: bool
    detail: str

    def to_json(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "mode": self.mode,
            "importable": self.importable,
            "detail": self.detail,
        }


@dataclass(frozen=True)
class ModelEvent:
    type: str
    text_delta: str | None
    finish_reason: str | None
    usage: dict[str, int] | None
    raw_provider: str

    def to_json(self) -> dict[str, Any]:
        return {
            "type": self.type,
            "textDelta": self.text_delta,
            "finishReason": self.finish_reason,
            "usage": self.usage,
            "rawProvider": self.raw_provider,
        }


@dataclass(frozen=True)
class ProviderError:
    kind: str
    retryable: bool
    message: str
    provider: str
    http_status: int | None

    def to_json(self) -> dict[str, Any]:
        return {
            "kind": self.kind,
            "retryable": self.retryable,
            "message": self.message,
            "provider": self.provider,
            "httpStatus": self.http_status,
        }


@dataclass(frozen=True)
class RouteDecision:
    mode: str
    provider: str
    reason: str
    fallback: str

    def to_json(self) -> dict[str, Any]:
        return {
            "mode": self.mode,
            "provider": self.provider,
            "reason": self.reason,
            "fallback": self.fallback,
        }
