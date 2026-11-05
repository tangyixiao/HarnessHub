"""Provider 平面（F21 S1：契约层）。

当前提供：冻结常量、DTO、streaming/error 归一化、路由、可选 SDK 探测。
全部零依赖，顶层不 import 任何第三方 SDK（ADR-0007：LiteLLM first）。

后续计划（不在 S1 范围）：

* `gateway.py` —— LiteLLM 统一入口。
* `litellm_adapter.py` / `openai_adapter.py` / `anthropic_adapter.py` / `google_adapter.py`
  —— 需要原生能力时才走的 escape hatch。
"""

from __future__ import annotations

from .dto import (
    ERROR_KINDS,
    MODES,
    MODEL_EVENT_TYPES,
    NATIVE_ONLY_CAPABILITIES,
    PROVIDER_IDS,
    ModelEvent,
    ProviderDescriptor,
    ProviderError,
    RouteDecision,
)
from .errors import normalize_error
from .lazy_import import probe, probe_all
from .routing import route
from .streaming import MAX_SHORT, MAX_TEXT_DELTA, normalize_stream_event

__all__: list[str] = [
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
]
