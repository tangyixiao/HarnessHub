//! CodeBuddy CLI（腾讯 @tencent-ai/codebuddy-code）的检测阶段：只探测 binary、
//! 版本与现存数据目录。
//!
//! launch / terminal / usage 尚未实现，能力矩阵保持全 false；Terminal 的能力
//! 过滤会自动不提供启动入口。不读取用户配置、认证或 Session 内容。
//! 与 Gemini 不同：未证实官方支持 home 覆盖 env，因此**不做**目录覆盖注入，
//! 只发现 home 下 `.codebuddy`。
//!
//! 事实依据（2026-10-11）：npm 包 `@tencent-ai/codebuddy-code`，PATH shim 为
//! `.cmd` / `.ps1` / 无扩展名三件套（与 Codex / Claude / Gemini 同构），
//! `codebuddy --version` 输出 `2.161.0`，数据目录 `~/.codebuddy`。
//! 详见 docs/specs/2026-10-11-codebuddy-detection-design.md。

use std::sync::Arc;

use crate::error::{Error, Result};
use crate::harness::adapter::{
    DetectResult, HarnessAdapter, HarnessCapabilities, HarnessId, LaunchRequest,
};
use crate::harness::launch::LaunchSpec;
use crate::harness::probe::HostProbe;

pub const CODEBUDDY_ID: &str = "codebuddy";

pub struct CodeBuddyAdapter {
    probe: Arc<dyn HostProbe>,
}

impl CodeBuddyAdapter {
    pub fn new(probe: Arc<dyn HostProbe>) -> Self {
        Self { probe }
    }

    /// 只返回**确实存在**的数据目录，避免凭空声称。
    fn existing_data_dirs(&self) -> Vec<String> {
        self.probe
            .home_dir()
            .map(|home| home.join(".codebuddy"))
            .filter(|path| self.probe.dir_exists(path))
            .map(|path| vec![path.to_string_lossy().into_owned()])
            .unwrap_or_default()
    }
}

impl HarnessAdapter for CodeBuddyAdapter {
    fn id(&self) -> HarnessId {
        HarnessId::from(CODEBUDDY_ID)
    }

    fn display_name(&self) -> &str {
        "CodeBuddy"
    }

    /// `installed` 只由 binary 是否存在决定；版本探测失败只是 `version = None`。
    fn detect(&self) -> DetectResult {
        let binary = self.probe.find_executable(CODEBUDDY_ID);
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
        match self.probe.find_executable(CODEBUDDY_ID) {
            Some(path) => self.probe.read_version(&path),
            None => Ok(None),
        }
    }

    /// 检测阶段**一个能力都不宣称**（ADR-0005 零宣称）。
    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities::default()
    }

    fn build_launch_spec(&self, _request: LaunchRequest) -> Result<LaunchSpec> {
        Err(Error::InvalidInput(
            "CodeBuddy 启动尚未实现：当前仅支持安装检测".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::adapter::{HarnessAdapter, HarnessCapabilities, LaunchRequest};
    use crate::harness::registry::HarnessRegistry;
    use crate::test_support::FakeHostProbe;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn installed() -> FakeHostProbe {
        FakeHostProbe::new().with_binary(
            "codebuddy",
            "D:/npm-global/codebuddy.cmd",
            include_str!("../../../../fixtures/codebuddy/version.txt"),
        )
    }

    #[test]
    fn detects_a_binary_and_parses_the_real_version_fixture() {
        let adapter = CodeBuddyAdapter::new(Arc::new(installed()));
        let detected = adapter.detect();
        assert!(detected.installed);
        assert_eq!(
            detected.binary_path.as_deref(),
            Some("D:/npm-global/codebuddy.cmd")
        );
        assert_eq!(detected.version.as_deref(), Some("2.161.0"));
        assert_eq!(adapter.version().unwrap().as_deref(), Some("2.161.0"));
    }

    #[test]
    fn missing_binary_is_uninstalled_even_if_data_remains() {
        let adapter = CodeBuddyAdapter::new(Arc::new(
            FakeHostProbe::new().with_dir("/home/dev/.codebuddy"),
        ));
        let detected = adapter.detect();
        assert!(!detected.installed);
        assert_eq!(detected.binary_path, None);
        assert_eq!(detected.version, None);
        assert_eq!(adapter.version().unwrap(), None);
        assert_eq!(detected.data_paths.len(), 1);
    }

    #[test]
    fn unreadable_version_does_not_hide_an_installed_binary() {
        let adapter = CodeBuddyAdapter::new(Arc::new(FakeHostProbe::new().with_binary(
            "codebuddy",
            "D:/npm-global/codebuddy.cmd",
            "unknown",
        )));
        let detected = adapter.detect();
        assert!(detected.installed);
        assert_eq!(detected.version, None);
    }

    #[test]
    fn reports_only_existing_default_data_directories() {
        let absent = CodeBuddyAdapter::new(Arc::new(installed())).detect();
        assert!(absent.data_paths.is_empty());

        let present =
            CodeBuddyAdapter::new(Arc::new(installed().with_dir("/home/dev/.codebuddy"))).detect();
        assert_eq!(
            present.data_paths,
            vec![PathBuf::from("/home/dev")
                .join(".codebuddy")
                .to_string_lossy()
                .into_owned()]
        );
    }

    #[test]
    fn missing_home_yields_no_data_paths() {
        let detected = CodeBuddyAdapter::new(Arc::new(installed())).detect();
        assert!(detected.installed);
        assert!(detected.data_paths.is_empty(), "home 不存在时不得凭空列出");
    }

    #[test]
    fn capabilities_are_unimplemented_regardless_of_installation() {
        for probe in [installed(), FakeHostProbe::new()] {
            let adapter = CodeBuddyAdapter::new(Arc::new(probe));
            assert_eq!(adapter.capabilities(), HarnessCapabilities::default());
        }
    }

    #[test]
    fn launch_is_rejected_while_unimplemented() {
        let result =
            CodeBuddyAdapter::new(Arc::new(installed())).build_launch_spec(LaunchRequest {
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
        registry.register(Box::new(CodeBuddyAdapter::new(Arc::new(installed()))));
        let summary = registry.summaries("local").remove(0);
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/codebuddy/summary.json"))
                .unwrap();
        assert_eq!(serde_json::to_value(summary).unwrap(), fixture);
    }
}
