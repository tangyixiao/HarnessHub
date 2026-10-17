#!/usr/bin/env python3
"""usage-connect 适配器：connect 型示例插件。

协议：行分隔 JSON-RPC 2.0 over stdio。宿主往 stdin 写一行一个完整 JSON 请求，
本进程往 stdout 写一行一个完整 JSON 响应；stderr 只用于诊断。

这个文件是**故意自包含**的：只用 Python 3 标准库，自带读循环，不 import
harness_hub_runtime.rpc（插件进程不该依赖宿主内部包）。

支持的方法：
    initialize   -> 握手回包，随后补发 initialized 通知
    usage/import -> 返回 {"session": [ccusage unified session 行, ...]}
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

PLUGIN_ID = "usage-connect"
PLUGIN_VERSION = "0.1.0"

# 宿主只认 supported_methods ∩ manifest capabilities。manifest 里声明了
# [capabilities] usage = true，所以这里只能报 usage/import。
SUPPORTED_METHODS = ["usage/import"]

# JSON-RPC 2.0 标准错误码 + 插件域错误码（与 plugins/sdk/protocol.md 一致）。
PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INTERNAL_ERROR = -32603
NOT_SUPPORTED = -32001
CAPABILITY_DENIED = -32002
NOT_IMPLEMENTED = -32003

# fixtures/ 与本脚本同级：adapter/usage.py -> adapter/ -> 插件根 -> fixtures/
PLUGIN_ROOT = Path(__file__).resolve().parent.parent
FIXTURE_PATH = PLUGIN_ROOT / "fixtures" / "sample.json"


def log(message: str) -> None:
    """诊断信息只走 stderr。宿主把它收进 64 KiB 环形缓冲，stdout 必须干净。"""
    print(f"[{PLUGIN_ID}] {message}", file=sys.stderr, flush=True)


def force_utf8_stdio() -> None:
    """把 stdin/stdout 钉死成 UTF-8 + LF。

    Windows 上 Python 只在**控制台**用 UTF-8（PEP 528），stdout 一旦是管道就退回
    本地代码页（中文环境是 cp936），非 ASCII 内容会被写成非法 UTF-8 字节，宿主按
    UTF-8 解 JSON 行时直接解析失败。示例插件显式开启，见 plugins/sdk/protocol.md。
    """
    sys.stdin.reconfigure(encoding="utf-8", errors="strict")
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")


def load_fixture() -> list[dict]:
    """读取并做最小自检的 fixture，返回 ccusage unified session 行列表。"""
    raw = FIXTURE_PATH.read_text(encoding="utf-8")
    payload = json.loads(raw)
    rows = payload.get("session")
    if not isinstance(rows, list):
        raise ValueError(f"{FIXTURE_PATH} 顶层缺少 session 数组")
    return rows


def handle_usage_import(params: dict) -> dict:
    """usage/import：把 fixture 原样当作 ccusage 报告返回。

    注意键名是**单数** session：宿主把 result 直接当 ccusage 报告喂给已有的
    parse_report / normalize_session，多一层包装就解析不了。
    """
    rows = load_fixture()
    log(f"usage/import source={params.get('source')!r} rows={len(rows)}")
    return {"session": rows}


def handle_request(request: dict) -> list[dict]:
    """处理一条请求，返回要写出的帧列表（通知请求返回空列表）。"""
    request_id = request.get("id")
    method = request.get("method")
    params = request.get("params") or {}

    if request_id is None:
        # 宿主发来的通知（例如将来的 shutdown）没有 id，不得回包。
        return []

    if method == "initialize":
        # 宿主会校验 plugin_id 与 manifest 完全一致，写错即握手失败。
        result = {
            "plugin_id": PLUGIN_ID,
            "plugin_version": PLUGIN_VERSION,
            "supported_methods": SUPPORTED_METHODS,
        }
        response = {"jsonrpc": "2.0", "id": request_id, "result": result}
        notification = {"jsonrpc": "2.0", "method": "initialized", "params": {}}
        return [response, notification]  # 两行，先回包后通知

    if method == "usage/import":
        return [{"jsonrpc": "2.0", "id": request_id, "result": handle_usage_import(params)}]

    if method in {"skill/list", "skill/get"}:
        # manifest 没声明 skills，宿主根本不会放行到这一层；留一条明确的错误。
        return [
            error_response(
                request_id,
                NOT_SUPPORTED,
                f"method not supported by {PLUGIN_ID}: {method}",
            )
        ]

    return [error_response(request_id, METHOD_NOT_FOUND, f"method not found: {method}")]


def error_response(request_id, code: int, message: str) -> dict:
    return {
        "jsonrpc": "2.0",
        "id": request_id,
        "error": {"code": code, "message": message},
    }


def main() -> int:
    force_utf8_stdio()
    log("ready (usage-connect v0.1.0, stdio JSON-RPC)")
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue

        try:
            request = json.loads(line)
        except json.JSONDecodeError as exc:
            # id 未知，按 JSON-RPC 2.0 规定填 null。
            reply = [error_response(None, PARSE_ERROR, f"parse error: {exc}")]
        else:
            if not isinstance(request, dict) or request.get("jsonrpc") != "2.0":
                reply = [
                    error_response(
                        request.get("id") if isinstance(request, dict) else None,
                        INVALID_REQUEST,
                        "invalid request: jsonrpc must be '2.0'",
                    )
                ]
            else:
                try:
                    reply = handle_request(request)
                except Exception as exc:  # noqa: BLE001 - 单个请求出错不能拖垮进程
                    log(f"internal error: {exc!r}")
                    reply = [
                        error_response(request.get("id"), INTERNAL_ERROR, f"internal error: {exc}")
                    ]

        for item in reply:
            sys.stdout.write(json.dumps(item, ensure_ascii=False) + "\n")
        sys.stdout.flush()

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
