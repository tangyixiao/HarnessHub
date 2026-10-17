#!/usr/bin/env python3
"""hello-skill 适配器：skill 型示例插件。

协议：行分隔 JSON-RPC 2.0 over stdio。宿主往 stdin 写一行一个完整 JSON 请求，
本进程往 stdout 写一行一个完整 JSON 响应；stderr 只用于诊断。

这个文件是**故意自包含**的：只用 Python 3 标准库，自带读循环，不 import
harness_hub_runtime.rpc（插件进程不该依赖宿主内部包）。

支持的方法：
    initialize  -> 握手回包，随后补发 initialized 通知
    skill/list  -> {"skills": [{"skill_id", "name", "description"}]}
    skill/get   -> {"skill_id", "name", "description", "body"}，body 是 SKILL.md 原文
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

PLUGIN_ID = "hello-skill"
PLUGIN_VERSION = "0.1.0"

# 宿主只认 supported_methods ∩ manifest capabilities。manifest 里声明了
# [capabilities] skills = true，所以这里报 skill/list 与 skill/get。
SUPPORTED_METHODS = ["skill/list", "skill/get"]

# JSON-RPC 2.0 标准错误码 + 插件域错误码（与 plugins/sdk/protocol.md 一致）。
PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INTERNAL_ERROR = -32603
NOT_SUPPORTED = -32001
CAPABILITY_DENIED = -32002
NOT_IMPLEMENTED = -32003

# skills/ 与本脚本同级：adapter/skill.py -> adapter/ -> 插件根 -> skills/
PLUGIN_ROOT = Path(__file__).resolve().parent.parent
SKILLS_ROOT = PLUGIN_ROOT / "skills"


def log(message: str) -> None:
    """诊断信息只走 stderr。宿主把它收进 64 KiB 环形缓冲，stdout 必须干净。"""
    print(f"[{PLUGIN_ID}] {message}", file=sys.stderr, flush=True)


def force_utf8_stdio() -> None:
    """把 stdin/stdout 钉死成 UTF-8 + LF。

    Windows 上 Python 只对**控制台**用 UTF-8（PEP 528），一旦 stdout 是管道就退回
    本地代码页（中文环境是 cp936）：SKILL.md 里的中文会被写成非法 UTF-8 字节，
    宿主按 UTF-8 解 JSON 行时直接解析失败。这类 bug 在英文 fixture 上永远不复现，
    所以示例插件在这里显式开启。
    """
    sys.stdin.reconfigure(encoding="utf-8", errors="strict")
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")


def split_frontmatter(text: str) -> dict[str, str]:
    """只从 SKILL.md 头部提取 name/description，**不改写正文**。

    这是 demo 用的极简实现：只认 `---` 包围的 `key: value` 单行字段，
    不支持 YAML 嵌套与多行。真实插件可以用 PyYAML，但这个示例坚持标准库。

    注意 body 一律是 SKILL.md 原文（含头部）：宿主对 SKILL.md 只做透传、
    不做解释，插件也不该替宿主决定"正文"从哪一行开始。
    """
    if not text.startswith("---"):
        return {}

    meta: dict[str, str] = {}
    for line in text.splitlines()[1:]:
        if line.strip() == "---":
            break
        key, sep, value = line.partition(":")
        if not sep:
            continue
        meta[key.strip()] = value.strip()
    return meta


def load_skill(skill_id: str) -> dict[str, str] | None:
    """按 skill_id 读取 skills/<id>/SKILL.md；目录不存在返回 None。

    skill_id 直接当目录名用，因此先挡掉路径穿越：只允许一层、不含分隔符的名字。
    """
    if not skill_id or skill_id in {".", ".."} or "/" in skill_id or "\\" in skill_id:
        return None

    skill_file = SKILLS_ROOT / skill_id / "SKILL.md"
    if not skill_file.is_file():
        return None

    text = skill_file.read_text(encoding="utf-8")
    meta = split_frontmatter(text)

    return {
        "skill_id": skill_id,
        "name": meta.get("name", skill_id),
        "description": meta.get("description", ""),
        # body 是 SKILL.md 原文：宿主只做透传，不做解释，也不重新渲染。
        "body": text,
    }


def list_skills() -> list[dict[str, str]]:
    """扫描 skills/*/SKILL.md，返回摘要列表（不含 body）。"""
    if not SKILLS_ROOT.is_dir():
        return []

    skills: list[dict[str, str]] = []
    for entry in sorted(SKILLS_ROOT.iterdir()):
        if not entry.is_dir():
            continue
        skill = load_skill(entry.name)
        if skill is not None:
            skills.append(
                {
                    "skill_id": skill["skill_id"],
                    "name": skill["name"],
                    "description": skill["description"],
                }
            )
    return skills


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

    if method == "skill/list":
        return [{"jsonrpc": "2.0", "id": request_id, "result": {"skills": list_skills()}}]

    if method == "skill/get":
        skill_id = params.get("skill_id")
        if not isinstance(skill_id, str) or not skill_id:
            return [error_response(request_id, INVALID_REQUEST, "skill/get 需要 params.skill_id")]
        skill = load_skill(skill_id)
        if skill is None:
            return [error_response(request_id, INVALID_REQUEST, f"unknown skill_id: {skill_id}")]
        log(f"skill/get skill_id={skill_id!r} body_chars={len(skill['body'])}")
        return [{"jsonrpc": "2.0", "id": request_id, "result": skill}]

    if method == "usage/import":
        # manifest 没声明 usage，宿主根本不会放行到这一层；留一条明确的错误。
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
    log("ready (hello-skill v0.1.0, stdio JSON-RPC)")
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
