//! Harness 检测结果的持久化。
//!
//! 为什么需要这一层：`sessions.harness_id` 有指向 `harnesses(id)` 的外键，
//! 而检测结果原本只存在于内存里的注册表中 —— 于是创建会话时会直接
//! `FOREIGN KEY constraint failed`。
//!
//! 真实宿主验证抓到了这个缺口（单元测试没抓到，因为测试夹具 `seeded_db()`
//! 替调用方把 harness 行插好了）。这里补齐生产路径上缺失的那一步。

use rusqlite::{params, Connection};

use crate::error::{Error, Result};
use crate::harness::registry::HarnessSummary;

/// 写入（或更新）一条 Harness 检测结果。
///
/// - `detected_at` 由调用方传入（用 `clock::now_rfc3339()`），便于测试注入确定时间。
/// - 幂等：同一 id 重复写入只更新元数据，`created_at` 保持首次写入的值。
pub fn upsert_harness(
    conn: &Connection,
    summary: &HarnessSummary,
    detected_at: &str,
) -> Result<()> {
    let capabilities_json = serde_json::to_string(&summary.capabilities)
        .map_err(|error| Error::InvalidInput(format!("能力矩阵序列化失败：{error}")))?;
    let data_paths_json = serde_json::to_string(&summary.data_paths)
        .map_err(|error| Error::InvalidInput(format!("数据目录序列化失败：{error}")))?;

    conn.execute(
        "INSERT INTO harnesses (
            id, display_name, installed, binary_path, version,
            capabilities_json, data_paths_json, detected_at, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?8)
         ON CONFLICT (id) DO UPDATE SET
            display_name      = excluded.display_name,
            installed         = excluded.installed,
            binary_path       = excluded.binary_path,
            version           = excluded.version,
            capabilities_json = excluded.capabilities_json,
            data_paths_json   = excluded.data_paths_json,
            detected_at       = excluded.detected_at,
            updated_at        = excluded.updated_at",
        params![
            summary.id,
            summary.display_name,
            i64::from(summary.installed),
            summary.binary_path,
            summary.version,
            capabilities_json,
            data_paths_json,
            detected_at,
        ],
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::adapter::HarnessCapabilities;
    use crate::test_support::empty_db;

    fn summary(installed: bool, version: Option<&str>) -> HarnessSummary {
        HarnessSummary {
            id: "codex".to_string(),
            display_name: "Codex".to_string(),
            installed,
            binary_path: installed.then(|| "D:/npm-global/codex.cmd".to_string()),
            version: version.map(str::to_string),
            capabilities: HarnessCapabilities::default(),
            data_paths: vec!["C:/Users/dev/.codex".to_string()],
        }
    }

    #[test]
    fn inserts_a_detected_harness() {
        let db = empty_db();

        upsert_harness(
            db.connection(),
            &summary(true, Some("0.152.1")),
            "2026-09-22T10:00:00Z",
        )
        .expect("写入");

        let (installed, version, detected_at): (i64, Option<String>, String) = db
            .connection()
            .query_row(
                "SELECT installed, version, detected_at FROM harnesses WHERE id = 'codex'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("应能读回");
        assert_eq!(installed, 1);
        assert_eq!(version.as_deref(), Some("0.152.1"));
        assert_eq!(detected_at, "2026-09-22T10:00:00Z");
    }

    #[test]
    fn capability_json_round_trips_in_camel_case() {
        let db = empty_db();
        let mut detected = summary(true, Some("0.152.1"));
        detected.capabilities = HarnessCapabilities {
            launch: true,
            tool_calls: true,
            ..HarnessCapabilities::default()
        };

        upsert_harness(db.connection(), &detected, "2026-09-22T10:00:00Z").expect("写入");

        let json: String = db
            .connection()
            .query_row(
                "SELECT capabilities_json FROM harnesses WHERE id = 'codex'",
                [],
                |row| row.get(0),
            )
            .expect("读取");
        let value: serde_json::Value = serde_json::from_str(&json).expect("解析");
        assert_eq!(value["launch"], true);
        assert_eq!(value["toolCalls"], true);
        assert!(value.get("tool_calls").is_none());
    }

    #[test]
    fn re_detection_updates_metadata_without_touching_created_at() {
        let db = empty_db();
        upsert_harness(
            db.connection(),
            &summary(false, None),
            "2026-09-22T10:00:00Z",
        )
        .expect("首次");

        upsert_harness(
            db.connection(),
            &summary(true, Some("0.152.1")),
            "2026-09-22T11:00:00Z",
        )
        .expect("再次");

        let (count, installed, version, created_at, detected_at): (
            i64,
            i64,
            Option<String>,
            String,
            String,
        ) = db
            .connection()
            .query_row(
                "SELECT count(*) OVER (), installed, version, created_at, detected_at
                   FROM harnesses WHERE id = 'codex'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .expect("读取");

        assert_eq!(count, 1, "重复检测不得插入第二行");
        assert_eq!(installed, 1, "已安装状态必须更新");
        assert_eq!(version.as_deref(), Some("0.152.1"));
        assert_eq!(
            created_at, "2026-09-22T10:00:00Z",
            "created_at 必须是首次写入时间"
        );
        assert_eq!(detected_at, "2026-09-22T11:00:00Z", "detected_at 必须更新");
    }

    #[test]
    fn upserted_harness_satisfies_the_sessions_foreign_key() {
        let db = empty_db();
        upsert_harness(
            db.connection(),
            &summary(true, None),
            "2026-09-22T10:00:00Z",
        )
        .expect("写入");
        crate::runtime::local::ensure_local_target(db.connection()).expect("runtime target");

        // 这正是真实宿主上报 FOREIGN KEY constraint failed 的那条路径。
        let service = crate::session::service::SessionService::new(db.connection());
        let record = service
            .start("codex", None, None)
            .expect("创建会话必须成功");

        assert_eq!(record.harness_id, "codex");
    }
}
