//! Codex CLI 适配器。
//!
//! 只负责「检测 + 能力声明」（生命周期方法在 PTY 接线后实现）。
//! 日志解析属于 `crate::usage`，不放在这里（ADR-0002）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::harness::adapter::{
    DetectResult, HarnessAdapter, HarnessCapabilities, HarnessId, LaunchRequest, ProcessHandle,
    ResumeRequest,
};
use crate::harness::probe::HostProbe;

/// 规范 Harness id，与 `harnesses.id` 一致。
pub const CODEX_ID: &str = "codex";

/// 展示名。由适配器自己声明（Registry 只透传），不在别处再维护一份映射。
pub const DISPLAY_NAME: &str = "Codex";

const EXECUTABLE_NAME: &str = "codex";

/// Codex 的数据目录候选。
///
/// 本机实测：Windows 与 Linux 都是 `~/.codex`（内含 `sessions/`、`history.jsonl`、
/// `session_index.jsonl`）。注意 `~/.codex/version.json` 里的 `latest_version` 是
/// **上游最新可用版本**，不是本机已安装版本，因此不能作为 `version` 的来源。
pub fn codex_data_dir_candidates(home: &Path) -> Vec<PathBuf> {
    vec![home.join(".codex")]
}

pub struct CodexAdapter {
    probe: Arc<dyn HostProbe>,
}

impl CodexAdapter {
    pub fn new(probe: Arc<dyn HostProbe>) -> Self {
        Self { probe }
    }

    /// 只返回**确实存在**的数据目录，避免凭空声称。
    fn existing_data_dirs(&self) -> Vec<String> {
        let Some(home) = self.probe.home_dir() else {
            return Vec::new();
        };

        codex_data_dir_candidates(&home)
            .into_iter()
            .filter(|path| self.probe.dir_exists(path))
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    }
}

impl HarnessAdapter for CodexAdapter {
    fn id(&self) -> HarnessId {
        HarnessId::from(CODEX_ID)
    }

    fn display_name(&self) -> &str {
        DISPLAY_NAME
    }

    fn detect(&self) -> DetectResult {
        let binary = self.probe.find_executable(EXECUTABLE_NAME);
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
        match self.probe.find_executable(EXECUTABLE_NAME) {
            Some(path) => self.probe.read_version(&path),
            None => Ok(None),
        }
    }

    /// **全部为 `false`**：因为 PTY / launch / resume / usage **都还没有实现**。
    ///
    /// 语义提醒（docs/adr/0005）：本方法回答「Adapter 实现了没有」，不是
    /// 「此刻能否运行」。所以 Task 4 实现 launch 之后就要在这里置 `true`，
    /// 而那之后**不会**因为某台机器上 binary 缺失、auth 过期导致某次 `launch()` 失败
    /// 就把它改回 `false` —— 那是 readiness 维度（ready / blocked / unknown，待实现）。
    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities::default()
    }

    fn launch(&self, _request: LaunchRequest) -> Result<ProcessHandle> {
        Err(Error::InvalidInput(
            "Codex 尚未接入 PTY 启动（见 docs/plans/2026-09-21-v0.1-walking-skeleton.md Task 4）"
                .to_string(),
        ))
    }

    fn resume(&self, _request: ResumeRequest) -> Result<ProcessHandle> {
        Err(Error::InvalidInput(
            "Codex resume 尚未接入 PTY（见 docs/plans/2026-09-21-v0.1-walking-skeleton.md Task 4）"
                .to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::adapter::HarnessAdapter;
    use crate::harness::probe::parse_version;
    use std::collections::{HashMap, HashSet};
    use std::path::{Path, PathBuf};

    /// 可编程的假宿主：不碰真实文件系统，也不起真实进程。
    struct FakeHostProbe {
        executables: HashMap<String, PathBuf>,
        versions: HashMap<PathBuf, String>,
        dirs: HashSet<PathBuf>,
        home: Option<PathBuf>,
    }

    impl FakeHostProbe {
        fn new() -> Self {
            Self {
                executables: HashMap::new(),
                versions: HashMap::new(),
                dirs: HashSet::new(),
                home: Some(PathBuf::from("/home/dev")),
            }
        }

        fn with_binary(mut self, name: &str, path: &str, version_output: &str) -> Self {
            let path = PathBuf::from(path);
            self.executables.insert(name.to_string(), path.clone());
            self.versions.insert(path, version_output.to_string());
            self
        }

        fn with_dir(mut self, path: &str) -> Self {
            self.dirs.insert(PathBuf::from(path));
            self
        }
    }

    impl HostProbe for FakeHostProbe {
        fn find_executable(&self, name: &str) -> Option<PathBuf> {
            self.executables.get(name).cloned()
        }

        fn read_version(&self, executable: &Path) -> Result<Option<String>> {
            Ok(self
                .versions
                .get(executable)
                .and_then(|raw| parse_version(raw)))
        }

        fn dir_exists(&self, path: &Path) -> bool {
            self.dirs.contains(path)
        }

        fn home_dir(&self) -> Option<PathBuf> {
            self.home.clone()
        }
    }

    fn adapter(probe: FakeHostProbe) -> CodexAdapter {
        CodexAdapter::new(std::sync::Arc::new(probe))
    }

    #[test]
    fn reports_not_installed_when_binary_missing() {
        let result = adapter(FakeHostProbe::new()).detect();

        assert!(!result.installed);
        assert!(result.binary_path.is_none());
        assert!(result.version.is_none());
    }

    #[test]
    fn reports_binary_path_and_parsed_version_when_installed() {
        let probe = FakeHostProbe::new().with_binary(
            "codex",
            "D:/npm-global/codex.cmd",
            "codex-cli 0.152.1",
        );

        let result = adapter(probe).detect();

        assert!(result.installed);
        assert_eq!(
            result.binary_path.as_deref(),
            Some("D:/npm-global/codex.cmd")
        );
        assert_eq!(result.version.as_deref(), Some("0.152.1"));
    }

    #[test]
    fn only_reports_data_dirs_that_actually_exist() {
        let probe = FakeHostProbe::new()
            .with_binary("codex", "D:/npm-global/codex.cmd", "codex-cli 0.152.1")
            .with_dir("/home/dev/.codex");

        let result = adapter(probe).detect();

        // 用 PathBuf 构造期望值：Windows 上 join 产生 `\`，硬编码 `/` 会误报失败。
        let expected = PathBuf::from("/home/dev").join(".codex");
        assert_eq!(
            result.data_paths,
            vec![expected.to_string_lossy().into_owned()]
        );
    }

    #[test]
    fn omits_data_dirs_that_do_not_exist() {
        let probe = FakeHostProbe::new().with_binary(
            "codex",
            "D:/npm-global/codex.cmd",
            "codex-cli 0.152.1",
        );

        let result = adapter(probe).detect();

        assert!(result.data_paths.is_empty(), "不存在的目录不得出现在结果里");
    }

    #[test]
    fn stays_installed_even_when_version_output_is_unparsable() {
        let probe = FakeHostProbe::new().with_binary("codex", "D:/npm-global/codex.cmd", "");

        let result = adapter(probe).detect();

        assert!(result.installed, "读不到版本不等于没安装");
        assert!(result.version.is_none());
    }

    #[test]
    fn adapter_id_is_codex() {
        assert_eq!(adapter(FakeHostProbe::new()).id().as_str(), "codex");
    }

    #[test]
    fn no_capability_is_claimed_before_its_task_lands() {
        let capabilities = adapter(FakeHostProbe::new()).capabilities();

        assert_eq!(
            capabilities,
            HarnessCapabilities::default(),
            "PTY / usage / replay 都还没实现，任何能力为 true 都是不诚实的宣称"
        );
    }

    /// 当前 launch 仍是 stub，因此 `capabilities.launch` 必须是 `false`。
    ///
    /// 刻意**不**建立「capabilities.launch == 某次 launch() 是否成功」这种长期契约：
    /// capability 表示 Adapter 是否实现该能力，运行时成败属于 readiness
    /// （见 docs/adr/0005-capability-vs-readiness.md）。Task 4 实现 launch 后，
    /// 这里会改成断言 `launch == true`，而 launch() 依然可能因为 binary 缺失、
    /// auth 过期等原因失败 —— 那不影响 capability。
    #[test]
    fn launch_capability_is_false_while_launch_is_still_a_stub() {
        let codex = adapter(FakeHostProbe::new());

        assert!(
            !codex.capabilities().launch,
            "launch 还没实现，capability 就不能为 true"
        );
        assert!(
            codex
                .launch(crate::harness::adapter::LaunchRequest {
                    project_id: None,
                    cwd: "D:/work".to_string(),
                    args: Vec::new(),
                    runtime_target_id: "local".to_string(),
                })
                .is_err(),
            "配套前提：此时 launch() 确实还是 stub"
        );
    }

    #[test]
    fn launch_reports_a_clear_error_before_pty_wiring() {
        use crate::harness::adapter::LaunchRequest;

        let request = LaunchRequest {
            project_id: None,
            cwd: "D:/work".to_string(),
            args: Vec::new(),
            runtime_target_id: "local".to_string(),
        };

        let error = adapter(FakeHostProbe::new())
            .launch(request)
            .expect_err("PTY 接线完成前必须失败");

        assert!(
            error.to_string().contains("PTY"),
            "错误信息要指出缺失的能力：{error}"
        );
    }

    #[test]
    fn resume_reports_a_clear_error_before_pty_wiring() {
        use crate::harness::adapter::ResumeRequest;

        let request = ResumeRequest {
            hub_session_id: "hub-1".to_string(),
            source_session_id: None,
            cwd: "D:/work".to_string(),
            runtime_target_id: "local".to_string(),
        };

        let error = adapter(FakeHostProbe::new())
            .resume(request)
            .expect_err("PTY 接线完成前必须失败");

        assert!(error.to_string().contains("resume"));
    }
}
