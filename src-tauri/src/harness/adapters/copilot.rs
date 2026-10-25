//! GitHub Copilot CLI（`@github/copilot`）的检测阶段：只探测 binary、版本与
//! 现存数据目录。
//!
//! launch / terminal / usage 尚未实现，能力矩阵保持全 false（ADR-0005 零宣称）；
//! Terminal 的能力过滤会自动不提供启动入口。不读取用户配置、认证或会话内容。
//! 未证实官方支持 home 覆盖 env，**不做**目录覆盖注入，只发现 home 下 `.copilot`。
//!
//! 事实依据（2026-10-25 真机取证）：PATH shim 为 npm 三件套（本机
//! `D:\npm-global\copilot(.cmd)`），`copilot --version` 首行输出
//! `GitHub Copilot CLI 1.0.91.`（次行为 update 提示，解析忽略），
//! 数据目录 `~/.copilot`（ide / logs / servers / session-state / skills 等）。

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::harness::adapter::{
    DetectResult, HarnessAdapter, HarnessCapabilities, HarnessId, LaunchRequest,
};
use crate::harness::launch::LaunchSpec;
use crate::harness::probe::HostProbe;

pub const COPILOT_ID: &str = "copilot";

pub struct CopilotAdapter {
    probe: Arc<dyn HostProbe>,
}

impl CopilotAdapter {
    pub fn new(probe: Arc<dyn HostProbe>) -> Self {
        Self { probe }
    }

    /// 只返回**确实存在**的数据目录，避免凭空声称。
    fn existing_data_dirs(&self) -> Vec<String> {
        self.probe
            .home_dir()
            .map(|home| home.join(".copilot"))
            .filter(|path| self.probe.dir_exists(path))
            .map(|path| vec![path.to_string_lossy().into_owned()])
            .unwrap_or_default()
    }
}

impl HarnessAdapter for CopilotAdapter {
    fn id(&self) -> HarnessId {
        HarnessId::from(COPILOT_ID)
    }

    fn display_name(&self) -> &str {
        "GitHub Copilot CLI"
    }

    /// `installed` 只由 binary 是否存在决定；版本探测失败只是 `version = None`。
    fn detect(&self) -> DetectResult {
        let binary = self.probe.find_executable(COPILOT_ID);
        let version = binary
            .as_deref()
            .and_then(|path| self.probe.read_version(path).ok().flatten());
        DetectResult {
            installed: binary.is_some(),
            binary_path: binary.map(|path| path.to_string_lossy().into_owned()),
            version,
            data_paths: self.existing_data_dirs(),
        }
    }

    fn version(&self) -> Result<Option<String>> {
        match self.probe.find_executable(COPILOT_ID) {
            Some(path) => self.probe.read_version(&path),
            None => Ok(None),
        }
    }

    /// 检测阶段**一个能力都不宣称**。
    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities::default()
    }

    fn build_launch_spec(&self, _request: LaunchRequest) -> Result<LaunchSpec> {
        Err(Error::InvalidInput(
            "GitHub Copilot CLI 启动尚未实现：当前仅支持安装检测".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::registry::HarnessRegistry;
    use crate::test_support::FakeHostProbe;
    use std::path::PathBuf;

    fn installed() -> FakeHostProbe {
        FakeHostProbe::new().with_binary(
            COPILOT_ID,
            "D:/npm-global/copilot.cmd",
            // 真机取证原文（两行）：semver 提取自首行，次行 update 提示被忽略。
            "GitHub Copilot CLI 1.0.91.\nRun 'copilot update' to check for updates.\n",
        )
    }

    #[test]
    fn detects_a_binary_and_parses_the_real_version_output() {
        let adapter = CopilotAdapter::new(Arc::new(installed()));
        let detected = adapter.detect();
        assert!(detected.installed);
        assert_eq!(
            detected.binary_path.as_deref(),
            Some("D:/npm-global/copilot.cmd")
        );
        assert_eq!(detected.version.as_deref(), Some("1.0.91"));
        assert_eq!(adapter.version().unwrap().as_deref(), Some("1.0.91"));
    }

    #[test]
    fn missing_binary_is_uninstalled_even_if_data_remains() {
        let adapter = CopilotAdapter::new(Arc::new(
            FakeHostProbe::new().with_dir("/home/dev/.copilot"),
        ));
        let detected = adapter.detect();
        assert!(!detected.installed);
        assert_eq!(detected.binary_path, None);
        assert_eq!(adapter.version().unwrap(), None);
        assert_eq!(detected.data_paths.len(), 1);
    }

    #[test]
    fn unreadable_version_does_not_hide_an_installed_binary() {
        let adapter = CopilotAdapter::new(Arc::new(FakeHostProbe::new().with_binary(
            COPILOT_ID,
            "D:/npm-global/copilot.cmd",
            "unknown",
        )));
        let detected = adapter.detect();
        assert!(detected.installed);
        assert_eq!(detected.version, None);
    }

    #[test]
    fn reports_only_existing_default_data_directories() {
        let absent = CopilotAdapter::new(Arc::new(installed())).detect();
        assert!(absent.data_paths.is_empty());

        let present =
            CopilotAdapter::new(Arc::new(installed().with_dir("/home/dev/.copilot"))).detect();
        assert_eq!(
            present.data_paths,
            vec![PathBuf::from("/home/dev")
                .join(".copilot")
                .to_string_lossy()
                .into_owned()]
        );
    }

    #[test]
    fn capabilities_are_unimplemented_regardless_of_installation() {
        for probe in [installed(), FakeHostProbe::new()] {
            let adapter = CopilotAdapter::new(Arc::new(probe));
            assert_eq!(adapter.capabilities(), HarnessCapabilities::default());
        }
    }

    #[test]
    fn launch_is_rejected_while_unimplemented() {
        let result = CopilotAdapter::new(Arc::new(installed())).build_launch_spec(LaunchRequest {
            project_id: None,
            cwd: String::new(),
            args: vec![],
            runtime_target_id: "local".into(),
        });
        assert!(
            matches!(result, Err(Error::InvalidInput(message)) if message.contains("尚未实现"))
        );
    }

    #[test]
    fn summary_serialization_matches_the_shared_ipc_fixture() {
        let mut registry = HarnessRegistry::new();
        registry.register(Box::new(CopilotAdapter::new(Arc::new(installed()))));
        let summary = registry.summaries("local").remove(0);
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/copilot/summary.json"))
                .unwrap();
        assert_eq!(serde_json::to_value(summary).unwrap(), fixture);
    }
}
