"""F22 S1 — 连接注册表状态机（spec §3.2–§3.4）。

状态机：disconnected → connecting → ready | degraded。
degraded 判定（spec §3.2 / 任务纪律）：握手失败 / 进程死亡 / 调用超时，
保留 lastError（≤ 256 chars，由 DTO 截断兜底）。

线程安全：`threading.Lock` 保护 connections 字典；`transport_factory` 是注入缝
（测试用假传输，生产用 default_transport_factory）。

安全红线（spec §2）：
- 快照 envKeys / headers 只回**名字**，值绝不进入快照、日志或错误文本。
- stdio 命令经参数数组直 exec（transports 层保证），绝不经 shell。

rpc 层错误映射约定：
- ValueError        → JSON-RPC -32602（参数非法）
- McpNotReadyError  → JSON-RPC -32002（message 带 server 名）
- 其余一律收敛为 degraded 快照，不抛 RPC 错（spec §3.3 状态收敛纪律）。
"""

from __future__ import annotations

import re
import threading
from typing import Any, Callable

from .dto import MAX_LAST_ERROR, PROTOCOL_VERSIONS, McpServerInfo, McpServerStatus
from .protocol import (
    DEFAULT_REQUEST_TIMEOUT,
    HANDSHAKE_TIMEOUT,
    INITIALIZED_NOTIFICATION,
    TOOL_CALL_TIMEOUT,
    call_tool_via,
    check_negotiated_version,
    initialize_params,
    list_tools_via,
    server_info_from,
)
from .transports import HttpTransport, McpTransport, StdioTransport

# 与 E18 mcp_servers registry 同规则（spec §3.3）。
NAME_PATTERN = re.compile(r"^[A-Za-z0-9._-]+$")
TRANSPORT_KINDS = ("stdio", "http")

TransportFactory = Callable[[dict], McpTransport]


class McpNotReadyError(Exception):
    """server 未就绪（不存在 / connecting / degraded）；rpc 层映射为 -32002。"""


def _clip_error(message: str) -> str:
    """lastError ≤ 256（DTO 也会兜底，这里先收敛，避免存储侧超长）。"""
    return message[:MAX_LAST_ERROR]


def _validate_connect_params(params: Any) -> dict:
    """connect 参数校验；非法一律 ValueError（rpc 层映射 -32602，spec §3.3）。"""
    if not isinstance(params, dict):
        raise ValueError("params 必须是对象")
    name = params.get("name")
    if not isinstance(name, str) or not NAME_PATTERN.match(name):
        raise ValueError(f"name 不匹配 ^[A-Za-z0-9._-]+$：{name!r}")
    kind = params.get("transport")
    if kind not in TRANSPORT_KINDS:
        raise ValueError(f"transport 必须是 {'/'.join(TRANSPORT_KINDS)}：{kind!r}")
    if kind == "stdio":
        command = params.get("command")
        if not isinstance(command, str) or not command:
            raise ValueError("stdio 传输需要非空 command")
        args = params.get("args")
        args = [] if args is None else args
        if not isinstance(args, (list, tuple)) or not all(isinstance(item, str) for item in args):
            raise ValueError("args 必须是字符串列表")
        env = params.get("env")
        env = {} if env is None else env
        if not isinstance(env, dict) or not all(
            isinstance(key, str) and isinstance(value, str) for key, value in env.items()
        ):
            raise ValueError("env 必须是字符串到字符串的映射")
    else:
        url = params.get("url")
        if not isinstance(url, str) or not url:
            raise ValueError("http 传输需要非空 url")
        headers = params.get("headers")
        headers = {} if headers is None else headers
        if not isinstance(headers, dict) or not all(
            isinstance(key, str) and isinstance(value, str) for key, value in headers.items()
        ):
            raise ValueError("headers 必须是字符串到字符串的映射")
    return params


def _env_key_names(params: dict) -> tuple[str, ...]:
    """快照只收名字（stdio → env 键名；http → headers 键名），值绝不进入。"""
    source = params.get("env") if params.get("transport") == "stdio" else params.get("headers")
    if not isinstance(source, dict):
        return ()
    return tuple(sorted(str(key) for key in source.keys()))


def default_transport_factory(params: dict) -> McpTransport:
    """生产传输工厂：stdio → 子进程，http → urllib；参数已在 connect 校验过。"""
    kind = params.get("transport")
    if kind == "stdio":
        command = params.get("command")
        args = params.get("args")
        args = list(args) if isinstance(args, (list, tuple)) else []
        env = params.get("env")
        env = {k: v for k, v in env.items()} if isinstance(env, dict) else None
        return StdioTransport([command, *args], env=env)
    if kind == "http":
        headers = params.get("headers")
        headers = {k: v for k, v in headers.items()} if isinstance(headers, dict) else None
        return HttpTransport(params["url"], headers=headers)
    raise ValueError(f"未知传输类型：{kind!r}")


class _Connection:
    """单条连接的可变运行状态；对外快照一律经 McpServerStatus 冻结输出。"""

    def __init__(self, name: str, transport_kind: str, env_keys: tuple[str, ...]) -> None:
        self.name = name
        self.transport_kind = transport_kind
        self.env_keys = env_keys
        self.state = "connecting"
        self.protocol_version: str | None = None
        self.server_info = McpServerInfo(name=None, version=None)
        self.tool_count = 0
        self.last_error: str | None = None
        self.transport: McpTransport | None = None

    def degrade(self, message: str) -> None:
        """判 degraded：保留 lastError，并关闭传输（进程死亡/超时后连接不可再用）。"""
        self.state = "degraded"
        self.last_error = _clip_error(str(message))
        transport = self.transport
        self.transport = None
        if transport is not None:
            try:
                transport.close()
            except BaseException:  # noqa: BLE001 - 收敛路径绝不抛
                pass

    def close_best_effort(self) -> None:
        transport = self.transport
        self.transport = None
        if transport is not None:
            try:
                transport.close()
            except BaseException:  # noqa: BLE001
                pass

    def to_status(self) -> dict:
        return McpServerStatus(
            name=self.name,
            transport=self.transport_kind,
            state=self.state,
            protocol_version=self.protocol_version,
            server_info=self.server_info,
            tool_count=self.tool_count,
            last_error=self.last_error,
            env_keys=self.env_keys,
        ).to_json()


class McpConnectionRegistry:
    """进程级连接注册表（spec §3.7）：connect / disconnect / status / tools_*。"""

    def __init__(self, transport_factory: TransportFactory | None = None) -> None:
        self._lock = threading.Lock()
        self._connections: dict[str, _Connection] = {}
        self._transport_factory = transport_factory or default_transport_factory

    # ---- 连接生命周期 -------------------------------------------------

    def connect(self, params: dict, transport_factory: TransportFactory | None = None) -> dict:
        """连接一个 MCP server；返回该 server 的 mcp.status 单元素形状。

        同名重连先隐式断旧（spec §3.3）；除 ValueError（参数非法）外的任何失败
        都收敛为 state=degraded + lastError 的正常返回，不抛 RPC 错。
        """
        params = _validate_connect_params(params)
        name = params["name"]
        kind = params["transport"]
        factory = transport_factory or self._transport_factory
        env_keys = _env_key_names(params)

        with self._lock:
            old = self._connections.pop(name, None)
        if old is not None:
            old.close_best_effort()

        connection = _Connection(name=name, transport_kind=kind, env_keys=env_keys)
        with self._lock:
            self._connections[name] = connection  # connecting 状态立即可见

        try:
            transport = factory(params)
        except Exception as error:  # noqa: BLE001 - 传输创建失败 → degraded
            connection.degrade(f"transport 创建失败：{error}")
            return connection.to_status()
        connection.transport = transport

        try:
            requested = params.get("protocolVersion")
            requested_version = requested if isinstance(requested, str) else PROTOCOL_VERSIONS[0]
            result = transport.send_request("initialize", initialize_params(requested), HANDSHAKE_TIMEOUT)
            version = check_negotiated_version(result, requested_version)
            info = server_info_from(result)
            if info is not None:
                connection.server_info = McpServerInfo(name=info[0], version=info[1])
            # spec §3.3：initialize 成功后必须发 notifications/initialized（无 id）。
            transport.send_notification(INITIALIZED_NOTIFICATION[0], INITIALIZED_NOTIFICATION[1])
        except Exception as error:  # noqa: BLE001 - 握手/协商失败 → degraded（保留双方版本）
            connection.degrade(str(error) or error.__class__.__name__)
            return connection.to_status()

        connection.state = "ready"
        connection.protocol_version = version
        return connection.to_status()

    def disconnect(self, name: str) -> dict:
        """断开并清理；未知名幂等返回 disconnected（spec §3.4）。"""
        with self._lock:
            connection = self._connections.pop(name, None)
        if connection is not None:
            connection.close_best_effort()
        return {"state": "disconnected", "name": name}

    def close_all(self) -> None:
        """全关（best-effort，绝不抛）：sidecar 退出路径防孤儿 MCP 子进程。"""
        with self._lock:
            connections = list(self._connections.values())
            self._connections.clear()
        for connection in connections:
            connection.close_best_effort()

    # ---- 查询与工具 ---------------------------------------------------

    def status(self) -> list[dict]:
        """状态快照列表（按 name 排序，确定性输出）。"""
        with self._lock:
            connections = list(self._connections.values())
        snapshots = [connection.to_status() for connection in connections]
        snapshots.sort(key=lambda item: item["name"])
        return snapshots

    def _ready_connection(self, name: str) -> _Connection:
        with self._lock:
            connection = self._connections.get(name)
        if connection is None or connection.state != "ready" or connection.transport is None:
            raise McpNotReadyError(f"server {name} 未就绪")
        return connection

    def tools_list(self, name: str) -> list[dict]:
        """mcp.tools.list：分页循尽归一；失败判 degraded 并以 -32002 语义抛出。"""
        connection = self._ready_connection(name)
        try:
            tools = list_tools_via(connection.transport, DEFAULT_REQUEST_TIMEOUT)
        except Exception as error:  # noqa: BLE001 - 超时/进程死亡/协议错误 → degraded
            connection.degrade(str(error) or error.__class__.__name__)
            raise McpNotReadyError(f"server {name} 未就绪：{connection.last_error}") from error
        with self._lock:
            connection.tool_count = len(tools)  # 工具数在 tools.list 后刷新（spec §3.3）
        return [tool.to_json() for tool in tools]

    def tools_call(self, name: str, tool: str, arguments: Any) -> dict:
        """mcp.tools.call：30s 超时；超时/传输失败判 degraded（spec §3.6）。

        tool 级错误（isError=true）是成功的协议交换，作为数据返回，不判 degraded。
        """
        if not isinstance(tool, str) or not tool:
            raise ValueError("tool 名必须是非空字符串")
        if arguments is None:
            arguments = {}
        if not isinstance(arguments, dict):
            raise ValueError("arguments 必须是 JSON 对象")
        connection = self._ready_connection(name)
        try:
            result = call_tool_via(connection.transport, tool, arguments, TOOL_CALL_TIMEOUT)
        except Exception as error:  # noqa: BLE001 - 调用超时/进程死亡 → degraded
            connection.degrade(str(error) or error.__class__.__name__)
            raise McpNotReadyError(f"server {name} 未就绪：{connection.last_error}") from error
        return result.to_json()
