//! Runtime Target 抽象。
//!
//! 职责：为「Harness 在哪里运行」提供统一句柄与路径映射（本机 / 远程 / 容器）。
//! Core API 不允许假定绝对本机路径或本机进程（ADR-0021）；
//! 所有 Harness 运行都必须关联 `runtime_target_id`。
//!
//! v0.1 只实现 LocalRuntime（[`local`]），远程 / 容器运行时为 Phase 10。

pub mod local;

pub use local::LOCAL_TARGET_ID;
