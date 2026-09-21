"""Provider 平面（Phase 8）。

计划中的内容：

* `gateway.py` —— LiteLLM 统一入口（ADR-0007：LiteLLM first）。
* `litellm_adapter.py` / `openai_adapter.py` / `anthropic_adapter.py` / `google_adapter.py`
  —— 需要原生能力时才走的 escape hatch。

当前为空：v0.1 的 Token / Cost 统计走 ccusage，不经过本模块。
"""

__all__: list[str] = []
