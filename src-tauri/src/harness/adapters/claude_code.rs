//! Claude Code 适配器（Task 7A：只做 detection / installation）。
//!
//! **零宣称**：本 Task 只证明「能检测到、版本读得到、数据目录找得到」。
//! `capabilities` 全部为 `false` —— 即使 `build_launch_spec` 已经能生成结构化 spec，
//! 也**不能**据此打勾：`launch` 要等 7B 的真实 spawn + lifecycle，`terminal` 要等
//! 真实 xterm 双向交互（ADR-0005）。
//!
//! 平台差异复用通用地基，不新增 Claude 专用逻辑：
//! `harness::probe::candidate_paths`（Windows `.cmd`/`.exe` 优先、无扩展名最后）与
//! `harness::launch::program_for`（`.ps1` → pwsh）。本机实测 Claude 与 Codex 一样是
//! `.cmd` / `.ps1` / 无扩展名三件套，因此这套顺序应可直接吃下 —— 真机 E2E 会确认选中的
//! 到底是哪一个。
//!
//! 权限语义（ADR-0003）：这里的 `data_paths` 只是**路径发现**，不是内容读取。
//! 本适配器不解析 `settings.json`、不读 `history.jsonl`、不碰任何用户数据。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::{Error, Result};
use crate::harness::adapter::{
    DetectResult, HarnessAdapter, HarnessCapabilities, HarnessId, LaunchRequest,
};
use crate::harness::launch::{program_for, LaunchSpec};
use crate::harness::probe::HostProbe;

/// 规范标识。ccusage 的 `agent` 值也是 `claude`，两边必须一致，才能把用量关到同一个 harness 上。
pub const CLAUDE_ID: &str = "claude";
/// 展示名。
pub const DISPLAY_NAME: &str = "Claude Code";
/// PATH 上的可执行名。
const EXECUTABLE_NAME: &str = "claude";

/// Claude Code 的数据目录候选。
///
/// 本机实测（2026-09-23，Claude Code 2.1.126）：`~/.claude` 内含
/// `projects/`、`sessions/`、`history.jsonl`、`settings.json`；另有 `~/.claude.json`。
/// 只列出**目录**，文件由上层在需要时决定（7A 不读内容）。
pub fn claude_data_dir_candidates(home: &Path) -> Vec<PathBuf> {
    vec![home.join(".claude")]
}

pub struct ClaudeCodeAdapter {
    probe: Arc<dyn HostProbe>,
}

impl ClaudeCodeAdapter {
    pub fn new(probe: Arc<dyn HostProbe>) -> Self {
        Self { probe }
    }

    /// 只返回**确实存在**的数据目录，避免凭空声称。
    fn existing_data_dirs(&self) -> Vec<String> {
        let Some(home) = self.probe.home_dir() else {
            return Vec::new();
        };

        claude_data_dir_candidates(&home)
            .into_iter()
            .filter(|path| self.probe.dir_exists(path))
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    }
}

impl HarnessAdapter for ClaudeCodeAdapter {
    fn id(&self) -> HarnessId {
        HarnessId::from(CLAUDE_ID)
    }

    fn display_name(&self) -> &str {
        DISPLAY_NAME
    }

    /// 扫描本机：binary / 版本 / 数据路径。**版本读不到不等于没安装** ——
    /// `installed` 只由 binary 是否存在决定（版本探测失败只是 `version = None`）。
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

    /// `launch` / `terminal` 已翻为 `true`：真机 GUI 验收通过（证据见 `tests/e2e/README.md`）——
    /// Terminal Launcher 选 Claude Code 且 cwd 指向窄范围 E2E 目录 → 真实 TUI 首屏显示该
    /// workspace → 真实按键通过 trust 确认 → 进入聊天界面 → 算术 prompt 往返（提交后的
    /// **新输出**里出现 585987）→ GUI kill 得到 `user_killed` + 真实退出码。
    ///
    /// 其余能力未实现，保持 `false`（含 `resume`，见 ADR-0005 的零宣称原则）。
    fn capabilities(&self) -> HarnessCapabilities {
        HarnessCapabilities {
            launch: true,
            terminal: true,
            // 7C：真机 ccusage 里 claude 的新记录经现有 importer **逐 key** 入库
            // （tests/claude_usage.rs），因此这里可以宣称 usage。
            usage: true,
            ..HarnessCapabilities::default()
        }
    }

    fn build_launch_spec(&self, request: LaunchRequest) -> Result<LaunchSpec> {
        let binary = self.probe.find_executable(EXECUTABLE_NAME).ok_or_else(|| {
            Error::InvalidInput("Claude Code 未安装：PATH 中找不到可执行文件".to_string())
        })?;

        // 平台差异交给通用地基：`.ps1` 需要 pwsh 宿主，`.cmd` 直接执行。
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
            // 与 Codex 一致：不隐式拼 shell 环境；需要时由上层显式注入。
            env: Vec::new(),
            runtime_target_id: request.runtime_target_id,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FakeHostProbe;

    /// 本机真机形态：`D:\npm-global\claude.cmd` + `2.1.126 (Claude Code)`。
    fn installed() -> FakeHostProbe {
        FakeHostProbe::new()
            .with_binary(
                "claude",
                "D:/npm-global/claude.cmd",
                "2.1.126 (Claude Code)",
            )
            .with_dir("/home/dev/.claude")
    }

    fn adapter(probe: FakeHostProbe) -> ClaudeCodeAdapter {
        ClaudeCodeAdapter::new(Arc::new(probe))
    }

    fn launch_request(args: &[&str]) -> LaunchRequest {
        LaunchRequest {
            project_id: None,
            cwd: "D:/work".to_string(),
            args: args.iter().map(|value| value.to_string()).collect(),
            runtime_target_id: "local".to_string(),
        }
    }

    #[test]
    fn adapter_identifies_itself_as_claude() {
        let adapter = adapter(installed());

        assert_eq!(adapter.id().as_str(), "claude");
        assert_eq!(adapter.display_name(), "Claude Code");
    }

    #[test]
    fn detect_reports_the_binary_and_the_parsed_version() {
        let detect = adapter(installed()).detect();

        assert!(detect.installed);
        assert_eq!(
            detect.binary_path.as_deref(),
            Some("D:/npm-global/claude.cmd")
        );
        // 不断言「永远是 2.1.126」：这里验证的是「真实输出 → 解析 → 结果」这条链路。
        assert_eq!(detect.version.as_deref(), Some("2.1.126"));
    }

    /// 版本探测失败**不得**把已检测到的 binary 说成未安装。
    #[test]
    fn an_unreadable_version_keeps_the_installation_visible() {
        let adapter = adapter(
            FakeHostProbe::new()
                .with_binary("claude", "D:/npm-global/claude.cmd", "")
                .with_dir("/home/dev/.claude"),
        );

        let detect = adapter.detect();

        assert!(detect.installed, "binary 在就是安装了");
        assert_eq!(
            detect.binary_path.as_deref(),
            Some("D:/npm-global/claude.cmd")
        );
        assert_eq!(detect.version, None, "读不到版本是未知，不是不可用");
    }

    #[test]
    fn nothing_is_claimed_when_the_binary_is_missing() {
        let detect = adapter(FakeHostProbe::new()).detect();

        assert!(!detect.installed);
        assert_eq!(detect.binary_path, None);
        assert_eq!(detect.version, None);
        assert!(detect.data_paths.is_empty());
    }

    #[test]
    fn version_is_none_when_not_installed() {
        assert_eq!(adapter(FakeHostProbe::new()).version().expect("探测"), None);
    }

    #[test]
    fn data_dir_candidates_are_dot_claude() {
        let candidates = claude_data_dir_candidates(Path::new("/home/dev"));

        assert_eq!(candidates, vec![PathBuf::from("/home/dev/.claude")]);
    }

    /// 只报告**存在**的目录：没装过 Claude 的机器上不得凭空列出 `~/.claude`。
    #[test]
    fn only_existing_data_dirs_are_reported() {
        let without_dir = adapter(FakeHostProbe::new().with_binary(
            "claude",
            "D:/npm-global/claude.cmd",
            "2.1.126",
        ))
        .detect();
        assert!(without_dir.data_paths.is_empty());

        let with_dir = adapter(installed()).detect();
        // 用 PathBuf 比较而不是写死字符串：Windows 上 join 出来的是 `\`。
        assert_eq!(
            with_dir.data_paths,
            vec![PathBuf::from("/home/dev")
                .join(".claude")
                .to_string_lossy()
                .into_owned()]
        );
    }

    /// 7A 的核心断言：detection 阶段**一个能力都不宣称**。
    /// 能力矩阵只包含**已经拿到真机证据**的能力（ADR-0005 零宣称）。
    ///
    /// 7B 起 `launch` / `terminal` 为 true：真机 GUI 往返（Terminal Launcher → Claude Code
    /// → 窄范围 cwd → trust 确认 → 聊天界面 → 算术 prompt 的**新输出** → user_killed 终态）。
    #[test]
    fn capabilities_claim_only_what_has_real_evidence() {
        let capabilities = adapter(installed()).capabilities();

        assert!(capabilities.launch, "Claude 已通过真机 spawn + lifecycle");
        assert!(
            capabilities.terminal,
            "Claude 已通过真机 GUI 双向交互 + resize"
        );
        assert!(capabilities.usage, "Claude 已通过真机 ccusage 逐 key 对账");
        for (name, value) in [
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

    /// 能生成 spec ≠ 宣称 launch：两者必须分开断言，防止「顺手打个勾」。
    /// 能生成 spec 与能力声明是**两件事**：生成 spec 不得带动别的能力。
    #[test]
    fn building_a_launch_spec_does_not_claim_extra_capabilities() {
        let adapter = adapter(installed());

        let spec = adapter
            .build_launch_spec(launch_request(&["--help"]))
            .expect("spec");

        assert_eq!(spec.program, PathBuf::from("D:/npm-global/claude.cmd"));
        assert_eq!(spec.args, vec!["--help".to_string()]);
        assert_eq!(spec.cwd, Some(PathBuf::from("D:/work")));
        assert_eq!(spec.runtime_target_id, "local");
        assert!(!adapter.capabilities().resume, "spec 与 resume 无关");
    }

    /// `.ps1` shim 走通用地基（pwsh 宿主），Claude 不新增平台专用分支。
    #[test]
    fn a_ps1_shim_is_hosted_by_pwsh_through_the_shared_helper() {
        let adapter = adapter(FakeHostProbe::new().with_binary(
            "claude",
            "D:/npm-global/claude.ps1",
            "2.1.126",
        ));

        let spec = adapter
            .build_launch_spec(launch_request(&[]))
            .expect("spec");

        assert_eq!(spec.program, PathBuf::from("pwsh"));
        assert!(spec.args.contains(&"-File".to_string()), "{:?}", spec.args);
        assert!(
            spec.args.contains(&"D:/npm-global/claude.ps1".to_string()),
            "{:?}",
            spec.args
        );
    }

    #[test]
    fn launch_spec_is_rejected_when_claude_is_not_installed() {
        let error = adapter(FakeHostProbe::new())
            .build_launch_spec(launch_request(&[]))
            .expect_err("没装就必须报错");

        assert!(error.to_string().contains("Claude Code"), "{error}");
    }
}
