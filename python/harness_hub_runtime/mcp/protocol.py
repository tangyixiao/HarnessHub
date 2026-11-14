"""F22 S1 — MCP 协议核心（spec §3.3 / §3.5 / §3.6）。

initialize / tools/list / tools/call 的请求构造与响应归一；传输无关——对传输对象
只要求冻结接口 `send_request / send_notification / close`（spec §3.7），便于测试注入。

协商纪律（spec §3.3）：client 发 `protocolVersion`（缺省取首选）；server 回的版本
不在支持集 → McpProtocolError（错误信息含双方版本），registry 据此判 degraded。
initialize 成功后必须发 `notifications/initialized` 通知（无 id、不回包）。

零依赖：只用 stdlib。
"""

from __future__ import annotations

from typing import Any

from harness_hub_runtime import __version__

from .dto import PROTOCOL_VERSIONS, McpToolCallResult, McpToolDescriptor
from .transports import McpProtocolError  # noqa: F401 - 统一从 protocol 导出，便于上层引用

# client 标识（initialize 的 clientInfo）；版本与 sidecar 包版本一致。
CLIENT_NAME = "harness-hub-runtime"
# spec §3.3：initialize 成功后必须发送的通知（method, params）；无 id、不回包。
INITIALIZED_NOTIFICATION = ("notifications/initialized", {})

# 分页游标循尽的总上限（spec §3.5：总上限 512 个工具；页数上限取同值，防恶意
# server 用「空页 + 永久游标」把循环挂死）。
TOOLS_MAX = 512
# 超时预算（spec §3.6：tools/call 30s；握手与 tools/list 用 10s）。
HANDSHAKE_TIMEOUT = 10.0
DEFAULT_REQUEST_TIMEOUT = 10.0
TOOL_CALL_TIMEOUT = 30.0


def initialize_params(requested_version: str | None = None) -> dict[str, Any]:
    """构造 initialize 的 params；版本缺省取支持集首选（spec §3.3）。"""
    version = (
        requested_version
        if isinstance(requested_version, str) and requested_version
        else PROTOCOL_VERSIONS[0]
    )
    return {
        "protocolVersion": version,
        "capabilities": {},
        "clientInfo": {"name": CLIENT_NAME, "version": __version__},
    }


def check_negotiated_version(result: Any, requested: str) -> str:
    """校验 server 协商结果：版本不在支持集 → 判失败，错误信息含双方版本。"""
    if not isinstance(result, dict):
        raise McpProtocolError("initialize 响应不是 JSON 对象")
    version = result.get("protocolVersion")
    if not isinstance(version, str) or not version:
        raise McpProtocolError("server 响应缺少 protocolVersion")
    if version not in PROTOCOL_VERSIONS:
        raise McpProtocolError(
            f"协议版本协商失败：client 请求 {requested}，server 返回 {version}，"
            f"支持集 {'/'.join(PROTOCOL_VERSIONS)}"
        )
    return version


def server_info_from(result: Any) -> tuple[str, str] | None:
    """取 serverInfo {name, version}；缺失/畸形返回 None（registry 保持 null 快照）。"""
    info = result.get("serverInfo") if isinstance(result, dict) else None
    if isinstance(info, dict):
        name = info.get("name")
        version = info.get("version")
        return (
            str(name) if isinstance(name, str) else "",
            str(version) if isinstance(version, str) else "",
        )
    return None


def tool_from_raw(raw: Any) -> McpToolDescriptor | None:
    """单个原始 tool → McpToolDescriptor；无名/畸形条目直接丢弃。

    inputSchemaKeys 只取顶层 properties 键名（spec §3.5：不回显完整 schema，
    防超大 payload；完整 schema 留给未来按需获取）。
    """
    if not isinstance(raw, dict):
        return None
    name = raw.get("name")
    if not isinstance(name, str) or not name:
        return None
    description = raw.get("description")
    description = description if isinstance(description, str) else ""
    schema = raw.get("inputSchema")
    keys: tuple[str, ...] = ()
    if isinstance(schema, dict):
        properties = schema.get("properties")
        if isinstance(properties, dict):
            keys = tuple(str(key) for key in properties.keys())
    return McpToolDescriptor(name=name, description=description, input_schema_keys=keys)


def list_tools_via(transport: Any, timeout: float = DEFAULT_REQUEST_TIMEOUT) -> list[McpToolDescriptor]:
    """tools/list 分页游标循尽，总上限 512（spec §3.5）。"""
    tools: list[McpToolDescriptor] = []
    cursor: str | None = None
    pages = 0
    while pages < TOOLS_MAX:  # 页数上限 = 工具上限：防「空页 + 永久游标」死循环
        pages += 1
        params: dict[str, Any] = {} if cursor is None else {"cursor": cursor}
        result = transport.send_request("tools/list", params, timeout)
        if not isinstance(result, dict):
            raise McpProtocolError("tools/list 响应不是 JSON 对象")
        raw_tools = result.get("tools")
        if isinstance(raw_tools, list):
            for raw in raw_tools:
                tool = tool_from_raw(raw)
                if tool is None:
                    continue
                tools.append(tool)
                if len(tools) >= TOOLS_MAX:
                    return tools[:TOOLS_MAX]
        next_cursor = result.get("nextCursor")
        if not isinstance(next_cursor, str) or not next_cursor:
            break
        cursor = next_cursor
    return tools[:TOOLS_MAX]


def tool_call_result_from_raw(result: Any) -> McpToolCallResult:
    """tools/call 响应归一（spec §3.6）：只拼 type=="text" 的 blocks。

    contentCount/contentTypes 统计全部 blocks；无 text block 时 textPreview 为 null；
    textPreview 超 2048 由 DTO 截断并置 truncated（spec §3.6：不回显完整原文超 4 KiB）。
    """
    if not isinstance(result, dict):
        raise McpProtocolError("tools/call 响应不是 JSON 对象")
    blocks = result.get("content")
    blocks = blocks if isinstance(blocks, list) else []
    types: list[str] = []
    texts: list[str] = []
    for block in blocks:
        if not isinstance(block, dict):
            continue
        block_type = block.get("type")
        if not isinstance(block_type, str):
            continue
        types.append(block_type)
        if block_type == "text":
            text = block.get("text")
            if isinstance(text, str):
                texts.append(text)
    # 多个 text block 用换行拼接（单 block 保持原文）；无 text block → null。
    preview = "\n".join(texts) if texts else None
    return McpToolCallResult(
        is_error=bool(result.get("isError", False)),
        content_count=len(blocks),
        content_types=tuple(types),
        text_preview=preview,
        truncated=False,
    )


def call_tool_via(transport: Any, tool: str, arguments: Any, timeout: float = TOOL_CALL_TIMEOUT) -> McpToolCallResult:
    """tools/call：arguments 缺失/非对象按 {} 发送（spec §3.6），30s 超时。"""
    if not isinstance(tool, str) or not tool:
        raise ValueError("tool 名必须是非空字符串")
    params = {"name": tool, "arguments": arguments if isinstance(arguments, dict) else {}}
    result = transport.send_request("tools/call", params, timeout)
    return tool_call_result_from_raw(result)
