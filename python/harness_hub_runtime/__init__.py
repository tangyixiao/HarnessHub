"""Harness Hub AI Runtime sidecar。

边界（docs/CONTEXT-MAP.md）：

* 本进程**不得**直接访问 SQLite —— 数据库归 Rust Control Plane 独占。
* 与 Rust 的通信走 **stdio JSON-RPC**，不为桌面端常驻开放 localhost HTTP 端口。
* 默认不联网：任何出网能力都必须由调用方显式请求，并且可关闭、可解释（local-first）。

当前阶段（v0.1）：只有 RPC 骨架与 `ping` / `runtime.info` 两个方法，
Provider / MCP / Tokenizer 等能力在 Phase 8 逐个接入。
"""

__version__ = "0.0.0"

__all__ = ["__version__"]
