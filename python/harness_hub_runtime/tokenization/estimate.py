"""Token 估算启发式（spec §5.2，ADR-0003：估计值必须标记 estimated）。

local-first：**只接收文本长度（int），永不接收 prompt / response 原文**。
零第三方依赖，不联网、不接触 SQLite。
"""

from __future__ import annotations

import math
from dataclasses import dataclass

from .revision import bind_revision


@dataclass(frozen=True)
class TokenEstimate:
    """冻结的估算结果；``to_json()`` 键序为跨 IPC 契约（spec §5.2）。"""

    input_tokens: int
    output_tokens: int
    total_tokens: int
    truth_level: str
    tokenizer_model: str
    revision: str
    estimated: bool

    def to_json(self) -> dict:
        return {
            "inputTokens": self.input_tokens,
            "outputTokens": self.output_tokens,
            "totalTokens": self.total_tokens,
            "truthLevel": self.truth_level,
            "tokenizerModel": self.tokenizer_model,
            "revision": self.revision,
            "estimated": self.estimated,
        }


def estimate(text_length: int, model: str) -> TokenEstimate:
    """按 ``max(1, ceil(text_length / 4))`` 粗估 token 数。

    ``text_length`` 必须是 ``int``（**排除 bool**）且 ``>= 0``，否则抛 ``ValueError``。
    ``model`` coerce 到 ``str``。返回值恒 ``estimated=True``（ADR-0003：绝不把估计伪装成精确值）。
    """
    if isinstance(text_length, bool) or not isinstance(text_length, int):
        raise ValueError("text_length must be a non-negative int (not bool)")
    if text_length < 0:
        raise ValueError("text_length must be a non-negative int")

    n = max(1, math.ceil(text_length / 4))
    binding = bind_revision(model)
    return TokenEstimate(
        input_tokens=n,
        output_tokens=0,
        total_tokens=n,
        truth_level="estimated",
        tokenizer_model=binding.tokenizer_model,
        revision=binding.revision,
        estimated=True,
    )
