"""F21 S1 — 可选 SDK 探测（spec §4.1，冻结）。

只用 `importlib.util.find_spec()` 检查第三方 SDK 是否可导入，**绝不**在模块顶层
import 它们，也绝不执行它们。任何异常都收敛成 `False`。
零依赖：只用 importlib.util / dataclasses / __future__。
"""

from __future__ import annotations

import importlib.util
from typing import Any

from .dto import ProviderDescriptor

# 冻结：模块名 → (descriptor id, mode)；顺序即 probe_all() 返回顺序。
_SDK_MODULES = (
    ("litellm", "litellm", "unified"),
    ("openai", "openai", "native"),
    ("anthropic", "anthropic", "native"),
    ("google.generativeai", "google", "native"),
)

_DETAIL_MISSING = "optional SDK not installed"
_DETAIL_PRESENT = "optional SDK available"


def probe(module: str) -> bool:
    """`find_spec` 成功即 True；任何异常（含非 str / 点号 / 缺失）一律 False。"""
    try:
        return importlib.util.find_spec(module) is not None
    except BaseException:
        return False


def probe_all() -> list[ProviderDescriptor]:
    descriptors: list[ProviderDescriptor] = []
    for module, provider_id, mode in _SDK_MODULES:
        importable = probe(module)
        detail = _DETAIL_PRESENT if importable else _DETAIL_MISSING
        descriptors.append(ProviderDescriptor(provider_id, mode, importable, detail))
    return descriptors
