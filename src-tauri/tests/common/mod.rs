//! 集成测试共用的小工具（`tests/common/` 目录不会被 Cargo 当成独立 target）。
//!
//! 两类东西：
//!
//! 1. **纯函数**：真机 E2E 的「这台机器到底有没有用量数据」前提判定。之所以共用，是因为
//!    `real_ccusage_import.rs` 与 `real_dashboard_summary.rs` 必须遵守**同一条**规则；
//!    两处各写一份一定会漂移。
//! 2. **[`ReplaySections`]**：把目标 `sections` 命令的输出固定成**一份快照**，让真机 E2E
//!    比的是同一份数据，而不是两次调用之间已经变过的数据（见该类型的文档）。
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
    /// 目标命令的参数尾巴（`session_report_arguments()`）。只看尾巴，不看 program：
    /// 不同 runner（直接 ccusage / 托管 npx）前缀不同，参数尾巴才是稳定的部分。
    sections_args: Vec<String>,
    /// 第一次真实运行的结果；之后每次调用都返回它。
    captured: CommandOutput,
}

impl ReplaySections {
    pub fn new(
        inner: Box<dyn CommandRunner>,
        sections_args: Vec<String>,
        captured: CommandOutput,
    ) -> Self {
        Self {
            inner,
            sections_args,
            captured,
        }
    }

    /// 真实跑**一次**目标命令，返回 `(replayer, 那一次的输出)`：
    /// 调用方用后者解析报告（报告与后续导入因此是同一份快照）。
    ///
    /// 命令 = 生产报告参数 + [`stable_window_arguments`]：生产参数的窗口是「全部数据」，
    /// 而**进行中的日期还在增长**，所以必须把上界切到至少 24 小时之前，这份快照本身
    /// 才是稳定的（否则连单次调用内部的两个 section 都会互相不一致）。
    pub fn capture_once(
        probe: &SystemHostProbe,
        real: &dyn CommandRunner,
    ) -> Option<(Self, CommandOutput)> {
        let runner = resolve_runner(probe, None)?;
        let mut arguments = session_report_arguments();
        arguments.extend(stable_window_arguments(now_unix_seconds()));
        let command = runner.with_arguments(&arguments);
        let captured = real.run(&command).expect("调用 ccusage");
        let replayer = Self::new(
            Box::new(SystemCommandRunner::new()),
            session_report_arguments(),
            captured.clone(),
        );
        Some((replayer, captured))
    }

    fn is_target(&self, command: &CommandSpec) -> bool {
        command.args.ends_with(self.sections_args.as_slice())
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

    /// 只 replay 目标 `sections` 命令；其他命令**必须**仍然走真实 runner ——
    /// 否则 `--version` / 可用性探测的结论也被固定住了，测试就变成自证。
    #[test]
    fn only_the_target_sections_command_is_replayed() {
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
        let replay = ReplaySections::new(
            Box::new(Stub {
                calls: Arc::clone(&calls),
            }),
            session_report_arguments(),
            captured.clone(),
        );

        // 目标命令：runner 前缀 + 报告参数（与生产 `import` 构造出来的完全一致）
        let mut args = vec!["--yes".to_string(), "ccusage@20.0.24".to_string()];
        args.extend(session_report_arguments());
        let target = CommandSpec {
            program: "npx".to_string(),
            args,
        };
        assert_eq!(
            replay.run(&target).expect("replay"),
            captured,
            "目标命令必须 replay 那份快照"
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

        let other = CommandSpec {
            program: "npx".to_string(),
            args: vec!["--version".to_string()],
        };
        assert_eq!(
            replay.run(&other).expect("delegate").stdout,
            "DELEGATED",
            "非目标命令必须委托给真实 runner"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "非目标命令必须走真实 runner"
        );
    }
}
