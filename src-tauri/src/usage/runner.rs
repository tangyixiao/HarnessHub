//! 外部命令的解析与调用（ADR-0011 决策一）。
//!
//! 解析顺序是**固定**的：
//!
//! ```text
//! 1. PATH 上的 ccusage
//! 2. 用户显式配置的 executable + args
//! 3. managed runner：固定版本（ccusage@20.0.24）
//! 4. 都不可用 → 调用方报告 unavailable（正常状态，不是崩溃）
//! ```
//!
//! 生产默认**永远不是** `ccusage@latest`：那等于让 JSON schema 在未来的某一天无声改变，
//! 再由我们的解析器去猜。自动升级是独立的 upgrade flow，不混进数据管道。
//!
//! 进程执行与 PATH 查找都通过 trait 注入，测试因此不需要真实进程与真实 PATH。

use std::path::PathBuf;

use crate::error::{Error, Result};
use crate::harness::probe::{parse_version, HostProbe};
use crate::usage::RunnerKind;

/// PATH 上期望的可执行名。
pub const CCUSAGE_BINARY: &str = "ccusage";
/// managed runner 用的启动器。
pub const MANAGED_BINARY: &str = "npx";
/// managed runner 固定到的版本。**改这里等于改数据管道的输入契约。**
pub const MANAGED_PACKAGE: &str = "ccusage@20.0.24";

/// 追加到 `--version` 参数。
pub fn version_arguments() -> Vec<String> {
    vec!["--version".to_string()]
}

/// 一次导入要执行的参数（ADR-0011 决策七：单次调用拿全部分段）。
pub fn session_report_arguments() -> Vec<String> {
    vec![
        "session".to_string(),
        "--sections".to_string(),
        "daily".to_string(),
        "--by-agent".to_string(),
        "--json".to_string(),
    ]
}

/// 一条可执行的命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }

    /// 人可读形式。**只用于日志/错误信息**，绝不交给 shell 执行。
    pub fn describe(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// 解析结果：用哪个 runner、具体命令是什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRunner {
    pub kind: RunnerKind,
    pub command: CommandSpec,
}

impl ResolvedRunner {
    /// 把报告参数接到 runner 命令后面。
    pub fn with_arguments(&self, arguments: &[String]) -> CommandSpec {
        let mut args = self.command.args.clone();
        args.extend_from_slice(arguments);
        CommandSpec {
            program: self.command.program.clone(),
            args,
        }
    }
}

/// 按固定顺序解析 runner。返回 `None` = 本机确实没有可用 runner。
pub fn resolve_runner(
    probe: &dyn HostProbe,
    configured: Option<CommandSpec>,
) -> Option<ResolvedRunner> {
    if let Some(path) = probe.find_executable(CCUSAGE_BINARY) {
        return Some(ResolvedRunner {
            kind: RunnerKind::Path,
            command: CommandSpec::new(display_path(path), Vec::new()),
        });
    }

    if let Some(configured) = configured {
        return Some(ResolvedRunner {
            kind: RunnerKind::Configured,
            command: configured,
        });
    }

    if let Some(path) = probe.find_executable(MANAGED_BINARY) {
        return Some(ResolvedRunner {
            kind: RunnerKind::ManagedNpx,
            command: CommandSpec::new(
                display_path(path),
                vec!["--yes".to_string(), MANAGED_PACKAGE.to_string()],
            ),
        });
    }

    None
}

fn display_path(path: PathBuf) -> String {
    path.to_string_lossy().into_owned()
}

/// 一次外部命令的执行结果。**保留退出码与 stderr**：
/// 「ccusage 非零退出」必须变成一条失败的导入记录，而不是「没有数据」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn failed(&self) -> bool {
        self.exit_code != 0
    }

    /// 失败时统一转成领域错误（前端/导入记录直接可用）。
    pub fn into_error(self, command: &CommandSpec) -> Error {
        let _ = command;
        Error::UsageCommandFailed {
            exit_code: self.exit_code,
            stderr: summarize_stderr(&self.stderr),
        }
    }
}

/// stderr 摘要：去掉空行、只留前几行，避免把整屏滚屏日志塞进数据库。
fn summarize_stderr(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .collect();
    if lines.is_empty() {
        "(stderr 为空)".to_string()
    } else {
        lines.join(" / ")
    }
}

/// 执行外部命令的能力（测试注入假实现，生产用真实子进程）。
pub trait CommandRunner: Send + Sync {
    fn run(&self, command: &CommandSpec) -> Result<CommandOutput>;
}

/// 真实实现：直接 `Command::new`，不经 shell（因此没有引号/注入问题）。
///
/// 唯一没有单测的分支：真实进程执行本身由真机 E2E（`tests/real_ccusage_import.rs`）覆盖。
pub struct SystemCommandRunner;

impl SystemCommandRunner {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SystemCommandRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRunner for SystemCommandRunner {
    fn run(&self, command: &CommandSpec) -> Result<CommandOutput> {
        let output = std::process::Command::new(&command.program)
            .args(&command.args)
            .output()?;

        Ok(CommandOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// 探测版本号。非零退出或输出里没有版本都算「探测不到」，由调用方决定怎么报告。
pub fn probe_version(
    runner: &ResolvedRunner,
    executor: &dyn CommandRunner,
) -> Result<Option<(String, String)>> {
    let command = runner.with_arguments(&version_arguments());
    let output = executor.run(&command)?;
    if output.failed() {
        return Err(output.into_error(&command));
    }
    let combined = format!("{} {}", output.stdout, output.stderr);
    Ok(parse_version(&combined).map(|version| (version, command.describe())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeHostProbe, ScriptedCommandRunner};

    fn configured() -> CommandSpec {
        CommandSpec::new(
            "D:/tools/ccusage.cmd",
            vec!["--config".to_string(), "D:/tools/ccusage.json".to_string()],
        )
    }

    #[test]
    fn prefers_the_path_binary_over_everything_else() {
        let probe = FakeHostProbe::with(&["ccusage", "npx"]);

        let runner = resolve_runner(&probe, Some(configured())).expect("必须解析出来");

        assert_eq!(runner.kind, RunnerKind::Path);
        assert!(runner.command.program.contains("ccusage"));
        assert!(runner.command.args.is_empty());
    }

    #[test]
    fn uses_the_configured_command_before_the_managed_runner() {
        let probe = FakeHostProbe::with(&["npx"]);

        let runner = resolve_runner(&probe, Some(configured())).expect("必须解析出来");

        assert_eq!(runner.kind, RunnerKind::Configured);
        assert_eq!(runner.command, configured());
    }

    #[test]
    fn falls_back_to_the_pinned_managed_runner() {
        let probe = FakeHostProbe::with(&["npx"]);

        let runner = resolve_runner(&probe, None).expect("npx 在就必须能用托管 runner");

        assert_eq!(runner.kind, RunnerKind::ManagedNpx);
        assert_eq!(
            runner.command.args,
            vec!["--yes".to_string(), "ccusage@20.0.24".to_string()]
        );
    }

    /// Windows 上 `npx` 实际是 `npx.cmd`，只给名字会 `NotFound`：
    /// 必须执行**探测到的绝对路径**。真机 E2E 抓到过这个 bug。
    #[test]
    fn managed_runner_executes_the_resolved_absolute_path() {
        let probe = FakeHostProbe::with(&["npx"]);

        let runner = resolve_runner(&probe, None).expect("托管 runner");

        assert_eq!(
            runner.command.program, "D:/fake/npx",
            "必须执行探测到的路径，而不是裸名字"
        );
    }

    /// 生产路径**永远**不得出现 `latest`：那会让 schema 无声漂移。
    #[test]
    fn never_builds_an_invocation_with_latest() {
        let probe = FakeHostProbe::with(&["npx"]);
        let runner = resolve_runner(&probe, None).expect("托管 runner");

        let full = runner
            .with_arguments(&session_report_arguments())
            .describe();

        assert!(
            !full.contains("latest"),
            "invocation 里出现了 latest：{full}"
        );
        assert!(full.contains("ccusage@20.0.24"), "{full}");
    }

    #[test]
    fn reports_unavailable_instead_of_failing_when_nothing_is_installed() {
        let probe = FakeHostProbe::with(&[]);

        assert!(resolve_runner(&probe, None).is_none());
    }

    #[test]
    fn the_import_invocation_is_a_single_multi_section_call() {
        let invocation = session_report_arguments();

        assert_eq!(invocation[0], "session");
        assert!(invocation.contains(&"--sections".to_string()));
        assert!(invocation.contains(&"--by-agent".to_string()));
        assert!(invocation.contains(&"--json".to_string()));
    }

    #[test]
    fn appending_arguments_keeps_the_runner_prefix() {
        let probe = FakeHostProbe::with(&["npx"]);
        let runner = resolve_runner(&probe, None).expect("托管 runner");

        let command = runner.with_arguments(&session_report_arguments());

        assert!(command.program.contains("npx"), "{:?}", command.program);
        assert_eq!(&command.args[..2], &["--yes", "ccusage@20.0.24"]);
        assert_eq!(command.args.len(), 2 + session_report_arguments().len());
    }

    #[test]
    fn probe_version_parses_the_real_ccusage_output() {
        let probe = FakeHostProbe::with(&["npx"]);
        let runner = resolve_runner(&probe, None).expect("托管 runner");
        let executor = ScriptedCommandRunner::returning("npx", 0, "ccusage 20.0.24\n", "");

        let (version, command) = probe_version(&runner, &executor)
            .expect("探测")
            .expect("有版本");

        assert_eq!(version, "20.0.24");
        assert!(command.contains("--version"), "{command}");
    }

    #[test]
    fn probe_version_reads_stderr_too() {
        let probe = FakeHostProbe::with(&["npx"]);
        let runner = resolve_runner(&probe, None).expect("托管 runner");
        let executor = ScriptedCommandRunner::returning("npx", 0, "", "ccusage 20.0.24\n");

        let version = probe_version(&runner, &executor)
            .expect("探测")
            .expect("有版本")
            .0;

        assert_eq!(version, "20.0.24");
    }

    #[test]
    fn probe_version_reports_a_non_zero_exit_instead_of_guessing() {
        let probe = FakeHostProbe::with(&["npx"]);
        let runner = resolve_runner(&probe, None).expect("托管 runner");
        let executor =
            ScriptedCommandRunner::returning("npx", 2, "", "Unknown option '--version'\n");

        let error = probe_version(&runner, &executor).expect_err("非零退出必须报错");

        match error {
            Error::UsageCommandFailed { exit_code, stderr } => {
                assert_eq!(exit_code, 2);
                assert!(stderr.contains("Unknown option"), "{stderr}");
            }
            other => panic!("必须是 UsageCommandFailed，实际：{other}"),
        }
    }

    #[test]
    fn probe_version_returns_none_when_output_has_no_version() {
        let probe = FakeHostProbe::with(&["npx"]);
        let runner = resolve_runner(&probe, None).expect("托管 runner");
        let executor = ScriptedCommandRunner::returning("npx", 0, "no version here\n", "");

        assert!(probe_version(&runner, &executor).expect("探测").is_none());
    }

    #[test]
    fn stderr_summaries_stay_short_and_never_empty() {
        let long = (1..=10)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(summarize_stderr(&long), "line 1 / line 2 / line 3");
        assert_eq!(summarize_stderr("\n\n"), "(stderr 为空)");
    }

    /// 非零退出留下的错误信息必须能让人定位问题。
    #[test]
    fn a_failed_command_reports_exit_code_and_stderr() {
        let output = CommandOutput {
            exit_code: 2,
            stdout: String::new(),
            stderr: "Unknown session option '--nope'\nRun 'ccusage --help' for usage.".to_string(),
        };

        let error = output.into_error(&CommandSpec::new("ccusage", vec![]));
        let text = error.to_string();

        assert!(text.contains('2'), "{text}");
        assert!(text.contains("Unknown session option"), "{text}");
    }
}
