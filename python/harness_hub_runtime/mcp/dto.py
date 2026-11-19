"""F22 S1 — mcp 契约层冻结 DTO 与常量（spec §3.2 / §3.5 / §3.6）。

全部为 `@dataclass(frozen=True)`；`to_json()` 锁定跨边界的 camelCase 键名与顺序
（对齐 providers/dto.py 风格，docs/specs/2026-11-09-f22-mcp-client-server-design.md）。

截断纪律（spec §3.2 / §3.5 / §3.6）在 `__post_init__` 统一执行，任何构造路径都安全：
lastError ≤ 256、serverInfo.name/version ≤ 120、description ≤ 200、
inputSchemaKeys ≤ 32、textPreview ≤ 2048（截断即置 truncated）。

零依赖：只用 dataclasses / typing / __future__。
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

# 冻结常量（顺序即契约，spec §3.2）：MCP 规范版本支持集，降序（首选在前）。
PROTOCOL_VERSIONS = ("2025-06-18", "2025-03-26", "2024-11-05")
# 连接状态集（spec §3.2）：degraded = 连接后进程死亡 / 握手失败 / 调用超时，保留 lastError。
SERVER_STATES = ("disconnected", "connecting", "ready", "degraded")

# 字符串/集合上限（spec §3.2–§3.6）。
MAX_LAST_ERROR = 256
MAX_SERVER_INFO = 120
MAX_DESCRIPTION = 200
MAX_SCHEMA_KEYS = 32
MAX_TEXT_PREVIEW = 2048


def _clip(text: str, limit: int) -> str:
    """按上限截断字符串（契约纪律，调用方无需再自查）。"""
    return text[:limit]


@dataclass(frozen=True)
class McpServerInfo:
    """serverInfo {name, version}；未完成握手时两个字段均为 None。"""

    name: str | None
    version: str | None

    def __post_init__(self) -> None:
        if self.name is not None:
            object.__setattr__(self, "name", _clip(self.name, MAX_SERVER_INFO))
        if self.version is not None:
            object.__setattr__(self, "version", _clip(self.version, MAX_SERVER_INFO))

    def to_json(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "version": self.version,
        }


@dataclass(frozen=True)
class McpServerStatus:
    """单个 MCP server 的状态快照（spec §3.2 形状，键序冻结）。"""

    name: str
    transport: str
    state: str
    protocol_version: str | None
    server_info: McpServerInfo
    tool_count: int
    last_error: str | None
    env_keys: tuple[str, ...]

    def __post_init__(self) -> None:
        if self.last_error is not None:
            object.__setattr__(self, "last_error", _clip(self.last_error, MAX_LAST_ERROR))

    def to_json(self) -> dict[str, Any]:
        # envKeys 只含名字，绝不回值（spec §2 红线 1）。
        return {
            "name": self.name,
            "transport": self.transport,
            "state": self.state,
            "protocolVersion": self.protocol_version,
            "serverInfo": self.server_info.to_json(),
            "toolCount": self.tool_count,
            "lastError": self.last_error,
            "envKeys": list(self.env_keys),
        }


@dataclass(frozen=True)
class McpToolDescriptor:
    """tools/list 归一后的工具描述（spec §3.5：不回显完整 schema，防超大 payload）。"""

    name: str
    description: str
    input_schema_keys: tuple[str, ...]

    def __post_init__(self) -> None:
        object.__setattr__(self, "description", _clip(self.description or "", MAX_DESCRIPTION))
        object.__setattr__(
            self, "input_schema_keys", tuple(self.input_schema_keys)[:MAX_SCHEMA_KEYS]
        )

    def to_json(self) -> dict[str, Any]:
        return {
            "name": self.name,
            "description": self.description,
            "inputSchemaKeys": list(self.input_schema_keys),
        }


@dataclass(frozen=True)
class McpToolCallResult:
    """tools/call 响应归一（spec §3.6：摘要形状，不回显完整原文超 4 KiB）。"""

    is_error: bool
    content_count: int
    content_types: tuple[str, ...]
    text_preview: str | None
    truncated: bool

    def __post_init__(self) -> None:
        # 防御：即使构造方忘了置位，超限即截断并强制 truncated=True。
        if self.text_preview is not None and len(self.text_preview) > MAX_TEXT_PREVIEW:
            object.__setattr__(self, "text_preview", _clip(self.text_preview, MAX_TEXT_PREVIEW))
            object.__setattr__(self, "truncated", True)

    def to_json(self) -> dict[str, Any]:
        return {
            "isError": self.is_error,
            "contentCount": self.content_count,
            "contentTypes": list(self.content_types),
            "textPreview": self.text_preview,
            "truncated": self.truncated,
        }
