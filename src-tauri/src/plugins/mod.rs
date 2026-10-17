//! 插件宿主。
//!
//! 职责：加载插件清单、按 manifest 授予受限 Host API、隔离插件故障。
//! 插件**不得**直接访问主数据库与 Secret Store（ADR-0018）。
//!
//! 模块分工（v0.1）：
//!
//! - [`manifest`]：解析 `manifest.toml` 并校验；`[capabilities]` 是授权白名单的唯一来源。
//! - [`rpc`]：行分隔 JSON-RPC 2.0 编解码，插件进程通信的唯一出口。
//! - [`adapters`]：v0.1 打通哪些方法、其余一律 `not_supported`。
//! - [`host_api`]：宿主真正提供给插件的受限能力，越权请求在这里被拒。
//! - [`registry`]：插件索引与状态的持久化。
//! - [`supervisor`]：子进程生命周期、握手、超时与降级。

pub mod adapters;
pub mod host_api;
pub mod manifest;
pub mod registry;
pub mod rpc;
pub mod supervisor;
