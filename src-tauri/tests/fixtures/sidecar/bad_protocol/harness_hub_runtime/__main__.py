"""F20 握手负路径 fixture：对任何请求都回一个不兼容的 protocolVersion。

Rust 侧 `initialize` 握手要求主版本一致（Host 发 "1.0"），这里故意自报
"2.0"，用来验证 `SidecarHandshake` → 状态收敛为 Degraded 而不是 panic。
只用标准库；不连数据库、不开 HTTP 端口（与真实 runtime 的边界一致）。
"""

import json
import sys


def main() -> int:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError:
            continue

        method = request.get("method") if isinstance(request, dict) else None
        if method == "shutdown":
            # 先回包再退出，避免 Rust 侧把 EOF 当成崩溃。
            response = {
                "jsonrpc": "2.0",
                "id": request.get("id"),
                "result": {"stopping": True},
            }
            sys.stdout.write(json.dumps(response, ensure_ascii=False) + "\n")
            sys.stdout.flush()
            break

        response = {
            "jsonrpc": "2.0",
            "id": request.get("id"),
            "result": {
                "protocolVersion": "2.0",
                "runtime": {
                    "name": "harness-hub-runtime",
                    "version": "0.5.0",
                    "transport": "stdio-jsonrpc",
                    "python": "0.0",
                    "protocolVersion": "2.0",
                    "providers": [],
                    "capabilities": ["ping", "runtime.info", "shutdown"],
                    "methods": ["ping", "runtime.info", "initialize", "shutdown"],
                },
                "capabilities": ["ping", "runtime.info", "shutdown"],
                "methods": ["ping", "runtime.info", "initialize", "shutdown"],
            },
        }
        sys.stdout.write(json.dumps(response, ensure_ascii=False) + "\n")
        sys.stdout.flush()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
