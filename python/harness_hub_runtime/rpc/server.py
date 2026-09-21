"""stdio JSON-RPC 2.0 服务端。

为什么是 stdio 而不是 localhost HTTP（docs/adr/0001、docs/CONTEXT.md）：
桌面应用没有理由常驻开放一个本机端口；stdio 让 sidecar 的生命周期完全跟随父进程，
也天然避免了端口冲突与未授权访问。

报文约定：**一行一个 JSON 对象**（line-delimited）。不用 Content-Length 头，
因为通信双方都由本项目控制，行协议更容易调试与测试。

错误码沿用 JSON-RPC 2.0 标准：
    -32700 解析错误 / -32600 请求非法 / -32601 方法不存在 / -32603 内部错误
"""

from __future__ import annotations

import json
import sys
from typing import Any, Callable, Iterable, TextIO

from harness_hub_runtime import __version__

Json = dict[str, Any]
Handler = Callable[[Json], Any]
MethodTable = dict[str, Handler]

PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INTERNAL_ERROR = -32603


class RpcError(Exception):
    """处理器主动抛出的、需要映射成 JSON-RPC 错误对象的异常。"""

    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


def _error_response(request_id: Any, code: int, message: str) -> Json:
    return {"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}}


def handle_request(request: Any, methods: MethodTable) -> Json | None:
    """处理单条请求。

    返回 `None` 表示这是一条通知（notification，没有 `id`），按 JSON-RPC 规范不应回包。
    """
    if not isinstance(request, dict) or request.get("jsonrpc") != "2.0":
        request_id = request.get("id") if isinstance(request, dict) else None
        return _error_response(request_id, INVALID_REQUEST, "不是合法的 JSON-RPC 2.0 请求")

    request_id = request.get("id")
    method = request.get("method")
    handler = methods.get(method) if isinstance(method, str) else None

    if handler is None:
        response = _error_response(request_id, METHOD_NOT_FOUND, f"未知方法：{method}")
    else:
        params = request.get("params") or {}
        try:
            response = {"jsonrpc": "2.0", "id": request_id, "result": handler(params)}
        except RpcError as error:
            response = _error_response(request_id, error.code, error.message)
        except Exception as error:  # noqa: BLE001 - 兜底：sidecar 不能因为单个方法崩溃
            response = _error_response(request_id, INTERNAL_ERROR, str(error))

    # 通知不回包，但处理器仍会被执行。
    return None if request_id is None else response


def runtime_info(_params: Json) -> Json:
    """返回 sidecar 自身信息，用于握手与排障。"""
    return {
        "name": "harness-hub-runtime",
        "version": __version__,
        "transport": "stdio-jsonrpc",
        "python": sys.version.split()[0],
        # Phase 8 起这里会列出 LiteLLM / 官方 SDK / MCP 等已就绪的 Provider。
        "providers": [],
    }


def ping(_params: Json) -> Json:
    return {"pong": True}


DEFAULT_METHODS: MethodTable = {
    "ping": ping,
    "runtime.info": runtime_info,
}


def serve(
    stdin: Iterable[str],
    stdout: TextIO,
    methods: MethodTable | None = None,
) -> None:
    """从 `stdin` 逐行读取请求，把响应写到 `stdout`。"""
    table = DEFAULT_METHODS if methods is None else methods

    for line in stdin:
        line = line.strip()
        if not line:
            continue

        try:
            request = json.loads(line)
        except json.JSONDecodeError as error:
            response: Json | None = _error_response(None, PARSE_ERROR, f"JSON 解析失败：{error}")
        else:
            response = handle_request(request, table)

        if response is not None:
            stdout.write(json.dumps(response, ensure_ascii=False) + "\n")
            stdout.flush()


def main() -> int:
    serve(sys.stdin, sys.stdout)
    return 0
