//! Python AI Runtime sidecar 生命周期与 stdio JSON-RPC 客户端。
//!
//! 职责：按需拉起 `python/` 下的 sidecar、通过 stdio 交换 JSON-RPC 消息、在退出时回收进程。
//! **不为桌面端常驻开放 localhost HTTP 端口**（ADR-0006 / docs/CONTEXT.md）。
//!
//! 当前为占位模块（Phase 8「Unified AI Runtime」）：v0.1 只要求架构上预留，
//! 因此这里不提供公开 API。
