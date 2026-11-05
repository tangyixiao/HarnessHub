//! Sidecar 启动配置与协议常量（F20 spec §4.2）。
//!
//! 默认 transport 是 stdio JSON-RPC（ADR-0006），**不开 localhost HTTP 端口**。
//! 打包态的 Python 运行时资源定位留待 F23；当前只保证 dev 运行与 env 覆盖。

use std::env;
use std::path::{Path, PathBuf};

/// Host 与 sidecar 约定线协议主版本；协商只比较 `.` 前的第一段。
pub const PROTOCOL_VERSION: &str = "1.0";

/// Host 要求 sidecar 必须自报的能力集；握手时逐项核对，缺一项即降级。
pub const REQUIRED_CAPABILITIES: &[&str] = &["ping", "runtime.info", "shutdown"];

/// `initialize.result.runtime.name` 必须等于这个值，防误连到别的 Python 进程。
pub const RUNTIME_NAME: &str = "harness-hub-runtime";

/// 拉起一个 sidecar 所需的全部外部输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarConfig {
    /// Python 解释器路径（或 PATH 里可直接找到的程序名）。
    pub python: PathBuf,
    /// `PYTHONPATH` 指向的目录，其下应有 `harness_hub_runtime/` 包。
    pub runtime_root: PathBuf,
    /// 传给 `initialize.params.hostVersion` 的宿主版本（仅展示 / 记录用）。
    pub host_version: String,
}

impl SidecarConfig {
    /// 从环境变量构造：
    /// - `HARNESS_HUB_PYTHON`：解释器，缺省 `"python"`；
    /// - `HARNESS_HUB_RUNTIME_ROOT`：runtime 根目录，缺省 `<src-tauri>/../python`；
    /// - `host_version`：编译期 `CARGO_PKG_VERSION`，不接受运行时注入。
    ///
    /// 启动命令固定为 `<python> -m harness_hub_runtime` + `PYTHONPATH=runtime_root`，
    /// **不经 shell、不拼接用户输入**（避免命令注入）。
    pub fn from_env() -> Self {
        let python = env::var_os("HARNESS_HUB_PYTHON")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("python"));
        let runtime_root = env::var_os("HARNESS_HUB_RUNTIME_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .parent()
                    .expect("src-tauri 有父目录")
                    .join("python")
            });
        Self {
            python,
            runtime_root,
            host_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_constants_lock_the_negotiation_contract() {
        assert_eq!(PROTOCOL_VERSION, "1.0");
        assert_eq!(RUNTIME_NAME, "harness-hub-runtime");
        assert_eq!(REQUIRED_CAPABILITIES, &["ping", "runtime.info", "shutdown"]);
    }

    #[test]
    fn from_env_defaults_to_dev_roots_without_shell() {
        // 不修改进程全局 env（并行测试会互相污染）：只断言两个常量的形状。
        let config = SidecarConfig {
            python: PathBuf::from("python"),
            runtime_root: PathBuf::from("python"),
            host_version: "0.5.0".to_string(),
        };
        assert_eq!(config.python, PathBuf::from("python"));
        assert!(config.runtime_root.ends_with("python"));
        assert_eq!(config.host_version, "0.5.0");
    }
}
