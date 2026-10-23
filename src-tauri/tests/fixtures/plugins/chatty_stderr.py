#!/usr/bin/env python3
"""往 stderr 无限猛写：验证 64 KiB 环形缓冲真的不会无界增长。

通过 usage/import 触发输出，经过真实 capability 协商；支持超长无换行与非法 UTF-8。
"""

import json
import sys

PLUGIN_ID = sys.argv[1] if len(sys.argv) > 1 else "chatty-plugin"

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")

    if method == "initialize":
        sys.stdout.write(
            json.dumps(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {
                        "plugin_id": PLUGIN_ID,
                        "plugin_version": "0.1.0",
                        "supported_methods": ["usage/import"],
                    },
                }
            )
            + "\n"
        )
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "method": "initialized", "params": {}}) + "\n")
        sys.stdout.flush()
        continue

    if method == "usage/import":
        mode = (message.get("params") or {}).get("mode")
        if mode == "invalid":
            payload = b"\xff\xfe\nAFTER-INVALID\n"
        elif mode == "unterminated":
            payload = b"x" * (256 * 1024) + b"FLOOD-END"
        else:
            payload = (b"x" * 200 + b"\n") * 1300 + b"FLOOD-END\n"
        sys.stderr.buffer.write(payload)
        sys.stderr.buffer.flush()
        sys.stdout.write(
            json.dumps({"jsonrpc": "2.0", "id": request_id, "result": {"flooded": True}}) + "\n"
        )
        sys.stdout.flush()
        continue

    error = {"code": -32601, "message": f"method not found: {method}"}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "error": error}) + "\n")
    sys.stdout.flush()
