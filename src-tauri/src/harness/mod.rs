//! Harness 适配层：能力矩阵、生命周期接口与适配器注册表。
//!
//! 对外只暴露 [`HarnessAdapter`] / [`HarnessCapabilities`] / [`HarnessRegistry`]，
//! 具体 Harness 实现放在 `adapters/` 下逐个接入。

pub mod adapter;
pub mod adapters;
pub mod probe;
pub mod registry;

pub use adapter::{
    DetectResult, HarnessAdapter, HarnessCapabilities, HarnessId, LaunchRequest, ProcessHandle,
    ResumeRequest,
};
pub use registry::HarnessRegistry;
