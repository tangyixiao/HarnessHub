//! Codex CLI 适配器。
//!
//! 只负责「检测 + 能力声明」（生命周期方法在 PTY 接线后实现）。
//! 日志解析属于 `crate::usage`，不放在这里（ADR-0002）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::harness::adapter::{
    DetectResult, HarnessAdapter, HarnessCapabilities, HarnessId, LaunchRequest,
};
use crate::harness::launch::{program_for, LaunchSpec};
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

    /// `launch` 已翻为 `true`：真实 installation → LaunchSpec → spawn → 生命周期
    /// 已在本机端到端验收通过（真机 Codex 跑出 TUI、正常退出 / 用户 kill /
    /// host_shutdown / lost 都落到正确终态）。
    ///
    /// `terminal` 仍为 `false`：GUI 侧的 resize 观测还差一次验收，
    /// 翻它之前不能宣称「交互式终端」这一整项能力。
    /// `usage` / `replay` 等仍未实现。
    ///
    /// 语义提醒（docs/adr/0005）：capability 回答「Adapter 实现了没有」，
    /// 不因某台机器上 binary 缺失或 auth 过期而回退。
    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            launch: true,
            ..HarnessCapabilities::default()
        }
    }

    fn build_launch_spec(&self, request: LaunchRequest) -> Result<LaunchSpec> {
        let binary = self.probe.find_executable(EXECUTABLE_NAME).ok_or_else(|| {
            Error::InvalidInput("Codex 未安装：PATH 中找不到可执行文件".to_string())
        })?;

        // 平台差异只在这里解决：Windows 的 npm shim（.cmd 直接执行、.ps1 交给 pwsh）。
        let (program, mut args) = program_for(&binary);
        args.extend(request.args);

        let cwd = if request.cwd.is_empty() {
            None
        } else {
            Some(PathBuf::from(request.cwd))
        };

        Ok(LaunchSpec {
            program,
            args,
            cwd,
            env: Vec::new(),
            runtime_target_id: request.runtime_target_id,
        })
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
    fn only_verified_capabilities_are_claimed() {
        let capabilities = adapter(FakeHostProbe::new()).capabilities();

        assert!(capabilities.launch, "launch 已通过真机端到端验收");
        for (name, value) in [
            ("terminal", capabilities.terminal),
            ("resume", capabilities.resume),
            ("usage", capabilities.usage),
            ("replay", capabilities.replay),
            ("tool_calls", capabilities.tool_calls),
            ("subagents", capabilities.subagents),
            ("live_state", capabilities.live_state),
            ("worktree", capabilities.worktree),
        ] {
            assert!(!value, "{name} 尚未验收，不得为 true");
        }
    }

    /// `build_launch_spec` 只是启动路径的一半，因此 `capabilities.launch` 仍为
    /// `false`（另一半是 Task 4 的 PTY spawn 与端到端验收）。
    ///
    /// 刻意**不**建立「capabilities.launch == 某次启动是否成功」这种长期契约：
    /// capability 表示 Adapter 是否实现该能力，运行时成败属于 readiness
    /// （见 docs/adr/0005-capability-vs-readiness.md）。
    #[test]
    fn launch_is_claimed_only_after_the_real_launch_path_passed_acceptance() {
        let codex = adapter(FakeHostProbe::new());

        assert!(
            codex.capabilities().launch,
            "真实 installation → LaunchSpec → spawn → 生命周期已验收，launch 应为 true"
        );
        assert!(
            codex.build_launch_spec(launch_request(&[])).is_err(),
            "配套前提：没装 Codex 时仍生成不出 launch spec（运行时失败属于 readiness）"
        );
    }

    /// `terminal` 要等 GUI 侧 resize 也验收通过才翻。
    #[test]
    fn terminal_capability_waits_for_the_gui_resize_acceptance() {
        assert!(
            !adapter(FakeHostProbe::new()).capabilities().terminal,
            "GUI 交互式终端还差 resize 观测，不得提前打勾"
        );
    }

    // ---- LaunchSpec（Task 4 的接口冻结，本期只定义边界） ----

    fn launch_request(extra_args: &[&str]) -> crate::harness::adapter::LaunchRequest {
        crate::harness::adapter::LaunchRequest {
            project_id: None,
            cwd: "D:/work".to_string(),
            args: extra_args.iter().map(|arg| arg.to_string()).collect(),
            runtime_target_id: "local".to_string(),
        }
    }

    #[test]
    fn launch_spec_is_rejected_when_codex_is_not_installed() {
        let error = adapter(FakeHostProbe::new())
            .build_launch_spec(launch_request(&[]))
            .expect_err("未安装必须失败");

        assert!(
            error.to_string().contains("未安装"),
            "错误信息要说明原因：{error}"
        );
    }

    /// Windows 上 npm 的 `.cmd` shim：直接执行，不套 shell。
    #[test]
    fn launch_spec_executes_cmd_shim_directly() {
        let probe = FakeHostProbe::new().with_binary(
            "codex",
            "D:/npm-global/codex.cmd",
            "codex-cli 0.152.1",
        );

        let spec = adapter(probe)
            .build_launch_spec(launch_request(&["--model", "gpt-5"]))
            .expect("生成 spec");

        assert_eq!(
            spec.program,
            PathBuf::from("D:/npm-global/codex.cmd"),
            "必须是可执行文件本身，不得退化成 cmd /c 之类的外壳"
        );
        assert_eq!(spec.args, vec!["--model".to_string(), "gpt-5".to_string()]);
        assert_eq!(spec.cwd, Some(PathBuf::from("D:/work")));
        assert_eq!(spec.runtime_target_id, "local");
    }

    /// 只有 `.ps1` 时不能被 CreateProcess 执行，必须交给 pwsh。
    #[test]
    fn launch_spec_hosts_ps1_shim_with_powershell() {
        let probe = FakeHostProbe::new().with_binary(
            "codex",
            "D:/npm-global/codex.ps1",
            "codex-cli 0.152.1",
        );

        let spec = adapter(probe)
            .build_launch_spec(launch_request(&["--version"]))
            .expect("生成 spec");

        assert_eq!(spec.program, PathBuf::from("pwsh"));
        assert_eq!(
            spec.args,
            vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-File".to_string(),
                "D:/npm-global/codex.ps1".to_string(),
                "--version".to_string(),
            ],
            "用户参数必须排在 shim 前置参数之后"
        );
    }

    #[test]
    fn launch_spec_omits_empty_cwd_instead_of_passing_an_empty_string() {
        let probe = FakeHostProbe::new().with_binary(
            "codex",
            "D:/npm-global/codex.cmd",
            "codex-cli 0.152.1",
        );
        let mut request = launch_request(&[]);
        request.cwd = String::new();

        let spec = adapter(probe)
            .build_launch_spec(request)
            .expect("生成 spec");

        assert!(spec.cwd.is_none(), "空 cwd 应视为「未指定」");
    }

    #[test]
    fn launch_spec_carries_the_runtime_target() {
        let probe = FakeHostProbe::new().with_binary(
            "codex",
            "D:/npm-global/codex.cmd",
            "codex-cli 0.152.1",
        );
        let mut request = launch_request(&[]);
        request.runtime_target_id = "wsl-ubuntu".to_string();

        let spec = adapter(probe)
            .build_launch_spec(request)
            .expect("生成 spec");

        assert_eq!(spec.runtime_target_id, "wsl-ubuntu");
    }
}
