//! 集成测试共用的小工具（`tests/common/` 目录不会被 Cargo 当成独立 target）。
//!
//! 两类东西：
//!
//! 1. **纯函数**：真机 E2E 的「这台机器到底有没有用量数据」前提判定。之所以共用，是因为
//!    `real_ccusage_import.rs` 与 `real_dashboard_summary.rs` 必须遵守**同一条**规则；
//!    两处各写一份一定会漂移。
//! 2. **[`ReplaySections`]**：真实跑一次目标命令、把输出固定下来，后续导入 replay 同一份，
//!    让「同快照对账」「重复导入必须幂等」比的是同一份数据（见该类型的文档）。
//!    窗口由调用方选（[`CaptureWindow`]）：**只有**需要比较 `daily` 与 `session` 的真机对账
//!    才用 `StablePast`；其余测试用 `Full`，避免丢掉今天的真实覆盖、或让逐 key 检查空转通过。
//!
//! 二者都只动测试侧，不碰生产代码。

#![allow(dead_code)] // 每个 test binary 只用到其中一部分

use harness_hub_lib::error::Result;
use harness_hub_lib::harness::probe::SystemHostProbe;
use harness_hub_lib::usage::runner::{
    resolve_runner, session_report_arguments, CommandOutput, CommandRunner, CommandSpec,
    SystemCommandRunner,
};
use serde_json::Value;

/// 真机 E2E 的前提判定结果。
#[derive(Debug, PartialEq, Eq)]
pub enum Precondition {
    /// 来源报告**本身**是空的：这台机器确实没有用量数据 → 明确跳过。
    NoDataInSource,
    /// 报告里有行，导入却得到 0 条 → adapter 丢行，必须**硬失败**（不能被跳过掩盖）。
    SourceHadRowsButNothingImported,
    /// 正常：既有数据也导入了，继续跑完整对账。
    Proceed,
}

/// 报告本身是否为空：`session` 与 `daily` 两个 section 都没有行。
///
/// 缺字段（例如 `empty.json` 没有 `daily`）按 0 行处理。
pub fn report_is_blank(report: &Value) -> bool {
    fn rows(report: &Value, key: &str) -> usize {
        report[key].as_array().map_or(0, Vec::len)
    }
    rows(report, "session") == 0 && rows(report, "daily") == 0
}

/// 前提判定：**先看报告本身是否为空**，再看导入条数。
///
/// 顺序是关键。早期版本只看「导入 0 条就跳过」，那会把「报告里有行、adapter 却把行丢光」
/// 这种真实回归一起吞掉（跳过 = 通过，见本文件 `rows_in_the_report_but_nothing_imported…`
/// 那条测试）。所以规则是：来源空 → 跳过；来源有行但一条没导入 → **失败**。
pub fn precondition(report: &Value, records_seen: u64) -> Precondition {
    if report_is_blank(report) {
        return Precondition::NoDataInSource;
    }
    if records_seen == 0 {
        return Precondition::SourceHadRowsButNothingImported;
    }
    Precondition::Proceed
}

/// 测试侧 replay：把**目标 `sections` 命令**的输出固定成一份快照。
///
/// 为什么必须有它：ccusage 的统计来自**活的** rollout 文件（本机 codex 会话在持续追加），
/// 每次调用都会重新推导，于是同一个 key 的数值在两次调用之间会变大（实测：262 行不变、
/// 其中 18 行被改写，+43214 token）。而真机 E2E 里的「同快照对账」与「重复导入必须幂等」
/// 想比的显然是**同一份数据** —— 先用真实 runner 跑一次留下输出，后续对账与幂等检查
/// 全部复用它，被比较的双方就由构造保证来自同一个快照。
///
/// **只 replay 目标命令**：`--version`、可用性探测等其他命令仍然走真实 runner，
/// 所以 provenance 与 detect 的结论没有变假。断言没有被放宽，也没有新增跳过：
/// 幂等断言（`inserted == 0`、汇总不变）在 replay 之下变成**精确**成立。
pub struct ReplaySections {
    /// 非目标命令照常走它。
    inner: Box<dyn CommandRunner>,
    /// 捕获时确定的**完整**目标命令（program + 全部参数）。必须整条匹配，不能只看参数尾巴：
    /// 换了可执行程序（例如真正的 `ccusage` 而不是托管的 `npx … ccusage@20.0.24`）或换了
    /// runner 前缀时，那条命令的输出与这份捕获无关，绝不能拿旧输出顶替。
    target: CommandSpec,
    /// 第一次真实运行的结果；之后每次命中目标命令都返回它。
    captured: CommandOutput,
}

/// 捕获窗口：决定这次真实取数要不要加日期上界。
///
/// **只在需要比较 `daily` 与 `session` 的真机对账里用 [`Self::StablePast`]**；
/// 其余测试用 [`Self::Full`]，因为它们只需要「同一份输出」（replay），
/// 加了历史上界反而会丢掉今天的真实覆盖、甚至让逐 key 检查空转通过。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureWindow {
    /// 全量取数：不加任何日期上界。
    Full,
    /// 稳定窗口：`--until <UTC 今天-2 天> -z UTC`（见 [`stable_window_arguments`]）。
    StablePast,
}

impl CaptureWindow {
    fn arguments(self, now_unix_seconds: u64) -> Vec<String> {
        match self {
            Self::Full => Vec::new(),
            Self::StablePast => stable_window_arguments(now_unix_seconds),
        }
    }
}

impl ReplaySections {
    pub fn new(
        inner: Box<dyn CommandRunner>,
        target: CommandSpec,
        captured: CommandOutput,
    ) -> Self {
        Self {
            inner,
            target,
            captured,
        }
    }

    /// 真实跑**一次**目标命令，返回 `(replayer, 那一次的输出)`：
    /// 调用方用后者解析报告（报告与后续导入因此是同一份输出）。
    ///
    /// 捕获命令 = 生产报告参数（+ 可选窗口参数）；而 **replay 的匹配目标始终是生产命令本身**
    /// （runner 前缀 + `session_report_arguments()`），因为适配器导入时构造的就是它。
    pub fn capture_once(
        probe: &SystemHostProbe,
        real: &dyn CommandRunner,
        window: CaptureWindow,
    ) -> Option<(Self, CommandOutput)> {
        let runner = resolve_runner(probe, None)?;
        let target = runner.with_arguments(&session_report_arguments());
        let mut capture_args = target.args.clone();
        capture_args.extend(window.arguments(now_unix_seconds()));
        let capture_spec = CommandSpec {
            program: target.program.clone(),
            args: capture_args,
        };
        let captured = real.run(&capture_spec).expect("调用 ccusage");
        let replayer = Self::new(
            Box::new(SystemCommandRunner::new()),
            target,
            captured.clone(),
        );
        Some((replayer, captured))
    }

    /// 整条命令相等才算命中（program + 全部参数）。
    fn is_target(&self, command: &CommandSpec) -> bool {
        command == &self.target
    }
}

/// 稳定窗口参数：`--until <UTC 今天 - 2 天> -z UTC`。
///
/// 为什么需要上界：ccusage 的统计来自活的 rollout 文件。进行中的日期在被读取期间还在增长，
/// 于是**同一次调用内部**先算的 `daily` 与后算的 `session` 都会不一致（实测同一份报告里
/// codex `daily` 比 `session` 少 166271 / 84090，而写入暂停时差值恰好是 0）。
/// 把上界切到至少 24 小时之前，再做同一份快照的逐项对账才有意义。
///
/// 为什么是「UTC 今天 - 2 天」而不是「- 1 天」：`--until` 按日期截断，而 ccusage 的日期
/// 分组用本机时区。减 2 天可以保证**任何时区**下上界距现在都 ≥ 24 小时。
/// `-z UTC` 让分组本身也不依赖机器时区。
pub fn stable_window_arguments(now_unix_seconds: u64) -> Vec<String> {
    let cutoff =
        harness_hub_lib::clock::format_rfc3339(now_unix_seconds.saturating_sub(2 * 86_400));
    vec![
        "--until".to_string(),
        cutoff[..10].to_string(),
        "-z".to_string(),
        "UTC".to_string(),
    ]
}

fn now_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

impl CommandRunner for ReplaySections {
    fn run(&self, command: &CommandSpec) -> Result<CommandOutput> {
        if self.is_target(command) {
            return Ok(self.captured.clone());
        }
        self.inner.run(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECTIONS: &str = include_str!("../../../fixtures/ccusage/sections.json");
    const EMPTY: &str = include_str!("../../../fixtures/ccusage/empty.json");

    fn parse(raw: &str) -> Value {
        serde_json::from_str(raw).expect("fixture 必须是合法 JSON")
    }

    /// 复现缺口：报告里有行（sections.json: session 2 行 / daily 2 行）却一条都没导入时，
    /// 判定**必须**是「失败」而不是「跳过」—— 否则 adapter 丢行的真实回归会被跳过吞掉。
    #[test]
    fn rows_in_the_report_but_nothing_imported_is_a_failure_not_a_skip() {
        assert_eq!(
            precondition(&parse(SECTIONS), 0),
            Precondition::SourceHadRowsButNothingImported,
            "报告有行却导入 0 条 = adapter 丢行，绝不能判成「机器没有数据」"
        );
    }

    /// 只有报告本身为空才算「机器没有数据」。
    #[test]
    fn a_blank_report_is_the_only_no_data_shape() {
        assert_eq!(
            precondition(&parse(EMPTY), 0),
            Precondition::NoDataInSource,
            "空报告（session/daily 都没有行）才是真正的「没有数据」"
        );
    }

    /// 正常形状：有行、也导入了 → 继续跑对账。
    #[test]
    fn rows_with_an_actual_import_proceed() {
        assert_eq!(precondition(&parse(SECTIONS), 7), Precondition::Proceed);
    }

    /// 稳定窗口的日期算法：上界必须是「UTC 今天 - 2 天」（跨月/跨年交给 `clock`）。
    #[test]
    fn the_stable_window_cuts_well_before_now() {
        assert_eq!(
            stable_window_arguments(1_700_000_000),
            vec!["--until", "2023-11-12", "-z", "UTC"],
            "2023-11-14T22:13:20Z 减 2 天 = 2023-11-12"
        );
        assert_eq!(
            stable_window_arguments(1_772_323_200)[1],
            "2026-02-27",
            "跨月：2026-03-01 减 2 天（2026 不是闰年）"
        );
    }

    /// 窗口只影响**取数命令**：`Full` 不许偷偷加上界（否则会丢掉今天的真实覆盖，
    /// 甚至让逐 key 检查空转通过）。
    #[test]
    fn only_the_stable_window_adds_the_historical_cutoff() {
        assert!(
            CaptureWindow::Full.arguments(1_700_000_000).is_empty(),
            "Full 必须是不加任何日期上界的全量取数"
        );
        assert_eq!(
            CaptureWindow::StablePast.arguments(1_700_000_000),
            vec!["--until", "2023-11-12", "-z", "UTC"],
            "只有 StablePast 才切上界"
        );
    }

    /// 只 replay **完整等于**目标命令的那一条；其他命令（含换了 runner / 可执行程序的）
    /// **必须**仍然走真实 runner —— 否则 `--version`、可用性探测、甚至另一条 runner 的输出
    /// 都会被固定住，测试就变成自证或错认。
    #[test]
    fn only_the_exact_target_command_is_replayed() {
        use harness_hub_lib::usage::runner::{
            session_report_arguments, CommandOutput, CommandRunner, CommandSpec,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        /// 假的「真实 runner」：只数被调用了几次，返回一个可辨认的哨兵输出。
        struct Stub {
            calls: Arc<AtomicUsize>,
        }

        impl CommandRunner for Stub {
            fn run(&self, _command: &CommandSpec) -> harness_hub_lib::error::Result<CommandOutput> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(CommandOutput {
                    exit_code: 0,
                    stdout: "DELEGATED".to_string(),
                    stderr: String::new(),
                })
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let captured = CommandOutput {
            exit_code: 0,
            stdout: SECTIONS.to_string(),
            stderr: String::new(),
        };

        // 目标命令：runner 前缀 + 报告参数（与生产 `import` 构造出来的完全一致）
        let mut args = vec!["--yes".to_string(), "ccusage@20.0.24".to_string()];
        args.extend(session_report_arguments());
        let target = CommandSpec {
            program: "npx".to_string(),
            args,
        };
        let replay = ReplaySections::new(
            Box::new(Stub {
                calls: Arc::clone(&calls),
            }),
            target.clone(),
            captured.clone(),
        );

        assert_eq!(
            replay.run(&target).expect("replay"),
            captured,
            "完整等于目标命令必须 replay"
        );
        assert_eq!(
            replay.run(&target).expect("replay"),
            captured,
            "重复调用必须仍然是同一份（这正是幂等断言所需要的）"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "目标命令不得碰到真实 runner"
        );

        // ① 换了可执行程序、但参数尾巴一样 —— 绝不能拿旧输出顶替
        let other_program = CommandSpec {
            program: "ccusage".to_string(),
            args: target.args.clone(),
        };
        assert_eq!(
            replay.run(&other_program).expect("delegate").stdout,
            "DELEGATED",
            "换了 program 的命令必须走真实 runner"
        );

        // ② 同一个程序、但 runner 前缀不同（例如别的版本）—— 同样不能命中
        let mut other_prefix_args = vec!["--yes".to_string(), "ccusage@20.0.25".to_string()];
        other_prefix_args.extend(session_report_arguments());
        let other_prefix = CommandSpec {
            program: target.program.clone(),
            args: other_prefix_args,
        };
        assert_eq!(
            replay.run(&other_prefix).expect("delegate").stdout,
            "DELEGATED",
            "runner 前缀不同的命令必须走真实 runner"
        );

        // ③ 目标命令 + 额外参数（例如捕获时带的窗口参数）也不再命中
        let mut windowed_args = target.args.clone();
        windowed_args.extend(["--until".to_string(), "2023-11-12".to_string()]);
        let windowed = CommandSpec {
            program: target.program.clone(),
            args: windowed_args,
        };
        assert_eq!(
            replay.run(&windowed).expect("delegate").stdout,
            "DELEGATED",
            "带额外参数的命令与目标命令不同，必须走真实 runner"
        );

        // ④ 完全无关的命令（`--version`）
        let version = CommandSpec {
            program: "npx".to_string(),
            args: vec!["--version".to_string()],
        };
        assert_eq!(
            replay.run(&version).expect("delegate").stdout,
            "DELEGATED",
            "非目标命令必须委托给真实 runner"
        );

        assert_eq!(
            calls.load(Ordering::SeqCst),
            4,
            "四个非目标命令都必须走真实 runner"
        );
    }
}
