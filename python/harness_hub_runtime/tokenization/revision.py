"""Tokenizer-model revision 绑定（spec §5.3）。

同一段文本在不同 tokenizer revision 下 token 数不同，所以任何 token 数字都必须能追到
``(tokenizerModel, revision)``；绑定不上就明确标 ``unknown``，不允许用「看起来对」的默认值蒙混。
零第三方依赖，不联网、不接触 SQLite，不接收任何 prompt / response 原文。
"""

from __future__ import annotations

from dataclasses import dataclass

# model 前缀 -> (tokenizerModel, revision)。顺序有意义：更具体的 "gpt-4o"
# 必须排在 "gpt-4" 之前，否则 gpt-4o-mini 会错配到 cl100k_base。
TOKENIZER_REVISIONS: dict[str, tuple[str, str]] = {
    "gpt-4o": ("o200k_base", "2024-05-13"),
    "gpt-4": ("cl100k_base", "2023-06-13"),
    "gpt-3.5": ("cl100k_base", "2023-06-13"),
    "claude-": ("anthropic-claude", "2024-06-20"),
    "gemini-": ("sentencepiece-gemini", "2024-09-24"),
}


@dataclass(frozen=True)
class RevisionBinding:
    """冻结的 revision 绑定结果；``to_json()`` 键序为跨 IPC 契约。"""

    model: str
    tokenizer_model: str
    revision: str
    bound: bool
    reason: str

    def to_json(self) -> dict:
        return {
            "model": self.model,
            "tokenizerModel": self.tokenizer_model,
            "revision": self.revision,
            "bound": self.bound,
            "reason": self.reason,
        }


def bind_revision(model: str, tokenizer_model: str | None = None) -> RevisionBinding:
    """按 TOKENIZER_REVISIONS 前缀绑定 ``(tokenizerModel, revision)``。

    - 命中已知前缀 -> bound=True, reason="known_model"，表值优先（即使显式给了不同的
      tokenizer_model，known binding 仍然胜出）。
    - 未命中且显式给了 tokenizer_model -> bound=False, reason="explicit_unverified",
      revision="unknown"（不假装绑定成功）。
    - 未命中且未给 -> bound=False, reason="unknown_model", revision="unknown"。

    ``model`` 一律 coerce 到 ``str``，None / 异常输入也不抛异常。
    """
    model_str = str(model)
    for prefix, (table_tokenizer, table_revision) in TOKENIZER_REVISIONS.items():
        if model_str.startswith(prefix):
            return RevisionBinding(
                model=model_str,
                tokenizer_model=table_tokenizer,
                revision=table_revision,
                bound=True,
                reason="known_model",
            )
    if tokenizer_model is not None:
        return RevisionBinding(
            model=model_str,
            tokenizer_model=str(tokenizer_model),
            revision="unknown",
            bound=False,
            reason="explicit_unverified",
        )
    return RevisionBinding(
        model=model_str,
        tokenizer_model="unknown",
        revision="unknown",
        bound=False,
        reason="unknown_model",
    )
