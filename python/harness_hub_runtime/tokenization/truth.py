"""Token Truth 四级可信度（ADR-0008 / R10，见 spec §5.1）。

仅做「候选值选择」与「等级排序」，零第三方依赖，不联网、不接触 SQLite，
不接收任何 prompt / response 原文。
"""

from __future__ import annotations

# 索引越小越可信；本元组顺序冻结，跨语言契约以 spec §5.1 为准。
TRUTH_PRECEDENCE: tuple[str, ...] = (
    "provider_reported",
    "provider_count_api",
    "local_exact",
    "estimated",
)

# F21 零依赖阶段本地只能产出这两级；前两级只作为更高优先级候选参与选择，不伪造。
LOCAL_LEVELS: tuple[str, ...] = ("local_exact", "estimated")


def truth_rank(level: str) -> int:
    """返回 level 在 TRUTH_PRECEDENCE 中的索引；未知 level 返回最差值 len(...)。"""
    try:
        return TRUTH_PRECEDENCE.index(level)
    except ValueError:
        return len(TRUTH_PRECEDENCE)


def select_truth(candidates: dict) -> tuple[str, int | None]:
    """按 TRUTH_PRECEDENCE 从高到低取第一个合法候选值。

    合法条件：值是 ``int``（**排除 bool**，尽管 bool 是 int 子类）且 ``>= 0``。
    None / 负数 / 非 int / bool / str 一律忽略；全无合法候选 -> ``("estimated", None)``。
    """
    for level in TRUTH_PRECEDENCE:
        if level not in candidates:
            continue
        value = candidates[level]
        if isinstance(value, bool) or not isinstance(value, int):
            continue
        if value < 0:
            continue
        return (level, value)
    return ("estimated", None)
