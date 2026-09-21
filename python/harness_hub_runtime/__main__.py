"""允许 `python -m harness_hub_runtime` 直接启动 sidecar。"""

from harness_hub_runtime.rpc.server import main

if __name__ == "__main__":
    raise SystemExit(main())
