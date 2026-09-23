//! `UsageSourceAdapter`：Usage 数据源的发现与导入（ADR-0002：与 `HarnessAdapter` 分离）。
//!
//! 方法按**代价**分成两类，调用方必须分清：
//!
//! | 方法 | 是否起进程 | 用途 |
//! | --- | --- | --- |
//! | [`UsageSourceAdapter::detect`] | 否（只看 PATH） | 只读 UI：能不能用、用哪个 runner |
//! | [`UsageSourceAdapter::version`] | 是（`--version`） | 真机探测版本 |
//! | [`UsageSourceAdapter::import`] | 是（一次 `--sections` 调用） | **变更操作**：调用 + 落库 |
//!
//! 只读路径不起进程是刻意的：`npx` 一次要一两秒，放在会被反复调用的只读接口里
//! 既慢又会把「读」变成「干活」。

use rusqlite::Connection;

use crate::clock;
use crate::error::{Error, Result};
use crate::harness::probe::HostProbe;
use crate::usage::ccusage::{
    normalize_session, parse_report, CcusageReport, PRICING_MODE_AUTO, REPORT_KIND, SOURCE_ID,
};
use crate::usage::importer::{request_from, FailedImportRequest, ImportOutcome, UsageImporter};
use crate::usage::runner::{
    probe_version, resolve_runner, session_report_arguments, CommandRunner, CommandSpec,
    ResolvedRunner,
};
use crate::usage::{RunnerKind, SourceStatus, UsageCapabilities, UsageSource};

/// 一个 Usage 数据源适配器。
///
/// `version` 与 `detect` 分开：`detect` 必须廉价且无副作用，
/// `version` 会真的起进程，因此只在变更路径上调用。
pub trait UsageSourceAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;

    /// 只读、**不起进程**：本机有没有、用哪个 runner、不可用时为什么。
    fn detect(&self) -> UsageSource;

    /// 真实版本（需要起一次 `--version`；探测不到返回 `None`）。
    fn version(&self) -> Result<Option<String>>;

    fn capabilities(&self) -> UsageCapabilities;

    /// **变更操作**：调用来源 → 归一化 → 幂等落库。
    ///
    /// 失败时**先写一条 `status = 'failed'` 的审计行再返回错误**，
    /// 这样「命令挂了」和「本来就没数据」在数据库里是可区分的。
    fn import(&self, connection: &Connection) -> Result<ImportOutcome>;
}

/// ccusage 的实现。v0.1 只注册它一个（ADR-0011 的外部优先原则）。
pub struct CcusageAdapter<'a> {
    probe: &'a dyn HostProbe,
    executor: &'a dyn CommandRunner,
    /// 用户显式配置的命令。`None` = 只能靠 PATH 或托管 runner。
    configured: Option<CommandSpec>,
}

impl<'a> CcusageAdapter<'a> {
    pub fn new(
        probe: &'a dyn HostProbe,
        executor: &'a dyn CommandRunner,
        configured: Option<CommandSpec>,
    ) -> Self {
        Self {
            probe,
            executor,
            configured,
        }
    }

    fn resolve(&self) -> Option<ResolvedRunner> {
        resolve_runner(self.probe, self.configured.clone())
    }

    fn unavailable_reason(&self) -> String {
        format!(
            "本机 PATH 上没有 {}，也没有可用的托管 runner（{}）。\
             可以安装 ccusage，或在设置里显式配置可执行文件与参数。",
            crate::usage::runner::CCUSAGE_BINARY,
            crate::usage::runner::MANAGED_PACKAGE
        )
    }
}

impl UsageSourceAdapter for CcusageAdapter<'_> {
    fn id(&self) -> &'static str {
        SOURCE_ID
    }

    fn display_name(&self) -> &'static str {
        "ccusage"
    }

    fn detect(&self) -> UsageSource {
        let (status, runner, reason) = match self.resolve() {
            Some(resolved) => (SourceStatus::Available, Some(resolved.kind), None),
            None => (
                SourceStatus::Unavailable,
                None,
                Some(self.unavailable_reason()),
            ),
        };

        UsageSource {
            id: SOURCE_ID.to_string(),
            display_name: self.display_name().to_string(),
            // 版本要起进程才知道；只读路径只报告「上一次导入时看到的版本」（由调用方合并）。
            version: None,
            status,
            capabilities: self.capabilities(),
            runner,
            reason,
        }
    }

    fn version(&self) -> Result<Option<String>> {
        let runner = self
            .resolve()
            .ok_or_else(|| Error::UsageUnavailable(self.unavailable_reason()))?;
        Ok(probe_version(&runner, self.executor)?.map(|(version, _)| version))
    }

    fn capabilities(&self) -> UsageCapabilities {
        UsageCapabilities {
            detect: true,
            import: true,
            // 文件监听未实现：不许写 true（ADR-0005）。
            watch: false,
            reconcile: true,
        }
    }

    fn import(&self, connection: &Connection) -> Result<ImportOutcome> {
        let started_at = clock::now_rfc3339();
        let importer = UsageImporter::new(connection);

        let Some(runner) = self.resolve() else {
            return self.fail(
                &importer,
                &started_at,
                None,
                Error::UsageUnavailable(self.unavailable_reason()),
            );
        };

        let source_version = match probe_version(&runner, self.executor) {
            Ok(Some((version, _))) => Some(format!("ccusage {version}")),
            Ok(None) => None,
            Err(error) => return self.fail(&importer, &started_at, Some(runner.kind), error),
        };

        let command = runner.with_arguments(&session_report_arguments());
        let output = match self.executor.run(&command) {
            Ok(output) if !output.failed() => output,
            Ok(output) => {
                let error = output.into_error(&command);
                return self.fail(&importer, &started_at, Some(runner.kind), error);
            }
            Err(error) => return self.fail(&importer, &started_at, Some(runner.kind), error),
        };

        let report: CcusageReport = match parse_report(&output.stdout) {
            Ok(report) => report,
            Err(error) => return self.fail(&importer, &started_at, Some(runner.kind), error),
        };

        let normalized = match normalize_session(&report, PRICING_MODE_AUTO) {
            Ok(normalized) => normalized,
            Err(error) => return self.fail(&importer, &started_at, Some(runner.kind), error),
        };

        importer.import(&request_from(
            &normalized,
            SOURCE_ID,
            source_version.as_deref(),
            Some(runner.kind),
            REPORT_KIND,
            &started_at,
        ))
    }
}

impl CcusageAdapter<'_> {
    /// 失败一律「先留痕，再报错」。
    fn fail<T>(
        &self,
        importer: &UsageImporter<'_>,
        started_at: &str,
        runner: Option<RunnerKind>,
        error: Error,
    ) -> Result<T> {
        importer.record_failure(&FailedImportRequest {
            source: SOURCE_ID,
            source_version: None,
            runner,
            report_kind: REPORT_KIND,
            started_at,
            error: &error.to_string(),
        })?;
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{empty_db, FakeHostProbe, ScriptedCommandRunner};

    const SESSION_FIXTURE: &str = include_str!("../../../fixtures/ccusage/session.json");
    const VERSION_OUTPUT: &str = "ccusage 20.0.24\n";

    /// 版本探测成功 + 报告调用按参数给出预设输出。
    fn executor_with_report(exit_code: i32, stdout: &str, stderr: &str) -> ScriptedCommandRunner {
        ScriptedCommandRunner::new()
            .with_output("--version", 0, VERSION_OUTPUT, "")
            .with_output("--sections", exit_code, stdout, stderr)
    }

    fn adapter<'a>(
        probe: &'a FakeHostProbe,
        executor: &'a ScriptedCommandRunner,
    ) -> CcusageAdapter<'a> {
        CcusageAdapter::new(probe, executor, None)
    }

    #[test]
    fn detect_reports_available_and_does_not_start_a_process() {
        let probe = FakeHostProbe::with(&["npx"]);
        let executor = executor_with_report(0, SESSION_FIXTURE, "");

        let source = adapter(&probe, &executor).detect();

        assert_eq!(source.status, SourceStatus::Available);
        assert_eq!(source.runner, Some(RunnerKind::ManagedNpx));
        assert_eq!(source.reason, None);
        assert_eq!(
            executor.calls(),
            Vec::<String>::new(),
            "只读探测不得起进程（npx 一次要一两秒）"
        );
        assert_eq!(source.version, None, "没起进程就不该声称知道版本");
    }

    #[test]
    fn detect_reports_unavailable_with_a_human_readable_reason() {
        let probe = FakeHostProbe::with(&[]);
        let executor = executor_with_report(0, SESSION_FIXTURE, "");

        let source = adapter(&probe, &executor).detect();

        assert_eq!(source.status, SourceStatus::Unavailable);
        assert_eq!(source.runner, None);
        let reason = source.reason.expect("必须给原因");
        assert!(reason.contains("ccusage"), "{reason}");
        assert!(reason.contains("ccusage@20.0.24"), "{reason}");
    }

    /// 能力矩阵必须逐项为真，`watch` 未实现就必须是 false（ADR-0005）。
    #[test]
    fn capabilities_are_an_honest_matrix() {
        let probe = FakeHostProbe::with(&["npx"]);
        let executor = executor_with_report(0, SESSION_FIXTURE, "");

        let capabilities = adapter(&probe, &executor).capabilities();

        assert!(capabilities.detect);
        assert!(capabilities.import);
        assert!(capabilities.reconcile);
        assert!(!capabilities.watch, "文件监听没实现，不许写 true");
    }

    #[test]
    fn version_reads_the_real_output() {
        let probe = FakeHostProbe::with(&["npx"]);
        let executor = executor_with_report(0, SESSION_FIXTURE, "");

        let version = adapter(&probe, &executor).version().expect("探测");

        assert_eq!(version.as_deref(), Some("20.0.24"));
    }

    #[test]
    fn version_reports_unavailable_when_there_is_no_runner() {
        let probe = FakeHostProbe::with(&[]);
        let executor = executor_with_report(0, SESSION_FIXTURE, "");

        let error = adapter(&probe, &executor)
            .version()
            .expect_err("必须报不可用");

        assert!(matches!(error, Error::UsageUnavailable(_)), "{error}");
    }

    #[test]
    fn import_runs_one_sections_call_and_persists_every_event() {
        let db = empty_db();
        let probe = FakeHostProbe::with(&["npx"]);
        let executor = executor_with_report(0, SESSION_FIXTURE, "");

        let outcome = adapter(&probe, &executor)
            .import(db.connection())
            .expect("导入");

        assert_eq!(outcome.import.records_seen, 7);
        assert_eq!(outcome.import.records_inserted, 7);
        assert_eq!(
            outcome.import.source_version.as_deref(),
            Some("ccusage 20.0.24"),
            "provenance 必须带上真实版本"
        );

        let calls = executor.calls();
        assert_eq!(calls.len(), 2, "一次 --version + 一次报告调用：{calls:?}");
        assert!(calls[0].contains("--version"), "{calls:?}");
        assert!(calls[1].contains("--sections"), "{calls:?}");
        assert!(calls[1].contains("--by-agent"), "{calls:?}");
        assert!(calls[1].contains("--json"), "{calls:?}");

        assert_eq!(
            UsageImporter::new(db.connection())
                .event_count()
                .expect("计数"),
            7
        );
    }

    #[test]
    fn import_reconciles_against_the_same_snapshot_totals() {
        let db = empty_db();
        let probe = FakeHostProbe::with(&["npx"]);
        let executor = executor_with_report(0, SESSION_FIXTURE, "");

        let outcome = adapter(&probe, &executor)
            .import(db.connection())
            .expect("导入");

        let reconciliation = outcome.reconciliation;
        assert_eq!(
            reconciliation.tokens_identity_holds,
            Some(true),
            "Σ事件 + 无法归因的差额 必须精确等于同快照 totals"
        );
        assert_eq!(
            reconciliation.row_total_residual, 910,
            "夹具里 opencode 那行的差额"
        );
        assert_eq!(reconciliation.cost_microunits_delta, Some(0));
        assert_eq!(reconciliation.unpriced_events, 1);
    }

    #[test]
    fn a_second_identical_import_through_the_adapter_inserts_nothing() {
        let db = empty_db();
        let probe = FakeHostProbe::with(&["npx"]);
        let executor = executor_with_report(0, SESSION_FIXTURE, "");
        let adapter = adapter(&probe, &executor);
        adapter.import(db.connection()).expect("首次导入");

        let second = adapter.import(db.connection()).expect("重复导入");

        assert_eq!(second.import.records_inserted, 0);
        assert_eq!(second.import.records_skipped, 7);
        let totals = UsageImporter::new(db.connection()).totals().expect("汇总");
        assert_eq!(totals.total_tokens, 885_763_878 - 910);
    }

    #[test]
    fn a_non_zero_exit_becomes_a_failed_import_row() {
        let db = empty_db();
        let probe = FakeHostProbe::with(&["npx"]);
        // 版本探测成功，报告调用失败（真实场景：参数不被上游支持）。
        let executor = executor_with_report(2, "", "Unknown session option\n");

        let error = adapter(&probe, &executor)
            .import(db.connection())
            .expect_err("非零退出必须报错");

        assert!(matches!(error, Error::UsageCommandFailed { .. }), "{error}");
        let importer = UsageImporter::new(db.connection());
        assert_eq!(importer.event_count().expect("计数"), 0, "不得留下事件");
        let history = importer.import_history(SOURCE_ID).expect("历史");
        assert_eq!(history.len(), 1, "失败也必须留痕");
        assert_eq!(history[0].status, crate::usage::ImportStatus::Failed);
        assert!(history[0]
            .error
            .as_deref()
            .expect("错误文本")
            .contains("Unknown session option"));
    }

    #[test]
    fn malformed_json_becomes_a_failed_import_row() {
        let db = empty_db();
        let probe = FakeHostProbe::with(&["npx"]);
        let executor = executor_with_report(0, "{\"session\": [{\"agent\":", "");

        let error = adapter(&probe, &executor)
            .import(db.connection())
            .expect_err("坏 JSON 必须报错");

        assert!(matches!(error, Error::UsageMalformed(_)), "{error}");
        let importer = UsageImporter::new(db.connection());
        assert_eq!(importer.event_count().expect("计数"), 0);
        assert_eq!(importer.import_history(SOURCE_ID).expect("历史").len(), 1);
    }

    #[test]
    fn no_runner_available_is_recorded_as_a_failed_import_not_a_crash() {
        let db = empty_db();
        let probe = FakeHostProbe::with(&[]);
        let executor = executor_with_report(0, SESSION_FIXTURE, "");

        let error = adapter(&probe, &executor)
            .import(db.connection())
            .expect_err("必须报不可用");

        assert!(matches!(error, Error::UsageUnavailable(_)), "{error}");
        let history = UsageImporter::new(db.connection())
            .import_history(SOURCE_ID)
            .expect("历史");
        assert_eq!(history.len(), 1, "不可用也要留痕");
        assert_eq!(history[0].status, crate::usage::ImportStatus::Failed);
        assert_eq!(
            executor.calls(),
            Vec::<String>::new(),
            "没有 runner 就不该起进程"
        );
    }
}
