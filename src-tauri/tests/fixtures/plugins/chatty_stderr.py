#!/usr/bin/env python3
"""往 stderr 无限猛写：验证 64 KiB 环形缓冲真的不会无界增长。

一次 flood/stderr 调用里写 ~256 KiB，然后正常回包，让宿主有机会统计缓冲大小。
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
                        "supported_methods": ["flood/stderr"],
                    },
                }
            )
            + "\n"
        )
        sys.stdout.write(json.dumps({"jsonrpc": "2.0", "method": "initialized", "params": {}}) + "\n")
        sys.stdout.flush()
        continue

    if method == "flood/stderr":
        chunk = "x" * 200 + "\n"
        for _ in range(1300):  # ~260 KiB
            sys.stderr.write(chunk)
        sys.stderr.flush()
        sys.stdout.write(
            json.dumps({"jsonrpc": "2.0", "id": request_id, "result": {"flooded": True}}) + "\n"
        )
        sys.stdout.flush()
        continue

    error = {"code": -32601, "message": f"method not found: {method}"}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "error": error}) + "\n")
    sys.stdout.flush()
