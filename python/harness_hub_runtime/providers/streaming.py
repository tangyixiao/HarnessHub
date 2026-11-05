"""F21 S1 — streaming event 归一化（spec §4.2，冻结）。

`normalize_stream_event()` 把各 provider 的原始事件收敛成安全、定长的 `ModelEvent`。
设计要点：first-match-wins、绝不回显原始输入对象、对畸形输入永不抛异常。
零依赖：只用 dataclasses / typing / __future__。
"""

from __future__ import annotations

from typing import Any

from .dto import ModelEvent

MAX_TEXT_DELTA = 256
MAX_SHORT = 64

_INPUT_KEYS = ("prompt_tokens", "input_tokens", "promptTokenCount")
_OUTPUT_KEYS = ("completion_tokens", "output_tokens", "candidatesTokenCount")


def _is_int(value: Any) -> bool:
    # bool 是 int 的子类，但不属于「整数 token 数」契约。
    return isinstance(value, int) and not isinstance(value, bool)


def _mk(event_type: str, raw_provider: str) -> ModelEvent:
    return ModelEvent(
        type=event_type,
        text_delta=None,
        finish_reason=None,
        usage=None,
        raw_provider=raw_provider,
    )


def _delta_text(event: dict[str, Any]) -> str | None:
    """按 openai → anthropic → google 顺序取首个可用文本；非字符串一律忽略。"""
    choices = event.get("choices")
    if isinstance(choices, list) and choices and isinstance(choices[0], dict):
        delta = choices[0].get("delta")
        if isinstance(delta, dict):
            content = delta.get("content")
            if isinstance(content, str):
                return content

    if event.get("type") == "content_block_delta":
        delta = event.get("delta")
        if isinstance(delta, dict):
            text = delta.get("text")
            if isinstance(text, str):
                return text

    candidates = event.get("candidates")
    if isinstance(candidates, list) and candidates and isinstance(candidates[0], dict):
        content = candidates[0].get("content")
        if isinstance(content, dict):
            parts = content.get("parts")
            if isinstance(parts, list) and parts and isinstance(parts[0], dict):
                text = parts[0].get("text")
                if isinstance(text, str):
                    return text
    return None


def _usage(blob: Any) -> dict[str, int] | None:
    """只保留整数 inputTokens / outputTokens；三套命名按对匹配，缺失或非法 -> None。"""
    if not isinstance(blob, dict):
        return None
    for in_key, out_key in zip(_INPUT_KEYS, _OUTPUT_KEYS):
        if in_key in blob and out_key in blob:
            i_val, o_val = blob[in_key], blob[out_key]
            if _is_int(i_val) and _is_int(o_val):
                return {"inputTokens": i_val, "outputTokens": o_val}
    return None


def _finish_reason(event: dict[str, Any]) -> str | None:
    choices = event.get("choices")
    if isinstance(choices, list) and choices and isinstance(choices[0], dict):
        value = choices[0].get("finish_reason")
        if isinstance(value, str) and value:
            return value
    return None


def _classify(event: Any, raw_provider: str) -> ModelEvent:
    if not isinstance(event, dict):
        return _mk("unknown", raw_provider)

    # 1) error 键存在（最高优先级）。
    if "error" in event:
        return _mk("error", raw_provider)

    # 2) start markers。
    if event.get("type") in ("message_start", "start"):
        return _mk("start", raw_provider)

    # 3-5) delta（openai / anthropic / google）。
    text = _delta_text(event)
    if text is not None:
        return ModelEvent(
            type="delta",
            text_delta=text[:MAX_TEXT_DELTA],
            finish_reason=None,
            usage=None,
            raw_provider=raw_provider,
        )

    # 6) usage / usageMetadata 存在。
    if "usage" in event or "usageMetadata" in event:
        blob = event["usage"] if "usage" in event else event["usageMetadata"]
        return ModelEvent(
            type="usage",
            text_delta=None,
            finish_reason=None,
            usage=_usage(blob),
            raw_provider=raw_provider,
        )

    # 7) openai choices[0].finish_reason 非空。
    finish = _finish_reason(event)
    if finish is not None:
        return ModelEvent(
            type="done",
            text_delta=None,
            finish_reason=finish[:MAX_SHORT],
            usage=None,
            raw_provider=raw_provider,
        )

    # 8) anthropic message_stop。
    if event.get("type") == "message_stop":
        return _mk("done", raw_provider)

    return _mk("unknown", raw_provider)


def normalize_stream_event(provider: str, event: dict) -> ModelEvent:
    raw_provider = provider[:MAX_SHORT] if isinstance(provider, str) else ""
    try:
        return _classify(event, raw_provider)
    except BaseException:
        # 畸形输入（含任意嵌套垃圾）统一收敛到安全默认值，绝不抛异常。
        return _mk("unknown", raw_provider)
