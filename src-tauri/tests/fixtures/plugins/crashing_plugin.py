#!/usr/bin/env python3
"""握手成功后立刻自杀：验证「插件崩了宿主不崩」。

先正常答 initialize（否则测的是握手失败），再 os._exit(1) 硬退出 ——
不用 sys.exit，避免 Python 解释器在退出时 flush 出额外输出干扰断言。
"""

import json
import os
import sys

PLUGIN_ID = sys.argv[1] if len(sys.argv) > 1 else "crashing-plugin"

for line in sys.stdin:
    message = json.loads(line)
    if message.get("method") != "initialize":
        continue

    sys.stdout.write(
        json.dumps(
            {
                "jsonrpc": "2.0",
                "id": message.get("id"),
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

    sys.stderr.write("crashing-plugin: 握手完成，现在硬退出\n")
    sys.stderr.flush()
    os._exit(1)
