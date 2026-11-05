"""Tokenization 契约层（F21 / S2，spec §3.7 / §4.4 / §5）。

边界（local-first / ADR-0003 / ADR-0008）：

* **零第三方依赖**：只用标准库（dataclasses / math / typing），绝不 import
  tiktoken / tokenizers / litellm 等。
* **不联网**：不发起任何网络请求，不读取任何凭据 / API Key。
* **不碰 SQLite**：数据库归 Rust Control Plane 独占。
* **不接收原文**：``estimate()`` 只接收文本长度（int），永不接收 prompt / response 原文。
* 估计值恒标记 ``estimated=True``，绝不把启发式估算伪装成 provider 上报的精确值。

公开 API：四级可信度 precedence、token 估算、tokenizer-model revision 绑定。
"""

from __future__ import annotations

from .estimate import TokenEstimate, estimate
from .revision import TOKENIZER_REVISIONS, RevisionBinding, bind_revision
from .truth import LOCAL_LEVELS, TRUTH_PRECEDENCE, select_truth, truth_rank

__all__ = [
    "TRUTH_PRECEDENCE",
    "LOCAL_LEVELS",
    "truth_rank",
    "select_truth",
    "TokenEstimate",
    "estimate",
    "TOKENIZER_REVISIONS",
    "RevisionBinding",
    "bind_revision",
]
