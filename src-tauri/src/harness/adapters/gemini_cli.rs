//! Gemini CLI 适配器：检测（2026-10-01 起）+ 终端生命周期（2026-10-11 起）。
//!
//! **零宣称**（ADR-0005）：每个能力都必须先拿到**指定的真机证据**才允许翻 true。
//! 当前 `launch` / `terminal` 为 `true`，证据是：
//!
//! ```text
//! launch     headless 真机：gemini@local → 现有 TerminalRuntime → running + 真实 pid
//!            （tests/real_gemini_terminal.rs [1]；binary 缺失时启动失败不假装 running）
//! terminal   headless 真机：真实 TUI 字节（bracketed paste）、DSR 闭环、方向键 ↓ 产生
//!            1 万+ 字节重绘、resize 不掉线、kill → user_killed + 真实退出码
//!            （real_gemini_terminal [2]-[6]）+ 四条状态链 user_killed / natural_exit
//!            （Ctrl+C×2）/ host_shutdown / lost（tests/gemini_lifecycle.rs）
//! 仍未翻     usage / resume / replay / tool_calls / subagents / live_state / worktree
//! ```
//!
//! 本机认证被 Google 服务端拒绝（「client is no longer supported … migrate to
//! Antigravity」，2026-10-11 实测），chat 级往返证据暂时不可达 —— 按 ADR-0005
//! 这是 **readiness** 问题（v0.1 尚未实现该维度），不得回写 capability；
//! 认证恢复后由 GUI 轮补强证据。GUI 验收记录见 tests/e2e/README.md。
//!
//! 平台差异复用通用地基，不新增 Gemini 专用逻辑：
//! `harness::probe::candidate_paths`（Windows `.cmd` 优先）与
//! `harness::launch::program_for`（`.ps1` → pwsh）。本机实测 Gemini 是
//! `.cmd` / 无扩展名两件套，真机 E2E 确认选中的是 `.cmd`。
//!
//! 权限语义（ADR-0003）：`data_paths` 只是**路径发现**。本适配器不读取
//! `settings.json` / `oauth_creds.json` / 会话内容；工作区信任弹窗保持
//! Gemini 自己的安全语义，Hub 不注入 `--skip-trust` 之类的绕过参数。

use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::harness::adapter::{
    DetectResult, HarnessAdapter, HarnessCapabilities, HarnessId, LaunchRequest,
};
use crate::harness::launch::{program_for, LaunchSpec};
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

    /// `launch` / `terminal` 已翻为 `true`：headless 真机 lifecycle 证据齐（见模块注释
    /// 与 tests/e2e/README.md）。其余能力未实现，保持 `false`（ADR-0005 零宣称）。
    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            launch: true,
            terminal: true,
            ..HarnessCapabilities::default()
        }
    }

    fn build_launch_spec(&self, request: LaunchRequest) -> Result<LaunchSpec> {
        let binary = self.probe.find_executable(GEMINI_ID).ok_or_else(|| {
            Error::InvalidInput("Gemini CLI 未安装：PATH 中找不到可执行文件".to_string())
        })?;

        // 平台差异交给通用地基：`.ps1` 需要 pwsh 宿主，`.cmd` 直接执行。
        // 默认零参数：工作区信任等策略由 Gemini 自己的 TUI 呈现，Hub 不替用户决定。
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
    use crate::error::Error;
    use crate::harness::adapter::{HarnessAdapter, LaunchRequest};
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

    /// 能力矩阵只包含**已经拿到真机证据**的能力（ADR-0005 零宣称），且与
    /// 机器是否安装无关（capability 是代码的静态事实）。
    ///
    /// `launch` / `terminal` 为 true：headless 真机 spawn + lifecycle（S1-S4）
    /// 见模块注释；usage 与其余能力未验收，不得为 true。
    #[test]
    fn capabilities_claim_only_terminal_lifecycle_evidence() {
        for probe in [installed(), FakeHostProbe::new()] {
            let capabilities = GeminiCliAdapter::new(Arc::new(probe), None).capabilities();
            assert!(
                capabilities.launch,
                "Gemini 已通过 headless 真机 spawn + lifecycle"
            );
            assert!(
                capabilities.terminal,
                "Gemini 已通过真实 TUI 字节 + DSR + resize + 终态取证"
            );
            for (name, value) in [
                ("usage", capabilities.usage),
                ("resume", capabilities.resume),
                ("replay", capabilities.replay),
                ("tool_calls", capabilities.tool_calls),
                ("subagents", capabilities.subagents),
                ("live_state", capabilities.live_state),
                ("worktree", capabilities.worktree),
            ] {
                assert!(!value, "{name} 尚未验收，不得为 true");
            }
        }
    }

    /// 能生成 spec ≠ 顺带宣称别的能力：能力矩阵只随验收证据变化。
    #[test]
    fn building_a_launch_spec_does_not_claim_extra_capabilities() {
        let adapter = GeminiCliAdapter::new(Arc::new(installed()), None);

        let spec = adapter
            .build_launch_spec(LaunchRequest {
                project_id: None,
                cwd: "D:/work".into(),
                args: vec!["--skip-trust".into()],
                runtime_target_id: "local".into(),
            })
            .expect("spec");

        assert_eq!(spec.program, PathBuf::from("D:/npm-global/gemini.cmd"));
        assert_eq!(spec.args, vec!["--skip-trust".to_string()]);
        assert_eq!(spec.cwd, Some(PathBuf::from("D:/work")));
        assert_eq!(spec.runtime_target_id, "local");
        let capabilities = adapter.capabilities();
        assert!(
            !capabilities.usage && !capabilities.resume,
            "spec 与 usage / resume 无关"
        );
    }

    /// `.ps1` shim 走通用地基（pwsh 宿主），Gemini 不新增平台专用分支。
    #[test]
    fn a_ps1_shim_is_hosted_by_pwsh_through_the_shared_helper() {
        let adapter = GeminiCliAdapter::new(
            Arc::new(FakeHostProbe::new().with_binary(
                "gemini",
                "D:/npm-global/gemini.ps1",
                "0.63.0",
            )),
            None,
        );

        let spec = adapter
            .build_launch_spec(LaunchRequest {
                project_id: None,
                cwd: String::new(),
                args: vec![],
                runtime_target_id: "local".into(),
            })
            .expect("spec");

        assert_eq!(spec.program, PathBuf::from("pwsh"));
        assert!(spec.args.contains(&"-File".to_string()), "{:?}", spec.args);
        assert!(
            spec.args.contains(&"D:/npm-global/gemini.ps1".to_string()),
            "{:?}",
            spec.args
        );
    }

    #[test]
    fn launch_spec_is_rejected_when_gemini_is_not_installed() {
        let error = GeminiCliAdapter::new(Arc::new(FakeHostProbe::new()), None)
            .build_launch_spec(LaunchRequest {
                project_id: None,
                cwd: String::new(),
                args: vec![],
                runtime_target_id: "local".into(),
            })
            .expect_err("没装就必须报错");

        assert!(error.to_string().contains("Gemini CLI"), "{error}");
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
