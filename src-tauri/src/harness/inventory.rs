//! Harness 清单同步：`detect` → `reconcile` → SQLite。
//!
//! 为什么要有这条独立路径（而不是在 `list_harnesses` 里顺手写库）：
//! 见 docs/adr/0007-harness-definition-vs-installation.md。
//!
//! ```text
//! list / get / inspect   = 不修改状态
//! refresh / detect / sync / reconcile = 明确允许修改状态
//! ```
//!
//! 否则会出现「只是打开 Harnesses 页面 → detected_at 变了 → 写库 → audit 变化 →
//! watcher 触发」这种很难推理的连锁行为。
//!
//! 调用点只有三处（都明确是「同步」语义）：
//!   1. 应用启动；
//!   2. 用户显式 Refresh（IPC `refresh_harnesses`）；
//!   3. `create_session` 的 invariant guard（保证 FK 行的存在，不作为主同步路径）。

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::error::Result;
use crate::harness::registry::HarnessSummary;
use crate::harness::store::{upsert_harness_definition, upsert_harness_installation};

/// 安装行的稳定 id：`<harness_id>@<runtime_target_id>`。
///
/// 用复合稳定键而不是随机 UUID：`upsert` 不需要先查后写，
/// 且 `sessions.installation_id` 可以直接按 (harness, runtime target) 推导。
pub fn installation_id(harness_id: &str, runtime_target_id: &str) -> String {
    format!("{harness_id}@{runtime_target_id}")
}

/// 一次同步的结果，直接透给 UI 作为「刷新已生效」的证据。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileReport {
    pub harnesses: usize,
    pub installations: usize,
}

/// 把内存中的检测结果同步进 SQLite。
///
/// 前置条件：`runtime_target_id` 必须已经存在（安装行有指向它的外键）。
/// 不存在时返回**明确错误**而不是把 SQLite 的 FK 报错原样抛给上层。
pub fn reconcile_harnesses(
    conn: &Connection,
    summaries: &[HarnessSummary],
    runtime_target_id: &str,
    now: &str,
) -> Result<ReconcileReport> {
    if !summaries.is_empty() {
        ensure_runtime_target_exists(conn, runtime_target_id)?;
    }

    for summary in summaries {
        upsert_harness_definition(conn, &summary.id, &summary.display_name, now)?;
        upsert_harness_installation(conn, summary, runtime_target_id, now)?;
    }

    Ok(ReconcileReport {
        harnesses: summaries.len(),
        installations: summaries.len(),
    })
}

fn ensure_runtime_target_exists(conn: &Connection, runtime_target_id: &str) -> Result<()> {
    let exists: i64 = conn.query_row(
        "SELECT count(*) FROM runtime_targets WHERE id = ?1",
        params![runtime_target_id],
        |row| row.get(0),
    )?;

    if exists == 0 {
        return Err(crate::error::Error::InvalidInput(format!(
            "runtime target 不存在：{runtime_target_id}；同步安装前必须先创建运行目标"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{detected_codex_summary, empty_db, RUNTIME_TARGET_ID};

    fn count(db: &crate::db::Database, sql: &str) -> i64 {
        db.connection()
            .query_row(sql, [], |row| row.get(0))
            .expect("计数")
    }

    fn empty_db_with_local_target() -> crate::db::Database {
        let db = empty_db();
        crate::runtime::local::ensure_local_target(db.connection()).expect("runtime target");
        db
    }

    #[test]
    fn installation_id_is_deterministic() {
        assert_eq!(installation_id("codex", "local"), "codex@local");
        assert_eq!(
            installation_id("codex", "wsl-ubuntu"),
            "codex@wsl-ubuntu",
            "同一 Harness 在不同 runtime target 上是不同安装"
        );
    }

    /// runtime target 不存在时必须给明确错误，而不是把 SQLite 的 FK 报错抛出去。
    #[test]
    fn reconcile_reports_a_missing_runtime_target_clearly() {
        let db = empty_db();

        let error = reconcile_harnesses(
            db.connection(),
            &[detected_codex_summary()],
            RUNTIME_TARGET_ID,
            "2026-09-22T10:00:00Z",
        )
        .expect_err("缺少 runtime target 时必须失败");

        let message = error.to_string();
        assert!(
            message.contains("runtime target"),
            "错误信息要说明原因：{message}"
        );
        assert_eq!(
            count(&db, "SELECT count(*) FROM harnesses"),
            0,
            "失败时不得留下半截数据"
        );
    }

    /// **冷启动路径**：空库 + ensure runtime target + reconcile。
    #[test]
    fn reconcile_from_an_empty_database_creates_definitions_and_installations() {
        let db = empty_db_with_local_target();

        let report = reconcile_harnesses(
            db.connection(),
            &[detected_codex_summary()],
            RUNTIME_TARGET_ID,
            "2026-09-22T10:00:00Z",
        )
        .expect("同步");

        assert_eq!(report.harnesses, 1);
        assert_eq!(report.installations, 1);
        assert_eq!(count(&db, "SELECT count(*) FROM harnesses"), 1);
        assert_eq!(count(&db, "SELECT count(*) FROM harness_installations"), 1);
    }

    #[test]
    fn reconcile_is_idempotent() {
        let db = empty_db_with_local_target();
        let summaries = [detected_codex_summary()];

        reconcile_harnesses(
            db.connection(),
            &summaries,
            RUNTIME_TARGET_ID,
            "2026-09-22T10:00:00Z",
        )
        .expect("首次");
        reconcile_harnesses(
            db.connection(),
            &summaries,
            RUNTIME_TARGET_ID,
            "2026-09-22T11:00:00Z",
        )
        .expect("再次");

        assert_eq!(count(&db, "SELECT count(*) FROM harnesses"), 1);
        assert_eq!(count(&db, "SELECT count(*) FROM harness_installations"), 1);
    }

    #[test]
    fn reconcile_of_an_empty_detection_result_writes_nothing() {
        let db = empty_db();

        let report = reconcile_harnesses(
            db.connection(),
            &[],
            RUNTIME_TARGET_ID,
            "2026-09-22T10:00:00Z",
        )
        .expect("同步");

        assert_eq!(report.harnesses, 0);
        assert_eq!(count(&db, "SELECT count(*) FROM harnesses"), 0);
    }

    /// 同一 Harness 在多个 runtime target 上应各自有一条安装。
    #[test]
    fn the_same_harness_can_have_installations_on_multiple_runtime_targets() {
        let db = empty_db_with_local_target();
        db.connection()
            .execute(
                "INSERT INTO runtime_targets (id, kind, display_name, created_at)
                 VALUES ('wsl-ubuntu', 'local', 'WSL Ubuntu', '2026-09-22T10:00:00Z')",
                [],
            )
            .expect("插入第二个 runtime target");

        reconcile_harnesses(
            db.connection(),
            &[detected_codex_summary()],
            RUNTIME_TARGET_ID,
            "2026-09-22T10:00:00Z",
        )
        .expect("local");
        reconcile_harnesses(
            db.connection(),
            &[detected_codex_summary()],
            "wsl-ubuntu",
            "2026-09-22T10:00:00Z",
        )
        .expect("wsl");

        assert_eq!(
            count(&db, "SELECT count(*) FROM harnesses"),
            1,
            "定义只有一条"
        );
        assert_eq!(
            count(&db, "SELECT count(*) FROM harness_installations"),
            2,
            "两个 runtime target 各有一条安装"
        );
    }
}
