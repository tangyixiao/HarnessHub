"""F21 S1 — 路由决策（spec §3.5，冻结）。

ADR-0007：LiteLLM first。`route()` 只做纯决策，不调用任何 provider。
零依赖：只用 dataclasses / typing / __future__。
"""

from __future__ import annotations

from .dto import NATIVE_ONLY_CAPABILITIES, PROVIDER_IDS, RouteDecision

FALLBACK = "litellm"


def route(provider: str, requested_mode: str, capability: str | None = None) -> RouteDecision:
    # provider 原样回显（未知 provider 也照抄）。
    if capability in NATIVE_ONLY_CAPABILITIES:
        return RouteDecision("native", provider, "native_only_capability", FALLBACK)
    if requested_mode == "native":
        return RouteDecision("native", provider, "explicit_native", FALLBACK)
    if provider not in PROVIDER_IDS:
        return RouteDecision("unified", provider, "unknown_provider", FALLBACK)
    return RouteDecision("unified", provider, "litellm_first", FALLBACK)
