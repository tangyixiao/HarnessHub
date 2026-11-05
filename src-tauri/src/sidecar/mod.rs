//! Python AI Runtime sidecar 生命周期与 stdio JSON-RPC 客户端。
//!
//! 职责：按需拉起 `python/` 下的 sidecar、通过 stdio 交换 JSON-RPC 消息、在退出时回收进程。
//! **不为桌面端常驻开放 localhost HTTP 端口**（ADR-0006 / docs/CONTEXT.md）。
//!
//! 模块分工（F20）：
//!
//! - [`dto`]：跨 Tauri IPC 的 DTO，**所有键名由 serialization contract test 锁定**。
//! - [`config`]：`SidecarConfig` 与协议常量。
//! - [`process`]：子进程 spawn / 握手 / 调用 / 关闭（复用 `crate::plugins::rpc`）。
//! - [`manager`]：状态机、降级与崩溃恢复，**四个状态方法绝不返回 Err**。
//!
//! 本模块是领域模块，**不得依赖任何 Tauri GUI 类型**；只有 `commands.rs` 与
//! `lib.rs::run()` 允许碰 `tauri::State` / `AppHandle`（spec §2）。

pub mod config;
pub mod dto;
pub mod manager;
pub mod process;
