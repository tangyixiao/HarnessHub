"""MCP 工具协议平面（F22 S1：Python MCP client 契约层，spec §3.7）。

MCP 是标准边界（ADR-0009）：Harness Hub 既作为 MCP Client 消费外部工具，
也可以把自身能力暴露为 MCP Server（Server 方向归 Rust，见 spec §4）。

模块划分（spec §3.7）：

* `dto.py` —— 冻结 DTO 与常量（版本支持集、状态集、截断纪律）。
* `protocol.py` —— 协议核心：initialize / tools/list / tools/call 请求构造与响应归一。
* `transports.py` —— StdioTransport（行分隔 JSON-RPC）/ HttpTransport（urllib + SSE）。
* `registry.py` —— 连接注册表状态机（线程安全）。

零依赖：纯 stdlib，不 import 任何 MCP SDK（spec §0 非目标）。
"""

from __future__ import annotations

from .dto import (
    MAX_DESCRIPTION,
    MAX_LAST_ERROR,
    MAX_SCHEMA_KEYS,
    MAX_SERVER_INFO,
    MAX_TEXT_PREVIEW,
    PROTOCOL_VERSIONS,
    SERVER_STATES,
    McpServerInfo,
    McpServerStatus,
    McpToolCallResult,
    McpToolDescriptor,
)

__all__: list[str] = [
    "PROTOCOL_VERSIONS",
    "SERVER_STATES",
    "MAX_LAST_ERROR",
    "MAX_SERVER_INFO",
    "MAX_DESCRIPTION",
    "MAX_SCHEMA_KEYS",
    "MAX_TEXT_PREVIEW",
    "McpServerInfo",
    "McpServerStatus",
    "McpToolDescriptor",
    "McpToolCallResult",
]
