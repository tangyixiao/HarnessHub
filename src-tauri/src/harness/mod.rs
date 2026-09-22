//! Harness 适配层：能力矩阵、生命周期接口、适配器注册表与清单同步。
//!
//! 对外只暴露 [`HarnessAdapter`] / [`HarnessCapabilities`] / [`HarnessRegistry`] /
//! [`HarnessSummary`]，具体 Harness 实现放在 `adapters/` 下逐个接入。
//!
//! 写路径只有一条：[`inventory::reconcile_harnesses`]（detect → reconcile → SQLite）。
//! 查询命令（`list_harnesses`）永远只读。

pub mod adapter;
pub mod adapters;
pub mod inventory;
pub mod probe;
pub mod registry;
pub mod store;

pub use adapter::{
    DetectResult, HarnessAdapter, HarnessCapabilities, HarnessId, LaunchRequest, ProcessHandle,
    ResumeRequest,
};
pub use inventory::ReconcileReport;
pub use registry::{HarnessRegistry, HarnessSummary};
