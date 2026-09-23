//! 把归一化后的事件写进 SQLite：**幂等**、可对账、可追溯。
//!
//! 三条硬规则（ADR-0011 决策二 / 八 / 十一）：
//!
//! 1. 幂等靠 `usage_events.stable_source_key UNIQUE` + `ON CONFLICT DO UPDATE`，
//!    并且**只有内容真的变了才算一次更新** —— 重复导入必须 `inserted = 0`。
//! 2. `hub_session_id` 只在同 harness + 同 `source_session_id` 的会话**已存在**时才填，
//!    绝不因为「时间差不多」就伪造一条 Harness Hub 会话。
//! 3. 上游汇总里无法归因到模型的 token 记进 `usage_imports.unattributed_tokens`，
//!    对账因此是恒等式而不是「差不多」。
//!
//! 失败也必须留痕：解析/调用失败由 [`UsageImporter::record_failure`] 写一条
//! `status = 'failed'` 的审计行，且不留下任何半成品事件。

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::usage::{
    EventTotals, ImportStatus, NormalizedUsage, ReportTotals, RunnerKind, UsageEventDraft,
    UsageImport,
};

/// 一次导入的输入。
pub struct ImportRequest<'a> {
    pub source: &'a str,
    pub source_version: Option<&'a str>,
    pub runner: Option<RunnerKind>,
    pub report_kind: &'a str,
    pub events: &'a [UsageEventDraft],
    /// 来源自己给的汇总；`None` 表示无法对账（必须如实说明，不能假装对上了）。
    pub report_totals: Option<ReportTotals>,
    /// 上游汇总里无法归因到模型的 token 数。
    pub row_total_residual: u64,
    pub started_at: &'a str,
}

/// 一次失败的导入（解析失败 / 外部命令非零退出）。
pub struct FailedImportRequest<'a> {
    pub source: &'a str,
    pub source_version: Option<&'a str>,
    pub runner: Option<RunnerKind>,
    pub report_kind: &'a str,
    pub started_at: &'a str,
    pub error: &'a str,
}

/// 同快照对账结果。**恒等式**，不是近似值。
///
/// 跨 IPC：序列化成 camelCase（前端要能显示「差 4 微单位」这种事实）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reconciliation {
    pub events_total_tokens: u64,
    /// 来源汇总（缺失时为 `None`，表示这次无法对账）。
    pub report_total_tokens: Option<u64>,
    pub row_total_residual: u64,
    /// `Σ事件金额 - 来源金额`（独立舍入的残差，微单位）。
    pub cost_microunits_delta: Option<i64>,
    pub unpriced_events: u64,
    pub timestampless: u64,
    /// `Σ事件 + 无法归因的差额 == 来源汇总` 是否成立。
    pub tokens_identity_holds: Option<bool>,
}

/// 导入结果：审计行 + 对账。
///
/// `reconciliation` **总是**存在：即使来源没给 `totals`，调用方仍然需要
/// 「多少事件没有时间戳 / 多少事件没有金额」这些事实，只是无法比较总数而已
/// （那时 `tokens_identity_holds` 为 `None`，表示「无法对账」而不是「对上了」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOutcome {
    pub import: UsageImport,
    pub reconciliation: Reconciliation,
}

pub struct UsageImporter<'a> {
    connection: &'a Connection,
}

/// 事件的所有可变字段。
///
/// `WHERE ... IS NOT excluded....` 让「内容没变」完全不产生写入，
/// 于是 `changes() == 0` 可以精确表示「跳过」，而 `inserted = 0` 才是幂等的证据。
const UPSERT_EVENT: &str = "INSERT INTO usage_events (
        id, stable_source_key, key_version, source, report_kind, harness, source_session_id,
        hub_session_id, project_id, model, provider, input_tokens, output_tokens,
        cache_creation_tokens, cached_input_tokens, reasoning_tokens, total_tokens,
        cost_microunits, currency, currency_source, token_source, cost_source, pricing_mode,
        occurred_at, occurred_at_source, day, import_id, raw_payload, imported_at
    ) VALUES (
        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19,
        ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29
    )
    ON CONFLICT (stable_source_key) DO UPDATE SET
        hub_session_id = excluded.hub_session_id,
        project_id = excluded.project_id,
        model = excluded.model,
        provider = excluded.provider,
        input_tokens = excluded.input_tokens,
        output_tokens = excluded.output_tokens,
        cache_creation_tokens = excluded.cache_creation_tokens,
        cached_input_tokens = excluded.cached_input_tokens,
        reasoning_tokens = excluded.reasoning_tokens,
        total_tokens = excluded.total_tokens,
        cost_microunits = excluded.cost_microunits,
        currency = excluded.currency,
        currency_source = excluded.currency_source,
        token_source = excluded.token_source,
        cost_source = excluded.cost_source,
        pricing_mode = excluded.pricing_mode,
        occurred_at = excluded.occurred_at,
        occurred_at_source = excluded.occurred_at_source,
        day = excluded.day,
        import_id = excluded.import_id,
        imported_at = excluded.imported_at
    WHERE usage_events.hub_session_id IS NOT excluded.hub_session_id
       OR usage_events.model IS NOT excluded.model
       OR usage_events.provider IS NOT excluded.provider
       OR usage_events.input_tokens IS NOT excluded.input_tokens
       OR usage_events.output_tokens IS NOT excluded.output_tokens
       OR usage_events.cache_creation_tokens IS NOT excluded.cache_creation_tokens
       OR usage_events.cached_input_tokens IS NOT excluded.cached_input_tokens
       OR usage_events.reasoning_tokens IS NOT excluded.reasoning_tokens
       OR usage_events.total_tokens IS NOT excluded.total_tokens
       OR usage_events.cost_microunits IS NOT excluded.cost_microunits
       OR usage_events.currency IS NOT excluded.currency
       OR usage_events.currency_source IS NOT excluded.currency_source
       OR usage_events.token_source IS NOT excluded.token_source
       OR usage_events.cost_source IS NOT excluded.cost_source
       OR usage_events.pricing_mode IS NOT excluded.pricing_mode
       OR usage_events.occurred_at IS NOT excluded.occurred_at
       OR usage_events.occurred_at_source IS NOT excluded.occurred_at_source
       OR usage_events.day IS NOT excluded.day";

const INSERT_IMPORT: &str = "INSERT INTO usage_imports (
        id, source, source_version, runner, report_kind, status, records_seen,
        records_inserted, records_updated, records_skipped, records_timestampless,
        unattributed_tokens, started_at, completed_at, error
    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)";

const SELECT_IMPORT: &str = "SELECT id, source, source_version, runner, report_kind, status,
        records_seen, records_inserted, records_updated, records_skipped, records_timestampless,
        unattributed_tokens, started_at, completed_at, error
    FROM usage_imports";

impl<'a> UsageImporter<'a> {
    pub fn new(connection: &'a Connection) -> Self {
        Self { connection }
    }

    /// 幂等导入。返回的审计行里 `seen == inserted + updated + skipped`。
    pub fn import(&self, request: &ImportRequest<'_>) -> Result<ImportOutcome> {
        let import_id = uuid::Uuid::new_v4().to_string();
        let seen = request.events.len() as u64;
        let mut inserted = 0u64;
        let mut updated = 0u64;
        let mut skipped = 0u64;
        let mut timestampless = 0u64;

        // `unchecked_transaction` 是因为我们只持有 `&Connection`：写操作必须整体成功或整体回滚，
        // 绝不允许留下「insert 了一半」的导入。
        let transaction = self.connection.unchecked_transaction()?;

        transaction.execute(
            INSERT_IMPORT,
            params![
                import_id,
                request.source,
                request.source_version,
                request.runner.map(RunnerKind::as_str),
                request.report_kind,
                ImportStatus::Running.as_str(),
                seen as i64,
                0i64,
                0i64,
                0i64,
                0i64,
                request.row_total_residual as i64,
                request.started_at,
                Option::<String>::None,
                Option::<String>::None,
            ],
        )?;

        for event in request.events {
            let hub_session_id =
                match_hub_session(&transaction, &event.harness, &event.source_session_id)?;
            if event.occurred_at.is_none() {
                timestampless += 1;
            }
            let existed: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM usage_events WHERE stable_source_key = ?1)",
                params![event.stable_source_key],
                |row| row.get(0),
            )?;

            let written = transaction.execute(
                UPSERT_EVENT,
                params![
                    uuid::Uuid::new_v4().to_string(),
                    event.stable_source_key,
                    event.key_version,
                    event.source,
                    event.report_kind,
                    event.harness,
                    event.source_session_id,
                    hub_session_id,
                    Option::<String>::None, // project_id：v0.1 不从 usage 推导项目归属
                    event.model,
                    event.provider,
                    event.input_tokens as i64,
                    event.output_tokens as i64,
                    event.cache_creation_tokens as i64,
                    event.cached_input_tokens.map(|value| value as i64),
                    event.reasoning_tokens.map(|value| value as i64),
                    event.total_tokens as i64,
                    event.cost_microunits,
                    event.currency,
                    event.currency_source,
                    event.token_source,
                    event.cost_source,
                    event.pricing_mode,
                    event.occurred_at,
                    event.occurred_at_source,
                    event.day,
                    import_id,
                    event.raw_payload,
                    request.started_at,
                ],
            )?;

            if !existed {
                inserted += 1;
            } else if written == 1 {
                updated += 1;
            } else {
                skipped += 1;
            }
        }

        let completed_at = clock_now();
        let import = UsageImport {
            id: import_id.clone(),
            source: request.source.to_string(),
            source_version: request.source_version.map(str::to_string),
            runner: request.runner,
            report_kind: Some(request.report_kind.to_string()),
            status: ImportStatus::Succeeded,
            started_at: request.started_at.to_string(),
            completed_at: Some(completed_at.clone()),
            records_seen: seen,
            records_inserted: inserted,
            records_updated: updated,
            records_skipped: skipped,
            records_timestampless: timestampless,
            error: None,
        };

        transaction.execute(
            "UPDATE usage_imports SET status = ?1, completed_at = ?2, records_inserted = ?3,
                 records_updated = ?4, records_skipped = ?5, records_timestampless = ?6
             WHERE id = ?7",
            params![
                ImportStatus::Succeeded.as_str(),
                completed_at,
                inserted as i64,
                updated as i64,
                skipped as i64,
                timestampless as i64,
                import_id,
            ],
        )?;

        transaction.commit()?;

        Ok(ImportOutcome {
            import,
            reconciliation: reconcile(
                request.events,
                request.report_totals,
                request.row_total_residual,
            ),
        })
    }

    /// 记录一次失败的导入：**必须**留下审计行，且不留事件。
    pub fn record_failure(&self, request: &FailedImportRequest<'_>) -> Result<UsageImport> {
        let import = UsageImport {
            id: uuid::Uuid::new_v4().to_string(),
            source: request.source.to_string(),
            source_version: request.source_version.map(str::to_string),
            runner: request.runner,
            report_kind: Some(request.report_kind.to_string()),
            status: ImportStatus::Failed,
            started_at: request.started_at.to_string(),
            completed_at: Some(clock_now()),
            records_seen: 0,
            records_inserted: 0,
            records_updated: 0,
            records_skipped: 0,
            records_timestampless: 0,
            error: Some(request.error.to_string()),
        };

        self.connection.execute(
            INSERT_IMPORT,
            params![
                import.id,
                import.source,
                import.source_version,
                import.runner.map(RunnerKind::as_str),
                import.report_kind,
                import.status.as_str(),
                0i64,
                0i64,
                0i64,
                0i64,
                0i64,
                0i64,
                import.started_at,
                import.completed_at,
                import.error,
            ],
        )?;

        Ok(import)
    }

    /// 从数据库重新汇总事件（对账与 UI 都只读这里，不信任内存里的中间值）。
    pub fn totals(&self) -> Result<EventTotals> {
        Ok(self.connection.query_row(
            "SELECT
                 COALESCE(SUM(input_tokens), 0),
                 COALESCE(SUM(output_tokens), 0),
                 COALESCE(SUM(cache_creation_tokens), 0),
                 COALESCE(SUM(cached_input_tokens), 0),
                 COALESCE(SUM(total_tokens), 0),
                 COALESCE(SUM(cost_microunits), 0),
                 COUNT(*) FILTER (WHERE cost_microunits IS NULL),
                 COUNT(*) FILTER (WHERE occurred_at IS NULL)
             FROM usage_events",
            [],
            |row| {
                Ok(EventTotals {
                    input_tokens: row.get::<_, i64>(0)? as u64,
                    output_tokens: row.get::<_, i64>(1)? as u64,
                    cache_creation_tokens: row.get::<_, i64>(2)? as u64,
                    cached_input_tokens: row.get::<_, i64>(3)? as u64,
                    total_tokens: row.get::<_, i64>(4)? as u64,
                    cost_microunits: row.get(5)?,
                    events_without_cost: row.get::<_, i64>(6)? as usize,
                    timestampless: row.get::<_, i64>(7)? as usize,
                })
            },
        )?)
    }

    /// 事件总数。
    pub fn event_count(&self) -> Result<u64> {
        let count: i64 =
            self.connection
                .query_row("SELECT COUNT(*) FROM usage_events", [], |row| row.get(0))?;
        Ok(count as u64)
    }

    /// 某个来源的导入历史（新的在前）。
    pub fn import_history(&self, source: &str) -> Result<Vec<UsageImport>> {
        let mut statement = self.connection.prepare(&format!(
            "{SELECT_IMPORT} WHERE source = ?1 ORDER BY started_at DESC, rowid DESC"
        ))?;
        let rows = statement.query_map(params![source], row_to_import)?;
        let mut history = Vec::new();
        for row in rows {
            history.push(row?);
        }
        Ok(history)
    }
}

/// `hub_session_id` 只在**可证明**对应时才填（ADR-0011 决策八）。
///
/// 依据是 `(harness_id, source_session_id)` 唯一索引 —— 同一个 harness 的同一个外部会话 id。
/// 时间窗口相近、日期相同都不是证据。
fn match_hub_session(
    connection: &Connection,
    harness: &str,
    source_session_id: &str,
) -> Result<Option<String>> {
    let found = connection
        .query_row(
            "SELECT hub_session_id FROM sessions
             WHERE harness_id = ?1 AND source_session_id = ?2",
            params![harness, source_session_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(found)
}

fn row_to_import(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageImport> {
    let status: String = row.get(5)?;
    let runner: Option<String> = row.get(3)?;
    Ok(UsageImport {
        id: row.get(0)?,
        source: row.get(1)?,
        source_version: row.get(2)?,
        runner: runner.as_deref().and_then(RunnerKind::parse),
        report_kind: row.get(4)?,
        status: match status.as_str() {
            "running" => ImportStatus::Running,
            "succeeded" => ImportStatus::Succeeded,
            // CHECK 约束已经限制了取值；真出现未知值说明库被外部改了，按失败处理更安全。
            _ => ImportStatus::Failed,
        },
        records_seen: row.get::<_, i64>(6)? as u64,
        records_inserted: row.get::<_, i64>(7)? as u64,
        records_updated: row.get::<_, i64>(8)? as u64,
        records_skipped: row.get::<_, i64>(9)? as u64,
        records_timestampless: row.get::<_, i64>(10)? as u64,
        started_at: row.get(12)?,
        completed_at: row.get(13)?,
        error: row.get(14)?,
    })
}

/// 同快照对账：**恒等式** + 必须被打印出来的金额残差。
fn reconcile(
    events: &[UsageEventDraft],
    totals: Option<ReportTotals>,
    row_total_residual: u64,
) -> Reconciliation {
    let summed = EventTotals::from_drafts(events);
    let cost_delta = totals
        .and_then(|totals| totals.cost_microunits)
        .map(|report_cost| summed.cost_microunits - report_cost);

    Reconciliation {
        events_total_tokens: summed.total_tokens,
        report_total_tokens: totals.map(|totals| totals.total_tokens),
        row_total_residual,
        cost_microunits_delta: cost_delta,
        unpriced_events: summed.events_without_cost as u64,
        timestampless: summed.timestampless as u64,
        tokens_identity_holds: totals
            .map(|totals| summed.total_tokens + row_total_residual == totals.total_tokens),
    }
}

/// 当前时间的 RFC3339 UTC。放在这里而不是 `clock`，是为了让导入记录只有一个时间来源。
fn clock_now() -> String {
    crate::clock::now_rfc3339()
}

/// 归一化结果 → 导入请求（省去调用方到处重复拼字段）。
pub fn request_from<'a>(
    normalized: &'a NormalizedUsage,
    source: &'a str,
    source_version: Option<&'a str>,
    runner: Option<RunnerKind>,
    report_kind: &'a str,
    started_at: &'a str,
) -> ImportRequest<'a> {
    ImportRequest {
        source,
        source_version,
        runner,
        report_kind,
        events: &normalized.events,
        report_totals: normalized.totals,
        row_total_residual: normalized.row_total_residual,
        started_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn draft(key: &str, harness: &str, period: &str, model: &str, total: u64) -> UsageEventDraft {
        UsageEventDraft {
            stable_source_key: key.to_string(),
            key_version: 1,
            source: "ccusage".to_string(),
            report_kind: "session".to_string(),
            harness: harness.to_string(),
            source_session_id: period.to_string(),
            model: model.to_string(),
            provider: None,
            input_tokens: total,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cached_input_tokens: Some(0),
            reasoning_tokens: None,
            total_tokens: total,
            cost_microunits: Some(1_000),
            currency: Some("USD".to_string()),
            currency_source: Some("ccusage_contract".to_string()),
            token_source: "ccusage_source_log".to_string(),
            cost_source: Some("ccusage_computed".to_string()),
            pricing_mode: Some("auto".to_string()),
            occurred_at: Some("2026-09-01T00:00:00Z".to_string()),
            occurred_at_source: "source_record".to_string(),
            day: Some("2026-09-01".to_string()),
            raw_payload: None,
        }
    }

    fn three_events() -> Vec<UsageEventDraft> {
        vec![
            draft("key-a", "codex", "rollout-a", "gpt-5.6-sol", 100),
            draft("key-b", "codex", "rollout-a", "gpt-5.6-terra", 200),
            draft("key-c", "claude", "session-c", "deepseek-v4-pro", 300),
        ]
    }

    fn report_totals(total_tokens: u64, cost_microunits: Option<i64>) -> ReportTotals {
        ReportTotals {
            input_tokens: total_tokens,
            output_tokens: 0,
            cache_creation_tokens: 0,
            cached_input_tokens: 0,
            total_tokens,
            cost_microunits,
            unpriced_models: 0,
        }
    }

    fn request<'a>(
        events: &'a [UsageEventDraft],
        totals: Option<ReportTotals>,
        residual: u64,
    ) -> ImportRequest<'a> {
        ImportRequest {
            source: "ccusage",
            source_version: Some("ccusage 20.0.24"),
            runner: Some(RunnerKind::ManagedNpx),
            report_kind: "session",
            events,
            report_totals: totals,
            row_total_residual: residual,
            started_at: "2026-09-22T10:00:00Z",
        }
    }

    fn importer(db: &Database) -> UsageImporter<'_> {
        UsageImporter::new(db.connection())
    }

    #[test]
    fn first_import_inserts_every_event() {
        let db = crate::test_support::empty_db();
        let events = three_events();

        let outcome = importer(&db)
            .import(&request(&events, Some(report_totals(600, Some(3_000))), 0))
            .expect("首次导入");

        assert_eq!(outcome.import.records_seen, 3);
        assert_eq!(outcome.import.records_inserted, 3);
        assert_eq!(outcome.import.records_updated, 0);
        assert_eq!(outcome.import.records_skipped, 0);
        assert_eq!(outcome.import.status, ImportStatus::Succeeded);
        assert_eq!(importer(&db).event_count().expect("计数"), 3);
    }

    /// 幂等的核心断言：**token 不得因为重复导入而增长**。
    #[test]
    fn identical_second_import_inserts_nothing_and_does_not_grow_totals() {
        let db = crate::test_support::empty_db();
        let events = three_events();
        importer(&db)
            .import(&request(&events, Some(report_totals(600, Some(3_000))), 0))
            .expect("首次导入");
        let before = importer(&db).totals().expect("首次汇总");

        let outcome = importer(&db)
            .import(&request(&events, Some(report_totals(600, Some(3_000))), 0))
            .expect("重复导入");

        assert_eq!(outcome.import.records_inserted, 0);
        assert_eq!(outcome.import.records_updated, 0);
        assert_eq!(outcome.import.records_skipped, 3);
        let after = importer(&db).totals().expect("二次汇总");
        assert_eq!(after, before, "重复导入不得改变任何汇总");
        assert_eq!(after.total_tokens, 600);
        assert_eq!(importer(&db).event_count().expect("计数"), 3);
    }

    /// 上游改了数字：原地更新，**不新增一行**（否则总量翻倍）。
    #[test]
    fn changed_upstream_record_updates_in_place_instead_of_duplicating() {
        let db = crate::test_support::empty_db();
        let first = three_events();
        importer(&db)
            .import(&request(&first, Some(report_totals(600, Some(3_000))), 0))
            .expect("首次导入");

        let mut changed = three_events();
        changed[0].total_tokens = 150;
        changed[0].input_tokens = 150;
        let outcome = importer(&db)
            .import(&request(&changed, Some(report_totals(650, Some(3_000))), 0))
            .expect("再次导入");

        assert_eq!(outcome.import.records_inserted, 0);
        assert_eq!(outcome.import.records_updated, 1);
        assert_eq!(outcome.import.records_skipped, 2);
        assert_eq!(importer(&db).event_count().expect("计数"), 3);
        assert_eq!(importer(&db).totals().expect("汇总").total_tokens, 650);
    }

    #[test]
    fn each_import_writes_an_audit_row_with_provenance() {
        let db = crate::test_support::empty_db();
        let events = three_events();

        let outcome = importer(&db)
            .import(&request(
                &events,
                Some(report_totals(600, Some(3_000))),
                910,
            ))
            .expect("导入");

        assert_eq!(outcome.import.source, "ccusage");
        assert_eq!(
            outcome.import.source_version.as_deref(),
            Some("ccusage 20.0.24")
        );
        assert_eq!(outcome.import.runner, Some(RunnerKind::ManagedNpx));
        assert!(outcome.import.completed_at.is_some(), "必须记完成时间");

        let history = importer(&db).import_history("ccusage").expect("历史");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, outcome.import.id);
        assert_eq!(history[0].records_seen, 3);
    }

    #[test]
    fn unattributed_tokens_are_persisted_on_the_import_row() {
        let db = crate::test_support::empty_db();
        let events = three_events();

        importer(&db)
            .import(&request(&events, Some(report_totals(1_510, None)), 910))
            .expect("导入");

        let stored: i64 = db
            .connection()
            .query_row(
                "SELECT unattributed_tokens FROM usage_imports LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("读取");
        assert_eq!(stored, 910);
    }

    #[test]
    fn a_failed_import_is_recorded_without_leaving_events() {
        let db = crate::test_support::empty_db();

        let import = importer(&db)
            .record_failure(&FailedImportRequest {
                source: "ccusage",
                source_version: None,
                runner: Some(RunnerKind::ManagedNpx),
                report_kind: "session",
                started_at: "2026-09-22T10:00:00Z",
                error: "外部命令失败（退出码 2）：Unknown session option '--nope'",
            })
            .expect("失败也要留痕");

        assert_eq!(import.status, ImportStatus::Failed);
        assert!(import.error.is_some());
        assert_eq!(
            importer(&db).event_count().expect("计数"),
            0,
            "不得留下半成品"
        );
        assert_eq!(
            importer(&db).import_history("ccusage").expect("历史").len(),
            1
        );
    }

    #[test]
    fn timestampless_events_are_counted_and_persisted_as_null() {
        let db = crate::test_support::empty_db();
        let mut events = three_events();
        events[1].occurred_at = None;
        events[1].occurred_at_source = "unavailable".to_string();
        events[1].day = None;

        let outcome = importer(&db)
            .import(&request(&events, Some(report_totals(600, None)), 0))
            .expect("导入");

        assert_eq!(outcome.import.records_timestampless, 1);
        assert_eq!(importer(&db).totals().expect("汇总").timestampless, 1);
        let stored: Option<String> = db
            .connection()
            .query_row(
                "SELECT occurred_at FROM usage_events WHERE stable_source_key = 'key-b'",
                [],
                |row| row.get(0),
            )
            .expect("读取");
        assert_eq!(stored, None, "不得用导入时间冒充发生时间");
    }

    /// 冷启动路径：**空库**里历史 usage 不得凭空造出 Harness Hub 会话。
    #[test]
    fn historical_sessions_never_fabricate_a_hub_session() {
        let db = crate::test_support::empty_db();
        let events = three_events();

        importer(&db)
            .import(&request(&events, Some(report_totals(600, None)), 0))
            .expect("导入");

        let linked: i64 = db
            .connection()
            .query_row(
                "SELECT count(*) FROM usage_events WHERE hub_session_id IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .expect("计数");
        let sessions: i64 = db
            .connection()
            .query_row("SELECT count(*) FROM sessions", [], |row| row.get(0))
            .expect("计数");

        assert_eq!(linked, 0, "没有可证明的对应关系就必须是 NULL");
        assert_eq!(sessions, 0, "不得反向生成会话");
    }

    #[test]
    fn hub_session_is_linked_only_when_provably_matched() {
        let db = crate::test_support::seeded_db();
        let events = three_events();
        db.connection()
            .execute(
                "INSERT INTO sessions
                    (hub_session_id, harness_id, runtime_target_id, source_session_id, status,
                     launch_mode, started_at, created_at, updated_at)
                 VALUES ('hub-1', 'codex', 'local', 'rollout-a', 'exited', 'terminal',
                         '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z')",
                [],
            )
            .expect("写入会话");

        importer(&db)
            .import(&request(&events, Some(report_totals(600, None)), 0))
            .expect("导入");

        let linked: Vec<(String, Option<String>)> = db
            .connection()
            .prepare("SELECT stable_source_key, hub_session_id FROM usage_events ORDER BY stable_source_key")
            .expect("prepare")
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .map(|row| row.expect("row"))
            .collect();

        assert_eq!(
            linked,
            vec![
                ("key-a".to_string(), Some("hub-1".to_string())),
                ("key-b".to_string(), Some("hub-1".to_string())),
                // 同一个 period 但 harness 不同 —— 不是同一个会话，不得硬连。
                ("key-c".to_string(), None),
            ]
        );
    }

    #[test]
    fn reconciliation_is_an_identity_over_tokens_and_reports_the_cost_residual() {
        let db = crate::test_support::empty_db();
        let events = three_events();

        let outcome = importer(&db)
            .import(&request(
                &events,
                Some(report_totals(1_510, Some(2_996))),
                910,
            ))
            .expect("导入");

        let reconciliation = outcome.reconciliation;
        assert_eq!(reconciliation.events_total_tokens, 600);
        assert_eq!(reconciliation.row_total_residual, 910);
        assert_eq!(reconciliation.report_total_tokens, Some(1_510));
        assert_eq!(
            reconciliation.tokens_identity_holds,
            Some(true),
            "600 + 910 == 1510"
        );
        assert_eq!(
            reconciliation.cost_microunits_delta,
            Some(4),
            "事件金额合计 3004，来源 2996，残差必须被打印出来"
        );
    }

    #[test]
    fn reconciliation_is_absent_when_the_source_has_no_totals() {
        let db = crate::test_support::empty_db();
        let events = three_events();

        let outcome = importer(&db)
            .import(&request(&events, None, 0))
            .expect("导入");

        let reconciliation = outcome.reconciliation;
        assert_eq!(reconciliation.report_total_tokens, None);
        assert_eq!(
            reconciliation.tokens_identity_holds, None,
            "无法对账就要说无法对账"
        );
    }

    #[test]
    fn an_import_with_no_events_is_a_successful_empty_import() {
        let db = crate::test_support::empty_db();

        let outcome = importer(&db)
            .import(&request(&[], Some(ReportTotals::default()), 0))
            .expect("空的导入不是错误");

        assert_eq!(outcome.import.status, ImportStatus::Succeeded);
        assert_eq!(outcome.import.records_seen, 0);
        assert_eq!(importer(&db).event_count().expect("计数"), 0);
        assert_eq!(
            importer(&db).import_history("ccusage").expect("历史").len(),
            1
        );
    }

    /// v0.1 **不做破坏性同步**：上游不再报的会话不会被删掉。
    ///
    /// 理由：ccusage 的调用范围（时间窗/agent）随时可能变窄，把「这次没看到」
    /// 当成「不存在了」会静默删掉用户的历史。唯一的真相来源是数据库，不是某一次调用。
    ///
    /// 注意这里断言的是**本次导入自身的口径**：这次只覆盖 2 个事件、报告也只汇总这 2 个，
    /// 所以恒等式成立。数据库里仍留着第 3 条历史事件 —— 那是刻意的。
    #[test]
    fn never_deletes_events_the_source_stopped_reporting() {
        let db = crate::test_support::empty_db();
        let first = three_events();
        importer(&db)
            .import(&request(&first, Some(report_totals(600, None)), 0))
            .expect("首次导入");

        let narrowed = &three_events()[..2];
        let outcome = importer(&db)
            .import(&request(narrowed, Some(report_totals(300, None)), 0))
            .expect("第二次导入");

        assert_eq!(outcome.import.records_skipped, 2);
        assert_eq!(
            importer(&db).event_count().expect("计数"),
            3,
            "不得删除历史事件"
        );
        assert_eq!(
            outcome.reconciliation.tokens_identity_holds,
            Some(true),
            "本次导入自身与其同快照 totals 必须一致"
        );
        assert_eq!(
            importer(&db).totals().expect("库内汇总").total_tokens,
            600,
            "库内仍是完整的 600，而不是被这次窄范围调用削掉"
        );
    }

    /// 报告汇总与明细对不上时必须**如实说对不上**，不允许静默放过。
    #[test]
    fn reconciliation_reports_false_when_the_report_totals_disagree() {
        let db = crate::test_support::empty_db();
        let events = three_events();

        let outcome = importer(&db)
            .import(&request(&events, Some(report_totals(1_510, None)), 0))
            .expect("导入");

        let reconciliation = outcome.reconciliation;
        assert_eq!(reconciliation.events_total_tokens, 600);
        assert_eq!(
            reconciliation.tokens_identity_holds,
            Some(false),
            "600 + 0 != 1510，必须报告对不上"
        );
    }

    /// 跨 IPC 契约：对账对象的键名与形状（前端靠它显示差异）。
    #[test]
    fn reconciliation_serializes_with_camel_case_keys() {
        let db = crate::test_support::empty_db();
        let events = three_events();
        let outcome = importer(&db)
            .import(&request(
                &events,
                Some(report_totals(1_510, Some(2_996))),
                910,
            ))
            .expect("导入");

        let json = serde_json::to_value(&outcome.reconciliation).expect("序列化");

        assert_eq!(json["eventsTotalTokens"], 600);
        assert_eq!(json["reportTotalTokens"], 1_510);
        assert_eq!(json["rowTotalResidual"], 910);
        assert_eq!(json["costMicrounitsDelta"], 4);
        assert_eq!(json["unpricedEvents"], 0);
        assert_eq!(json["timestampless"], 0);
        assert_eq!(json["tokensIdentityHolds"], true);
        assert!(
            json.get("events_total_tokens").is_none(),
            "不得两套契约并存"
        );
    }
}
