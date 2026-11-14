"""F22 S1 — 传输层（spec §3.7 / §2 红线 1、2、6）。

传输接口冻结（spec §3.7）：
    send_request(method, params, timeout) -> dict
    send_notification(method, params) -> None
    close() -> None（绝不抛异常）

* StdioTransport：`subprocess.Popen(list argv)` 参数数组直 exec——**绝不经 shell、
  不拼字符串**（spec §2 红线 2，与 sidecar/process.rs 同纪律）；env 继承宿主进程
  后单次覆盖（红线 1：Rust 不在 IPC 里传 env 值，值只走子进程环境）；行分隔
  JSON-RPC；Windows 加 CREATE_NO_WINDOW 防止闪窗。
* HttpTransport：urllib POST，Accept 同时声明 JSON 与 SSE；`Mcp-Session-Id` 响应头
  存住并在后续请求回传；HTTP 4xx 时无 session 头重试一次（server 不支持 session
  时）；SSE 只解析 `data:`/`event:` 最小子集，JSON 单对象响应也兼容。

零依赖：只用 stdlib（subprocess / threading / urllib）。
"""

from __future__ import annotations

import json
import os
import queue
import subprocess
import sys
import threading
import time
from typing import Any
from urllib.error import HTTPError
from urllib.request import Request, urlopen

Json = dict[str, Any]

# HTTP 传输缺省超时（spec §3.7：10s）。
HTTP_TIMEOUT = 10.0
# stdio 终止宽限期（spec §3.4：terminate → 2s → kill）。
TERMINATE_GRACE_SECONDS = 2.0
# Windows：不给 MCP server 子进程弹控制台窗口（0x08000000）。
CREATE_NO_WINDOW = 0x08000000
SESSION_HEADER = "Mcp-Session-Id"
# MCP streamable HTTP 的协议 Accept 头（JSON 响应与 SSE 事件流都接受）。
HTTP_ACCEPT = "application/json, text/event-stream"


class McpTransportError(Exception):
    """传输层失败（进程死亡 / 写入失败 / HTTP 错误）。"""


class McpTimeoutError(McpTransportError):
    """send_request 超时；registry 据此判 degraded（spec §3.2）。"""


class McpProtocolError(McpTransportError):
    """server 返回的 JSON-RPC error 或违反 MCP 协议形状。"""


class McpTransport:
    """传输接口基类：接口冻结，任何实现不得偏离签名（spec §3.7）。"""

    def send_request(self, method: str, params: dict, timeout: float = HTTP_TIMEOUT) -> dict:
        raise NotImplementedError

    def send_notification(self, method: str, params: dict) -> None:
        raise NotImplementedError

    def close(self) -> None:
        raise NotImplementedError


class StdioTransport(McpTransport):
    """行分隔 JSON-RPC 子进程传输。

    读用后台线程 + 队列（Windows 管道没有可靠的带超时读），响应按 id 匹配；
    server 主动发的通知/请求被跳过，绝不让单个方法崩掉读线程。
    """

    def __init__(self, argv: list[str], env: dict[str, str] | None = None) -> None:
        # 红线 2：命令必须是参数数组，绝不接 shell 字符串。
        if not isinstance(argv, (list, tuple)) or not argv:
            raise ValueError("argv 必须是非空列表（绝不经 shell、不拼字符串）")
        if not all(isinstance(item, str) and item for item in argv):
            raise ValueError("argv 每一项必须是非空字符串")
        self._argv = list(argv)

        # 红线 1：值来源 = 继承宿主进程环境 + 单次覆盖；env 值绝不写日志/持久层。
        merged_env = dict(os.environ)
        if env:
            merged_env.update(env)

        self._creationflags = CREATE_NO_WINDOW if sys.platform == "win32" else 0
        self._process = subprocess.Popen(
            self._argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            # stderr 直接丢弃：既防管道写满导致子进程阻塞，也避免 server 日志
            # （可能含敏感信息）进入我们的错误文本。
            stderr=subprocess.DEVNULL,
            env=merged_env,
            text=True,
            encoding="utf-8",
            creationflags=self._creationflags,
        )
        self._messages: queue.Queue[Any] = queue.Queue()
        self._write_lock = threading.Lock()
        self._next_id = 1
        self._closed = False
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()

    def _read_loop(self) -> None:
        stdout = self._process.stdout
        if stdout is None:  # pragma: no cover - Popen(PIPE) 恒有 stdout
            self._messages.put(None)
            return
        try:
            for line in stdout:
                line = line.strip()
                if not line:
                    continue
                try:
                    message = json.loads(line)
                except json.JSONDecodeError:
                    continue  # 非 JSON 行（如第三方 server 的 banner）静默忽略
                self._messages.put(message)
        except BaseException:  # noqa: BLE001 - 后台读线程：任何异常都按 EOF 收尾
            pass
        finally:
            self._messages.put(None)  # EOF 哨兵

    def _send_message(self, payload: dict) -> None:
        if self._closed:
            raise McpTransportError("stdio 传输已关闭")
        line = json.dumps(payload, ensure_ascii=False) + "\n"
        stdin = self._process.stdin
        if stdin is None:  # pragma: no cover - Popen(PIPE) 恒有 stdin
            raise McpTransportError("stdin 不可用")
        try:
            with self._write_lock:
                stdin.write(line)
                stdin.flush()
        except (OSError, ValueError) as error:
            raise McpTransportError(f"写入 stdin 失败（server 进程可能已退出）：{error}") from error

    def send_request(self, method: str, params: dict, timeout: float = HTTP_TIMEOUT) -> dict:
        if not isinstance(method, str) or not method:
            raise ValueError("method 必须是非空字符串")
        request_id = self._next_id
        self._next_id += 1
        self._send_message(
            {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params if isinstance(params, dict) else {}}
        )
        deadline = time.monotonic() + max(float(timeout), 0.0)
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise McpTimeoutError(f"等待 {method} 响应超时（{timeout}s）")
            try:
                message = self._messages.get(timeout=remaining)
            except queue.Empty:
                raise McpTimeoutError(f"等待 {method} 响应超时（{timeout}s）") from None
            if message is None:
                raise McpTransportError("server 进程已退出（stdout EOF）")
            if not isinstance(message, dict) or message.get("id") != request_id:
                continue  # 通知/乱序消息跳过，按 id 匹配响应
            if "error" in message:
                error = message.get("error") or {}
                raise McpProtocolError(f"server 返回错误 {error.get('code')}：{error.get('message')}")
            if "result" not in message:
                continue
            return message.get("result")

    def send_notification(self, method: str, params: dict) -> None:
        if not isinstance(method, str) or not method:
            raise ValueError("method 必须是非空字符串")
        self._send_message(
            {"jsonrpc": "2.0", "method": method, "params": params if isinstance(params, dict) else {}}
        )

    def close(self) -> None:
        """terminate → 2s → kill；绝不抛异常（spec §3.7）。"""
        if self._closed:
            return
        self._closed = True
        process = self._process
        try:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=TERMINATE_GRACE_SECONDS)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=TERMINATE_GRACE_SECONDS)
        except BaseException:  # noqa: BLE001 - close() 冻结语义：绝不抛
            pass
        for stream in (process.stdin, process.stdout):
            try:
                if stream is not None:
                    stream.close()
            except BaseException:  # noqa: BLE001
                pass
        try:
            self._reader.join(timeout=0.5)
        except BaseException:  # noqa: BLE001
            pass


def parse_json_object(raw: bytes) -> dict | None:
    """把字节解析成 JSON 对象；任何失败都返回 None（绝不抛）。"""
    try:
        parsed = json.loads(raw.decode("utf-8", errors="replace"))
    except (json.JSONDecodeError, ValueError):
        return None
    return parsed if isinstance(parsed, dict) else None


def parse_sse_messages(raw: bytes) -> list[dict]:
    """SSE 最小子集（spec §3.7）：只认 `data:` 行与 `event:` 行，注释行忽略。

    空行 = 事件分隔；同一事件的多条 data: 行按规范用 \\n 连接后再解析 JSON。
    非 JSON 的 data 行（如 keep-alive 心跳）静默跳过。
    """
    messages: list[dict] = []
    data_lines: list[str] = []
    for line in raw.decode("utf-8", errors="replace").splitlines():
        if line.startswith("data:"):
            data_lines.append(line[len("data:"):].lstrip(" "))
        elif line.startswith(":"):
            continue  # SSE 注释 / keep-alive
        elif not line.strip():
            # 空行：事件结束，尝试解析已积累的 data 行。
            if data_lines:
                message = parse_json_object("\n".join(data_lines).encode("utf-8"))
                if message is not None:
                    messages.append(message)
                data_lines = []
        # event:/retry:/id: 等字段在最小子集之外，忽略
    if data_lines:
        # 流结束时仍未遇到空行的最后一个事件（宽松收尾）。
        message = parse_json_object("\n".join(data_lines).encode("utf-8"))
        if message is not None:
            messages.append(message)
    return messages


def decode_response_body(content_type: str, raw: bytes) -> list[dict]:
    """按 Content-Type 解析 HTTP 响应体：SSE 流或 JSON 单对象都兼容。"""
    if not raw:
        return []
    if "text/event-stream" in (content_type or "").lower():
        return parse_sse_messages(raw)
    message = parse_json_object(raw)
    return [message] if message is not None else []


class HttpTransport(McpTransport):
    """MCP Streamable HTTP 传输（urllib POST，无第三方依赖）。

    - 每个请求一次 POST；Accept 同时声明 JSON 与 SSE。
    - `Mcp-Session-Id` 响应头存住并在后续请求回传。
    - HTTP 4xx 且已带 session 时：无 session 头重试一次（server 不支持 session
      或 session 已失效的降级路径）；重试仍失败则抛 McpTransportError。5xx 不重试。
    """

    def __init__(self, url: str, headers: dict[str, str] | None = None, timeout: float = HTTP_TIMEOUT) -> None:
        if not isinstance(url, str) or not url.strip():
            raise ValueError("url 必须是非空字符串")
        self._url = url
        self._headers = {str(key): str(value) for key, value in (headers or {}).items()}
        self._timeout = float(timeout)
        self._session_id: str | None = None
        self._next_id = 1

    def _post(self, payload: dict, timeout: float) -> list[dict]:
        while True:
            headers = {"Content-Type": "application/json", "Accept": HTTP_ACCEPT}
            headers.update(self._headers)
            had_session = self._session_id is not None
            if had_session:
                headers[SESSION_HEADER] = self._session_id
            body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
            request = Request(self._url, data=body, headers=headers, method="POST")
            try:
                with urlopen(request, timeout=timeout) as response:
                    session_id = response.headers.get(SESSION_HEADER)
                    content_type = response.headers.get("Content-Type", "")
                    raw = response.read()
                if session_id:
                    self._session_id = session_id
                return decode_response_body(content_type, raw)
            except HTTPError as error:
                # 404/4xx：无 session 头重试一次（server 不支持 session 时）；5xx 不重试。
                if had_session and 400 <= error.code < 500:
                    self._session_id = None
                    continue
                raise McpTransportError(f"HTTP {error.code}：{error.reason}") from error
            except OSError as error:  # URLError / socket.timeout 都是其子类
                raise McpTransportError(f"HTTP 请求失败：{error}") from error

    def send_request(self, method: str, params: dict, timeout: float = HTTP_TIMEOUT) -> dict:
        if not isinstance(method, str) or not method:
            raise ValueError("method 必须是非空字符串")
        request_id = self._next_id
        self._next_id += 1
        payload = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params if isinstance(params, dict) else {},
        }
        messages = self._post(payload, float(timeout) if timeout else HTTP_TIMEOUT)
        # POST 一请求一响应：取第一个含 result/error 的消息即可（SSE 流里混入的
        # server→client 请求/通知没有这两个键，天然被跳过）。
        for message in messages:
            if "error" in message:
                error = message.get("error") or {}
                raise McpProtocolError(f"server 返回错误 {error.get('code')}：{error.get('message')}")
            if "result" in message:
                return message.get("result")
        raise McpTransportError("HTTP 响应中没有 JSON-RPC 结果")

    def send_notification(self, method: str, params: dict) -> None:
        if not isinstance(method, str) or not method:
            raise ValueError("method 必须是非空字符串")
        payload = {
            "jsonrpc": "2.0",
            "method": method,
            "params": params if isinstance(params, dict) else {},
        }
        # 通知无 id、不期待响应；空 body（202 Accepted 语义）完全合法。
        self._post(payload, self._timeout)

    def close(self) -> None:
        """HTTP 无常驻资源；冻结接口要求 close 永不抛异常。"""
        return None
