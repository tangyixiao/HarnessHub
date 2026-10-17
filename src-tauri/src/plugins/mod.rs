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

pub mod adapters;
pub mod manifest;
pub mod rpc;
