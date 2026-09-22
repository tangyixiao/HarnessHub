//! Harness **定义**与**安装**的持久化。
//!
//! 两张表的分工（见 docs/adr/0007、migration 0002）：
//!
//! ```text
//! harnesses             = Harness 定义 / 注册表身份（codex / claude / …）
//! harness_installations = 某个 runtime_target 上检测到的真实安装
//!                         （binary 路径、版本、可用性、最后可见时间）
//! ```
//!
//! 为什么必须拆开：后续 Runtime Target 会有 Local / WSL / SSH / Container，
// 同一个 Codex 在多台机器上可能是不同版本、不同路径。
//!
//! 写入只发生在 [`crate::harness::inventory`] 的同步路径里；查询命令不得写库。

use rusqlite::{params, Connection};

use crate::error::{Error, Result};
use crate::harness::registry::HarnessSummary;

/// 写入（或更新）Harness 定义行。
///
/// 幂等：重复调用只更新 `display_name` / `updated_at`，`created_at` 保持首次值。
pub fn upsert_harness_definition(
    conn: &Connection,
    harness_id: &str,
    display_name: &str,
    now: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO harnesses (id, display_name, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT (id) DO UPDATE SET
            display_name = excluded.display_name,
            updated_at   = excluded.updated_at",
        params![harness_id, display_name, now],
    )?;

    Ok(())
}

/// 写入（或更新）某个 runtime target 上的安装行，返回 installation id。
///
/// 幂等：`first_detected_at` 只在首次插入时写入，`last_seen_at` 每次都刷新 ——
/// 这样「这个安装什么时候第一次被发现」和「最后一次看到它是什么时候」不会混淆。
pub fn upsert_harness_installation(
    conn: &Connection,
    summary: &HarnessSummary,
    runtime_target_id: &str,
    now: &str,
) -> Result<String> {
    let installation_id =
        crate::harness::inventory::installation_id(&summary.id, runtime_target_id);

    let capabilities_json = serde_json::to_string(&summary.capabilities)
        .map_err(|error| Error::InvalidInput(format!("能力矩阵序列化失败：{error}")))?;
    let data_paths_json = serde_json::to_string(&summary.data_paths)
        .map_err(|error| Error::InvalidInput(format!("数据目录序列化失败：{error}")))?;

    // 检测是确定性的：installed 为真即 available，否则 unavailable。
    // `unknown` 留给「连检测都没跑成」的情况（目前不会产生）。
    let availability = if summary.installed {
        "available"
    } else {
        "unavailable"
    };

    conn.execute(
        "INSERT INTO harness_installations (
            id, harness_id, runtime_target_id, binary_path, version,
            capabilities_json, data_paths_json, availability, first_detected_at, last_seen_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
         ON CONFLICT (harness_id, runtime_target_id) DO UPDATE SET
            binary_path       = excluded.binary_path,
            version           = excluded.version,
            capabilities_json = excluded.capabilities_json,
            data_paths_json   = excluded.data_paths_json,
            availability      = excluded.availability,
            last_seen_at      = excluded.last_seen_at",
        params![
            installation_id,
            summary.id,
            runtime_target_id,
            summary.binary_path,
            summary.version,
            capabilities_json,
            data_paths_json,
            availability,
            now,
        ],
    )?;

    Ok(installation_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::adapter::HarnessCapabilities;
    use crate::test_support::{empty_db, RUNTIME_TARGET_ID};

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

    fn seed_dependencies(db: &crate::db::Database) {
        upsert_harness_definition(db.connection(), "codex", "Codex", "2026-09-22T09:00:00Z")
            .expect("定义");
        crate::runtime::local::ensure_local_target(db.connection()).expect("runtime target");
    }

    #[test]
    fn definition_upsert_is_idempotent_and_keeps_created_at() {
        let db = empty_db();
        upsert_harness_definition(db.connection(), "codex", "Codex", "2026-09-22T09:00:00Z")
            .expect("首次");
        upsert_harness_definition(
            db.connection(),
            "codex",
            "Codex CLI",
            "2026-09-22T10:00:00Z",
        )
        .expect("再次");

        let (count, display_name, created_at, updated_at): (i64, String, String, String) = db
            .connection()
            .query_row(
                "SELECT count(*) OVER (), display_name, created_at, updated_at FROM harnesses",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("读取");

        assert_eq!(count, 1, "定义不得重复插入");
        assert_eq!(display_name, "Codex CLI", "展示名应更新");
        assert_eq!(created_at, "2026-09-22T09:00:00Z");
        assert_eq!(updated_at, "2026-09-22T10:00:00Z");
    }

    #[test]
    fn installation_records_binary_version_and_availability() {
        let db = empty_db();
        seed_dependencies(&db);

        let id = upsert_harness_installation(
            db.connection(),
            &summary(true, Some("0.152.1")),
            RUNTIME_TARGET_ID,
            "2026-09-22T10:00:00Z",
        )
        .expect("写入安装");

        assert_eq!(id, "codex@local");
        let (binary, version, availability): (String, String, String) = db
            .connection()
            .query_row(
                "SELECT binary_path, version, availability FROM harness_installations WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("读取");
        assert_eq!(binary, "D:/npm-global/codex.cmd");
        assert_eq!(version, "0.152.1");
        assert_eq!(availability, "available");
    }

    #[test]
    fn unavailable_installation_is_recorded_as_unavailable() {
        let db = empty_db();
        seed_dependencies(&db);

        upsert_harness_installation(
            db.connection(),
            &summary(false, None),
            RUNTIME_TARGET_ID,
            "2026-09-22T10:00:00Z",
        )
        .expect("写入");

        let availability: String = db
            .connection()
            .query_row(
                "SELECT availability FROM harness_installations",
                [],
                |row| row.get(0),
            )
            .expect("读取");
        assert_eq!(availability, "unavailable");
    }

    #[test]
    fn re_detection_updates_last_seen_but_keeps_first_detected() {
        let db = empty_db();
        seed_dependencies(&db);
        upsert_harness_installation(
            db.connection(),
            &summary(false, None),
            RUNTIME_TARGET_ID,
            "2026-09-22T10:00:00Z",
        )
        .expect("首次");

        upsert_harness_installation(
            db.connection(),
            &summary(true, Some("0.152.1")),
            RUNTIME_TARGET_ID,
            "2026-09-22T11:00:00Z",
        )
        .expect("再次");

        let (count, availability, first, last): (i64, String, String, String) = db
            .connection()
            .query_row(
                "SELECT count(*) OVER (), availability, first_detected_at, last_seen_at
                   FROM harness_installations",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("读取");

        assert_eq!(count, 1, "同一安装不得重复插入");
        assert_eq!(availability, "available", "可用性应更新");
        assert_eq!(
            first, "2026-09-22T10:00:00Z",
            "first_detected_at 必须保持首见时间"
        );
        assert_eq!(last, "2026-09-22T11:00:00Z", "last_seen_at 必须刷新");
    }

    #[test]
    fn capabilities_round_trip_in_camel_case() {
        let db = empty_db();
        seed_dependencies(&db);
        let mut detected = summary(true, None);
        detected.capabilities = HarnessCapabilities {
            launch: true,
            tool_calls: true,
            ..HarnessCapabilities::default()
        };

        let id = upsert_harness_installation(
            db.connection(),
            &detected,
            RUNTIME_TARGET_ID,
            "2026-09-22T10:00:00Z",
        )
        .expect("写入");

        let json: String = db
            .connection()
            .query_row(
                "SELECT capabilities_json FROM harness_installations WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .expect("读取");
        let value: serde_json::Value = serde_json::from_str(&json).expect("解析");
        assert_eq!(value["toolCalls"], true);
        assert!(value.get("tool_calls").is_none());
    }
}
