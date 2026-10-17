#!/usr/bin/env python3
"""最小可用的正常插件：正确握手 + 应答 usage/import。

supervisor 的「全链路 OK」用例用它。argv[1] 是要自报的插件 id
（宿主会校验回包 id 与 manifest 一致，所以不能写死）。
"""

import json
import sys


def main() -> None:
    plugin_id = sys.argv[1] if len(sys.argv) > 1 else "echo-plugin"

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        message = json.loads(line)

        method = message.get("method")
        request_id = message.get("id")

        if method == "initialize":
            response = {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "plugin_id": plugin_id,
                    "plugin_version": "0.1.0",
                    "supported_methods": ["usage/import", "skill/list"],
                },
            }
            sys.stdout.write(json.dumps(response) + "\n")
            sys.stdout.write(
                json.dumps({"jsonrpc": "2.0", "method": "initialized", "params": {}}) + "\n"
            )
            sys.stdout.flush()
            continue

        if method == "usage/import":
            result = {"ok": True, "plugin": plugin_id, "session": []}
            sys.stdout.write(
                json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}) + "\n"
            )
            sys.stdout.flush()
            continue

        error = {"code": -32601, "message": f"method not found: {method}"}
        sys.stdout.write(
            json.dumps({"jsonrpc": "2.0", "id": request_id, "error": error}) + "\n"
        )
        sys.stdout.flush()


if __name__ == "__main__":
    main()
