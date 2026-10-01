//! Gemini CLI 的检测阶段：只探测 binary、版本与现存数据目录。
//!
//! launch / terminal / usage 尚未实现，能力矩阵保持全 false。
//! 不读取用户配置、认证或 Session 内容；环境配置根由组合根显式注入。

use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::harness::adapter::{
    DetectResult, HarnessAdapter, HarnessCapabilities, HarnessId, LaunchRequest,
};
use crate::harness::launch::LaunchSpec;
use crate::harness::probe::HostProbe;

pub const GEMINI_ID: &str = "gemini";

pub struct GeminiCliAdapter {
    probe: Arc<dyn HostProbe>,
    config_home: Option<PathBuf>,
}

impl GeminiCliAdapter {
    /// `config_home` 对应 GEMINI_CLI_HOME；CLI 使用该根目录下的 `.gemini`。
    pub fn new(probe: Arc<dyn HostProbe>, config_home: Option<PathBuf>) -> Self {
        Self { probe, config_home }
    }

    fn existing_data_dirs(&self) -> Vec<String> {
        self.config_home
            .clone()
            .or_else(|| self.probe.home_dir())
            .map(|root| root.join(".gemini"))
            .filter(|path| self.probe.dir_exists(path))
            .map(|path| vec![path.to_string_lossy().into_owned()])
            .unwrap_or_default()
    }
}

impl HarnessAdapter for GeminiCliAdapter {
    fn id(&self) -> HarnessId {
        HarnessId::from(GEMINI_ID)
    }

    fn display_name(&self) -> &str {
        "Gemini CLI"
    }

    fn detect(&self) -> DetectResult {
        let binary = self.probe.find_executable(GEMINI_ID);
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
        match self.probe.find_executable(GEMINI_ID) {
            Some(path) => self.probe.read_version(&path),
            None => Ok(None),
        }
    }

    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities::default()
    }

    fn build_launch_spec(&self, _request: LaunchRequest) -> Result<LaunchSpec> {
        Err(Error::InvalidInput(
            "Gemini CLI 启动尚未实现：当前仅支持安装检测".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::harness::adapter::{HarnessAdapter, HarnessCapabilities, LaunchRequest};
    use crate::harness::registry::HarnessRegistry;
    use crate::test_support::FakeHostProbe;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn installed() -> FakeHostProbe {
        FakeHostProbe::new().with_binary(
            "gemini",
            "D:/npm-global/gemini.cmd",
            include_str!("../../../../fixtures/gemini/version.txt"),
        )
    }

    #[test]
    fn detects_a_binary_and_parses_the_real_version_fixture() {
        let adapter = GeminiCliAdapter::new(Arc::new(installed()), None);
        let detected = adapter.detect();
        assert!(detected.installed);
        assert_eq!(
            detected.binary_path.as_deref(),
            Some("D:/npm-global/gemini.cmd")
        );
        assert_eq!(detected.version.as_deref(), Some("0.59.0"));
        assert_eq!(adapter.version().unwrap().as_deref(), Some("0.59.0"));
    }

    #[test]
    fn missing_binary_is_uninstalled_even_if_data_remains() {
        let adapter = GeminiCliAdapter::new(
            Arc::new(FakeHostProbe::new().with_dir("/home/dev/.gemini")),
            None,
        );
        let detected = adapter.detect();
        assert!(!detected.installed);
        assert_eq!(detected.binary_path, None);
        assert_eq!(detected.version, None);
        assert_eq!(adapter.version().unwrap(), None);
        assert_eq!(detected.data_paths.len(), 1);
    }

    #[test]
    fn unreadable_version_does_not_hide_an_installed_binary() {
        let adapter = GeminiCliAdapter::new(
            Arc::new(FakeHostProbe::new().with_binary(
                "gemini",
                "D:/npm-global/gemini.cmd",
                "unknown",
            )),
            None,
        );
        let detected = adapter.detect();
        assert!(detected.installed);
        assert_eq!(detected.version, None);
    }

    #[test]
    fn version_io_failure_and_missing_home_are_safe_unknowns() {
        struct FailingProbe;
        impl HostProbe for FailingProbe {
            fn find_executable(&self, _: &str) -> Option<PathBuf> {
                Some(PathBuf::from("D:/npm-global/gemini.cmd"))
            }
            fn read_version(&self, _: &std::path::Path) -> Result<Option<String>> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "version probe denied",
                )
                .into())
            }
            fn dir_exists(&self, _: &std::path::Path) -> bool {
                true
            }
            fn home_dir(&self) -> Option<PathBuf> {
                None
            }
        }
        let adapter = GeminiCliAdapter::new(Arc::new(FailingProbe), None);
        let detected = adapter.detect();
        assert!(detected.installed);
        assert_eq!(detected.version, None);
        assert!(detected.data_paths.is_empty());
        assert!(matches!(adapter.version(), Err(Error::Io(_))));
        let isolated =
            GeminiCliAdapter::new(Arc::new(FailingProbe), Some(PathBuf::from("/isolated")))
                .detect();
        assert_eq!(
            isolated.data_paths,
            vec![PathBuf::from("/isolated")
                .join(".gemini")
                .to_string_lossy()
                .into_owned()]
        );
    }

    #[test]
    fn reports_only_existing_default_data_directories() {
        let absent = GeminiCliAdapter::new(Arc::new(installed()), None).detect();
        assert!(absent.data_paths.is_empty());
        let present =
            GeminiCliAdapter::new(Arc::new(installed().with_dir("/home/dev/.gemini")), None)
                .detect();
        assert_eq!(
            present.data_paths,
            vec![PathBuf::from("/home/dev")
                .join(".gemini")
                .to_string_lossy()
                .into_owned()]
        );
    }

    #[test]
    fn configuration_home_is_a_root_and_does_not_fall_back_to_default_data() {
        let probe = installed()
            .with_dir("/home/dev/.gemini")
            .with_dir("/isolated/.gemini");
        let detected =
            GeminiCliAdapter::new(Arc::new(probe), Some(PathBuf::from("/isolated"))).detect();
        assert_eq!(
            detected.data_paths,
            vec![PathBuf::from("/isolated")
                .join(".gemini")
                .to_string_lossy()
                .into_owned()]
        );
        let missing_override = GeminiCliAdapter::new(
            Arc::new(installed().with_dir("/home/dev/.gemini")),
            Some(PathBuf::from("/missing")),
        )
        .detect();
        assert!(missing_override.data_paths.is_empty());
    }

    #[test]
    fn capabilities_are_unimplemented_regardless_of_installation() {
        for probe in [installed(), FakeHostProbe::new()] {
            let adapter = GeminiCliAdapter::new(Arc::new(probe), None);
            assert_eq!(adapter.capabilities(), HarnessCapabilities::default());
            let result = adapter.build_launch_spec(LaunchRequest {
                project_id: None,
                cwd: String::new(),
                args: vec![],
                runtime_target_id: "local".into(),
            });
            assert!(
                matches!(result, Err(Error::InvalidInput(message)) if message.contains("尚未实现"))
            );
        }
    }

    #[test]
    fn summary_serialization_matches_the_shared_ipc_fixture() {
        let mut registry = HarnessRegistry::new();
        registry.register(Box::new(GeminiCliAdapter::new(Arc::new(installed()), None)));
        let summary = registry.summaries("local").remove(0);
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/gemini/summary.json")).unwrap();
        assert_eq!(serde_json::to_value(summary).unwrap(), fixture);
    }
}
