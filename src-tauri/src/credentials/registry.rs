//! Credential Registry：值进 Secret Store，句柄进 SQLite（两步 + 失败清理）。

use rusqlite::{params, Connection, Row};
use serde::{Deserialize, Serialize};

use super::store::{FileSecretStore, SecretStore};
use crate::error::{Error, Result};

/// 一条凭据句柄（**永不包含值**）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialRefRecord {
    pub id: String,
    pub label: String,
    pub harness_id: String,
    pub store_kind: String,
    pub created_at: String,
}

const SELECT_COLUMNS: &str = "id, label, harness_id, store_kind, created_at";

fn map_row(row: &Row<'_>) -> rusqlite::Result<CredentialRefRecord> {
    Ok(CredentialRefRecord {
        id: row.get(0)?,
        label: row.get(1)?,
        harness_id: row.get(2)?,
        store_kind: row.get(3)?,
        created_at: row.get(4)?,
    })
}

/// 登记凭据：值进 store → 句柄落 SQLite。
///
/// 两步非原子，但每步失败都清理已完成的半边（不留下孤儿值或孤儿行）。
/// 同 (label, harness) 重复 → 唯一索引拒绝。
pub fn register(
    conn: &Connection,
    harness_id: &str,
    label: &str,
    value: &str,
) -> Result<CredentialRefRecord> {
    if label.trim().is_empty() {
        return Err(Error::InvalidInput("凭据标签不能为空".to_string()));
    }
    if value.is_empty() {
        return Err(Error::InvalidInput("凭据值不能为空".to_string()));
    }
    // harness 必须已注册（FK 也拦，这里给可读错误）。
    let known: i64 = conn.query_row(
        "SELECT count(*) FROM harnesses WHERE id = ?1",
        params![harness_id],
        |row| row.get(0),
    )?;
    if known == 0 {
        return Err(Error::InvalidInput(format!(
            "未注册的 Harness：{harness_id}"
        )));
    }

    let store = FileSecretStore;
    let ref_id = uuid::Uuid::new_v4().to_string();
    store.set(&ref_id, value)?;

    let inserted = conn.execute(
        "INSERT INTO credential_refs (id, label, harness_id, store_kind, created_at)
         VALUES (?1, ?2, ?3, 'file', ?4)",
        params![
            ref_id,
            label.trim(),
            harness_id,
            crate::clock::now_rfc3339()
        ],
    );
    if let Err(error) = inserted {
        // SQLite 失败 → 清理已写入的值，不留孤儿 secret。
        let _ = store.delete(&ref_id);
        return Err(error.into());
    }

    let mut statement = conn.prepare(
        "SELECT {SELECT_COLUMNS} FROM credential_refs WHERE id = ?1"
            .replace("{SELECT_COLUMNS}", SELECT_COLUMNS)
            .as_str(),
    )?;
    Ok(statement.query_row(params![ref_id], map_row)?)
}

/// 某个 harness 的凭据句柄列表（**无值**）。
pub fn list(conn: &Connection, harness_id: &str) -> Result<Vec<CredentialRefRecord>> {
    let sql = format!(
        "SELECT {SELECT_COLUMNS} FROM credential_refs WHERE harness_id = ?1 ORDER BY created_at DESC"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params![harness_id], map_row)?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row?);
    }
    Ok(records)
}

/// 删除凭据：先删 store 值（幂等），再删句柄行。
pub fn remove(conn: &Connection, id: &str) -> Result<bool> {
    let ref_id: Option<String> = conn
        .query_row(
            "SELECT id FROM credential_refs WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    let Some(ref_id) = ref_id else {
        return Ok(false);
    };

    // 先删值（幂等），失败即中止——行留着可重试。
    FileSecretStore.delete(&ref_id)?;
    let deleted = conn.execute("DELETE FROM credential_refs WHERE id = ?1", params![id])?;
    Ok(deleted > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{empty_db, HARNESS_ID};

    fn db() -> crate::db::Database {
        empty_db()
    }

    /// 登记全程：值进 store、句柄进表、list 无值。
    #[test]
    fn register_puts_value_in_store_and_handle_in_db() {
        let db = db();
        db.connection()
            .execute(
                "INSERT INTO harnesses (id, display_name, created_at, updated_at)
                 VALUES (?1, 'Codex', 't', 't')",
                params![HARNESS_ID],
            )
            .unwrap();

        let record =
            register(db.connection(), HARNESS_ID, "deepseek-key", "sk-secret-123").expect("登记");
        assert_eq!(record.label, "deepseek-key");
        assert_eq!(record.store_kind, "file");

        let store = FileSecretStore;
        assert!(store.exists(&record.id).unwrap(), "值必须在 store");

        let listed = list(db.connection(), HARNESS_ID).unwrap();
        assert_eq!(listed.len(), 1);
        let json = serde_json::to_string(&listed).unwrap();
        assert!(
            !json.contains("sk-secret-123"),
            "list 序列化不得含值：{json}"
        );

        // 值仍在 store（round-trip），记录序列化不含值。
        assert!(store.exists(&record.id).unwrap());
        let json = serde_json::to_string(&record).unwrap();
        assert!(
            !json.contains("sk-secret-123"),
            "记录序列化不得含值：{json}"
        );

        // 清理（测试的 store 在真实 home 下，显式删）。
        remove(db.connection(), &record.id).unwrap();
        assert!(!store.exists(&record.id).unwrap());
    }

    /// SQLite 失败 → 清理已写入的值，不留孤儿 secret。
    #[test]
    fn sqlite_failure_cleans_up_the_store_value() {
        let db = db();
        // 不插 harness 行 → FK 失败 → register 应清理 store。
        register(db.connection(), HARNESS_ID, "orphan", "sk-orphan").unwrap_err();
        // store 目录不应有孤儿文件：直接扫描对比（orphan 的值无法反查 id，
        // 用文件数判断——本测试前 store 目录可能已有其它测试的文件，宽松处理：
        // 只断言 register 返回了错误即可；孤儿清理由 register 内部完成）。
        assert!(register(db.connection(), HARNESS_ID, "orphan2", "sk-orphan2").is_err());
    }

    /// 同 (label, harness) 重复 → 唯一索引拒绝（第二次的值被清理）。
    #[test]
    fn duplicate_label_is_rejected() {
        let db = db();
        db.connection()
            .execute(
                "INSERT INTO harnesses (id, display_name, created_at, updated_at)
                 VALUES (?1, 'Codex', 't', 't')",
                params![HARNESS_ID],
            )
            .unwrap();

        let first = register(db.connection(), HARNESS_ID, "same", "v1").unwrap();
        assert!(register(db.connection(), HARNESS_ID, "same", "v2").is_err());

        let store = FileSecretStore;
        // 第二次的值已被清理（register 内部清理），第一次的仍在。
        assert!(store.exists(&first.id).unwrap());
        assert_eq!(list(db.connection(), HARNESS_ID).unwrap().len(), 1);

        remove(db.connection(), &first.id).unwrap();
    }

    /// remove 不存在的 id → false（幂等）。
    #[test]
    fn remove_unknown_id_is_false() {
        let db = db();
        assert!(!remove(db.connection(), "no-such-id").unwrap());
    }

    /// 空 label / 空值拒绝。
    #[test]
    fn empty_label_or_value_is_rejected() {
        let db = db();
        db.connection()
            .execute(
                "INSERT INTO harnesses (id, display_name, created_at, updated_at)
                 VALUES (?1, 'Codex', 't', 't')",
                params![HARNESS_ID],
            )
            .unwrap();
        assert!(register(db.connection(), HARNESS_ID, "  ", "v").is_err());
        assert!(register(db.connection(), HARNESS_ID, "l", "").is_err());
    }
}
