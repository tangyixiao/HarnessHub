//! OpenCode CLI（sst/opencode）的检测阶段：只探测 binary、版本与现存数据目录。
//!
//! launch / terminal / usage 尚未实现，能力矩阵保持全 false（ADR-0005 零宣称）；
//! Terminal 的能力过滤会自动不提供启动入口。不读取配置、认证、Session 或日志内容。
//!
//! OpenCode 遵循 XDG Base Directory 规范，四个根**互相独立**，可分别被
//! `XDG_DATA_HOME` / `XDG_CONFIG_HOME` / `XDG_STATE_HOME` / `XDG_CACHE_HOME`
//! 覆盖；未设置时各自回退到默认值（Windows 上同样是 Unix 风格默认根，
//! 这是本机 `opencode debug paths` 的实测输出，不是猜测）。
//!
//! **刻意不 shell out 到 `opencode debug paths`**：`HostProbe` 只承诺
//! `--version` 这一条子命令；为了目录发现再多跑一条子命令等于新增一层探测抽象，
//! 而 XDG 解析规则已经实测可复现，可以在测试里离线锁定。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::harness::adapter::{
    DetectResult, HarnessAdapter, HarnessCapabilities, HarnessId, LaunchRequest,
};
use crate::harness::launch::LaunchSpec;
use crate::harness::probe::HostProbe;

pub const OPENCODE_ID: &str = "opencode";

/// `opencode debug paths` 里各根下固定的子目录名。
const OPENCODE_DIR_NAME: &str = "opencode";

/// 四个 XDG 根的可选覆盖，**只在组合根读取一次环境**后注入适配器，
/// 这样单测完全不依赖真实环境（与 Gemini 的 `GEMINI_CLI_HOME` 同构）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenCodeXdgRoots {
    pub data_home: Option<PathBuf>,
    pub config_home: Option<PathBuf>,
    pub state_home: Option<PathBuf>,
    pub cache_home: Option<PathBuf>,
}

impl OpenCodeXdgRoots {
    /// 从环境读取四个 XDG 覆盖；空字符串视为未设置（与 PATH 类变量的既有处理一致）。
    pub fn from_env() -> Self {
        Self {
            data_home: non_empty_env("XDG_DATA_HOME"),
            config_home: non_empty_env("XDG_CONFIG_HOME"),
            state_home: non_empty_env("XDG_STATE_HOME"),
            cache_home: non_empty_env("XDG_CACHE_HOME"),
        }
    }
}

fn non_empty_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub struct OpenCodeAdapter {
    probe: Arc<dyn HostProbe>,
    roots: OpenCodeXdgRoots,
}

impl OpenCodeAdapter {
    pub fn new(probe: Arc<dyn HostProbe>, roots: OpenCodeXdgRoots) -> Self {
        Self { probe, roots }
    }

    /// 解析一个 XDG 根：覆盖优先，否则按默认相对段挂在 home 下。
    /// home 未知且没有覆盖时返回 `None` —— 不凭空猜目录。
    fn resolve_root(
        override_root: Option<&PathBuf>,
        home: Option<&Path>,
        default_segments: &[&str],
    ) -> Option<PathBuf> {
        match override_root {
            Some(root) => Some(root.clone()),
            None => home.map(|home| {
                default_segments
                    .iter()
                    .fold(home.to_path_buf(), |path, segment| path.join(segment))
            }),
        }
    }

    /// 只返回**确实存在**的数据目录，顺序固定为 data / config / state / cache。
    fn existing_data_dirs(&self) -> Vec<String> {
        let home = self.probe.home_dir();
        let roots = [
            Self::resolve_root(
                self.roots.data_home.as_ref(),
                home.as_deref(),
                &[".local", "share"],
            ),
            Self::resolve_root(
                self.roots.config_home.as_ref(),
                home.as_deref(),
                &[".config"],
            ),
            Self::resolve_root(
                self.roots.state_home.as_ref(),
                home.as_deref(),
                &[".local", "state"],
            ),
            Self::resolve_root(self.roots.cache_home.as_ref(), home.as_deref(), &[".cache"]),
        ];

        roots
            .into_iter()
            .flatten()
            .map(|root| root.join(OPENCODE_DIR_NAME))
            .filter(|path| self.probe.dir_exists(path))
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    }
}

impl HarnessAdapter for OpenCodeAdapter {
    fn id(&self) -> HarnessId {
        HarnessId::from(OPENCODE_ID)
    }

    fn display_name(&self) -> &str {
        "OpenCode"
    }

    /// `installed` 只由 binary 是否存在决定；版本探测失败只是 `version = None`。
    fn detect(&self) -> DetectResult {
        let binary = self.probe.find_executable(OPENCODE_ID);
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
        match self.probe.find_executable(OPENCODE_ID) {
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
            "OpenCode 启动尚未实现：当前仅支持安装检测".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::adapter::{HarnessAdapter, HarnessCapabilities, LaunchRequest};
    use crate::harness::registry::HarnessRegistry;
    use crate::test_support::FakeHostProbe;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    const BINARY: &str = "D:/Scoop/shims/opencode.exe";
    const HOME: &str = "/home/dev";

    fn installed() -> FakeHostProbe {
        FakeHostProbe::new().with_binary(
            OPENCODE_ID,
            BINARY,
            include_str!("../../../../fixtures/opencode/version.txt"),
        )
    }

    fn joined(root: &str, segments: &[&str]) -> String {
        segments
            .iter()
            .fold(PathBuf::from(root), |path, segment| path.join(segment))
            .to_string_lossy()
            .into_owned()
    }

    /// 默认根（home 已设置、无覆盖）下四个目录的期望路径。
    fn default_root(segments: &[&str]) -> String {
        joined(HOME, segments)
    }

    /// home 未知但 `dir_exists` 恒真的探针：用来证明「不凭空猜目录」，
    /// 且覆盖根不依赖 home。
    struct HomelessProbe;

    impl HostProbe for HomelessProbe {
        fn find_executable(&self, name: &str) -> Option<PathBuf> {
            (name == OPENCODE_ID).then(|| PathBuf::from(BINARY))
        }

        fn read_version(&self, _executable: &Path) -> Result<Option<String>> {
            Ok(Some("1.18.23".to_string()))
        }

        fn dir_exists(&self, _path: &Path) -> bool {
            true
        }

        fn home_dir(&self) -> Option<PathBuf> {
            None
        }
    }

    #[test]
    fn detects_a_binary_and_parses_the_real_version_fixture() {
        let adapter = OpenCodeAdapter::new(Arc::new(installed()), OpenCodeXdgRoots::default());
        let detected = adapter.detect();
        assert!(detected.installed);
        assert_eq!(detected.binary_path.as_deref(), Some(BINARY));
        assert_eq!(detected.version.as_deref(), Some("1.18.23"));
        assert_eq!(adapter.version().unwrap().as_deref(), Some("1.18.23"));
    }

    #[test]
    fn missing_binary_is_uninstalled_even_if_data_remains() {
        let probe = FakeHostProbe::new().with_dir(&default_root(&[".local", "share", "opencode"]));
        let adapter = OpenCodeAdapter::new(Arc::new(probe), OpenCodeXdgRoots::default());
        let detected = adapter.detect();
        assert!(!detected.installed);
        assert_eq!(detected.binary_path, None);
        assert_eq!(detected.version, None);
        assert_eq!(adapter.version().unwrap(), None);
        assert_eq!(detected.data_paths.len(), 1);
    }

    #[test]
    fn unreadable_version_does_not_hide_an_installed_binary() {
        let probe = FakeHostProbe::new().with_binary(OPENCODE_ID, BINARY, "unknown");
        let adapter = OpenCodeAdapter::new(Arc::new(probe), OpenCodeXdgRoots::default());
        let detected = adapter.detect();
        assert!(detected.installed);
        assert_eq!(detected.version, None);
    }

    #[test]
    fn reports_only_existing_xdg_data_directories_in_a_stable_order() {
        let absent = OpenCodeAdapter::new(Arc::new(installed()), OpenCodeXdgRoots::default());
        assert!(absent.detect().data_paths.is_empty(), "都不存在时不得列出");

        let probe = installed()
            .with_dir(&default_root(&[".cache", "opencode"]))
            .with_dir(&default_root(&[".local", "state", "opencode"]))
            .with_dir(&default_root(&[".config", "opencode"]))
            .with_dir(&default_root(&[".local", "share", "opencode"]));
        let adapter = OpenCodeAdapter::new(Arc::new(probe), OpenCodeXdgRoots::default());
        assert_eq!(
            adapter.detect().data_paths,
            vec![
                default_root(&[".local", "share", "opencode"]),
                default_root(&[".config", "opencode"]),
                default_root(&[".local", "state", "opencode"]),
                default_root(&[".cache", "opencode"]),
            ],
            "顺序固定为 data / config / state / cache"
        );
    }

    #[test]
    fn missing_home_without_overrides_yields_no_data_paths() {
        let detected =
            OpenCodeAdapter::new(Arc::new(HomelessProbe), OpenCodeXdgRoots::default()).detect();
        assert!(detected.installed);
        assert!(
            detected.data_paths.is_empty(),
            "home 与 XDG 都未知时不得凭空列出目录"
        );
    }

    #[test]
    fn an_explicit_xdg_root_is_listed_even_when_home_is_unknown() {
        let roots = OpenCodeXdgRoots {
            data_home: Some(PathBuf::from("D:/xdg-data")),
            ..OpenCodeXdgRoots::default()
        };
        let detected = OpenCodeAdapter::new(Arc::new(HomelessProbe), roots).detect();
        assert_eq!(
            detected.data_paths,
            vec![joined("D:/xdg-data", &["opencode"])],
            "覆盖是绝对路径，不依赖 home"
        );
    }

    #[test]
    fn xdg_overrides_replace_the_default_roots_and_do_not_fall_back() {
        let roots = OpenCodeXdgRoots {
            data_home: Some(PathBuf::from("D:/xdg-data")),
            config_home: Some(PathBuf::from("D:/xdg-config")),
            state_home: Some(PathBuf::from("D:/xdg-state")),
            cache_home: Some(PathBuf::from("D:/xdg-cache")),
        };
        // 默认根**同时存在**也不得回退：覆盖即权威。
        let probe = installed()
            .with_dir(&default_root(&[".local", "share", "opencode"]))
            .with_dir(&default_root(&[".config", "opencode"]))
            .with_dir(&default_root(&[".local", "state", "opencode"]))
            .with_dir(&default_root(&[".cache", "opencode"]))
            .with_dir(&joined("D:/xdg-data", &["opencode"]))
            .with_dir(&joined("D:/xdg-config", &["opencode"]))
            .with_dir(&joined("D:/xdg-state", &["opencode"]))
            .with_dir(&joined("D:/xdg-cache", &["opencode"]));
        let adapter = OpenCodeAdapter::new(Arc::new(probe), roots);
        assert_eq!(
            adapter.detect().data_paths,
            vec![
                joined("D:/xdg-data", &["opencode"]),
                joined("D:/xdg-config", &["opencode"]),
                joined("D:/xdg-state", &["opencode"]),
                joined("D:/xdg-cache", &["opencode"]),
            ]
        );
    }

    #[test]
    fn each_xdg_root_is_independent() {
        let roots = OpenCodeXdgRoots {
            data_home: Some(PathBuf::from("D:/xdg-data")),
            ..OpenCodeXdgRoots::default()
        };
        let probe = installed()
            .with_dir(&default_root(&[".local", "share", "opencode"]))
            .with_dir(&default_root(&[".config", "opencode"]))
            .with_dir(&joined("D:/xdg-data", &["opencode"]));
        let adapter = OpenCodeAdapter::new(Arc::new(probe), roots);
        assert_eq!(
            adapter.detect().data_paths,
            vec![
                joined("D:/xdg-data", &["opencode"]),
                default_root(&[".config", "opencode"]),
            ],
            "只覆盖 data 时其余根仍走默认值"
        );
    }

    #[test]
    fn capabilities_are_unimplemented_regardless_of_installation() {
        for probe in [installed(), FakeHostProbe::new()] {
            let adapter = OpenCodeAdapter::new(Arc::new(probe), OpenCodeXdgRoots::default());
            assert_eq!(adapter.capabilities(), HarnessCapabilities::default());
        }
    }

    #[test]
    fn launch_is_rejected_while_unimplemented() {
        let result = OpenCodeAdapter::new(Arc::new(installed()), OpenCodeXdgRoots::default())
            .build_launch_spec(LaunchRequest {
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
        registry.register(Box::new(OpenCodeAdapter::new(
            Arc::new(installed()),
            OpenCodeXdgRoots::default(),
        )));
        let summary = registry.summaries("local").remove(0);
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/opencode/summary.json"))
                .unwrap();
        assert_eq!(serde_json::to_value(summary).unwrap(), fixture);
    }
}
