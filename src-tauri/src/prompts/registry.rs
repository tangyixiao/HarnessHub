//! Prompt CRUD（SQLite 侧）。

use rusqlite::{params, Connection, Row};
use serde::Deserialize;

use super::PromptArtifact;
use crate::error::{Error, Result};

/// 新增 / 更新 prompt 的输入。`id` 为 None = 新建。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptInput {
    pub id: Option<String>,
    pub name: String,
    pub content: String,
    /// 绑定项目（None = 全局）。
    pub project_id: Option<String>,
    pub enabled: bool,
}

const PROMPT_COLUMNS: &str = "id, name, content, project_id, enabled, created_at, updated_at";

fn map_prompt(row: &Row<'_>) -> rusqlite::Result<PromptArtifact> {
    Ok(PromptArtifact {
        id: row.get(0)?,
        name: row.get(1)?,
        content: row.get(2)?,
        project_id: row.get(3)?,
        enabled: row.get::<_, i64>(4)? != 0,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

/// 新增或更新 prompt。校验：name / content 非空；project 必须已登记。
///
/// 更新保持 `created_at` 不变；`project_id` 更新时同样校验存在性（NULL 合法）。
pub fn upsert_prompt(conn: &Connection, input: &PromptInput) -> Result<PromptArtifact> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(Error::InvalidInput("prompt 名不能为空".to_string()));
    }
    if input.content.trim().is_empty() {
        return Err(Error::InvalidInput("prompt 内容不能为空".to_string()));
    }
    if let Some(project_id) = &input.project_id {
        let known: i64 = conn.query_row(
            "SELECT count(*) FROM projects WHERE id = ?1",
            params![project_id],
            |row| row.get(0),
        )?;
        if known == 0 {
            return Err(Error::InvalidInput(format!("未登记的项目：{project_id}")));
        }
    }

    let now = crate::clock::now_rfc3339();
    // id 在新建时即生成：回查用它（input.id 在新建路径是 None）。
    let id = input
        .id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let inserted = match &input.id {
        Some(_) => conn.execute(
            "UPDATE prompt_artifacts SET name = ?2, content = ?3, project_id = ?4,
             enabled = ?5, updated_at = ?6 WHERE id = ?1",
            params![
                id,
                name,
                input.content,
                input.project_id,
                i64::from(input.enabled),
                now
            ],
        ),
        None => conn.execute(
            "INSERT INTO prompt_artifacts (id, name, content, project_id, enabled,
             created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![
                id,
                name,
                input.content,
                input.project_id,
                i64::from(input.enabled),
                now
            ],
        ),
    };

    match inserted {
        Ok(updated) if updated > 0 => {
            let mut statement = conn.prepare(&format!(
                "SELECT {PROMPT_COLUMNS} FROM prompt_artifacts WHERE id = ?1"
            ))?;
            Ok(statement.query_row(params![id], map_prompt)?)
        }
        Ok(_) => Err(Error::InvalidInput("prompt 不存在".to_string())),
        Err(error) => Err(error.into()),
    }
}

/// prompt 列表。`project_id` 过滤：Some(id) = 该项目绑定 + 全局；None = 全部。
/// **纯读**。
pub fn list_prompts(conn: &Connection, project_id: Option<&str>) -> Result<Vec<PromptArtifact>> {
    let (sql, param): (String, Vec<&str>) = match project_id {
        Some(id) => (
            format!(
                "SELECT {PROMPT_COLUMNS} FROM prompt_artifacts
                 WHERE project_id = ?1 OR project_id IS NULL
                 ORDER BY name ASC"
            ),
            vec![id],
        ),
        None => (
            format!("SELECT {PROMPT_COLUMNS} FROM prompt_artifacts ORDER BY name ASC"),
            vec![],
        ),
    };
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(param), map_prompt)?;
    let mut prompts = Vec::new();
    for row in rows {
        prompts.push(row?);
    }
    Ok(prompts)
}

/// 删除 prompt。返回是否真的删了。
pub fn remove_prompt(conn: &Connection, id: &str) -> Result<bool> {
    let deleted = conn.execute("DELETE FROM prompt_artifacts WHERE id = ?1", params![id])?;
    Ok(deleted > 0)
}

/// 某 scope（project 或全局）参与渲染的 enabled prompts，按 name 升序。
///
/// project_id = Some → 该项目绑定 + 全局；None → 仅全局。
pub fn enabled_for_scope(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<Vec<PromptArtifact>> {
    let mut prompts = list_prompts(conn, project_id)?;
    prompts.retain(|prompt| prompt.enabled);
    Ok(prompts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::empty_db;

    fn db() -> crate::db::Database {
        empty_db()
    }

    fn seed_project(db: &crate::db::Database) -> String {
        // projects 表的合法行（project registry 的形状）。
        db.connection()
            .execute(
                "INSERT INTO projects (id, name, repo_root, created_at, updated_at)
                 VALUES ('proj-1', 'Demo', 'D:/work/demo', 't', 't')",
                [],
            )
            .unwrap();
        "proj-1".to_string()
    }

    fn input(name: &str, project_id: Option<String>) -> PromptInput {
        PromptInput {
            id: None,
            name: name.to_string(),
            content: format!("# {name}\n指令正文"),
            project_id,
            enabled: true,
        }
    }

    #[test]
    fn upsert_creates_updates_and_validates() {
        let db = db();
        let project_id = seed_project(&db);
        let conn = db.connection();

        let created = upsert_prompt(conn, &input("style", Some(project_id.clone()))).unwrap();
        assert_eq!(created.name, "style");
        assert!(created.enabled);
        assert_eq!(created.project_id.as_deref(), Some(project_id.as_str()));

        // 更新：改名 + 停用。
        let update = PromptInput {
            id: Some(created.id.clone()),
            name: "style-v2".to_string(),
            content: "新内容".to_string(),
            project_id: None,
            enabled: false,
        };
        let updated = upsert_prompt(conn, &update).unwrap();
        assert_eq!(updated.id, created.id);
        assert!(!updated.enabled);
        assert!(updated.project_id.is_none());
        assert_eq!(updated.created_at, created.created_at, "created_at 不变");

        // 校验：空名 / 空内容 / 未登记项目。
        assert!(upsert_prompt(
            conn,
            &PromptInput {
                name: "  ".to_string(),
                ..input("x", None)
            }
        )
        .is_err());
        assert!(upsert_prompt(
            conn,
            &PromptInput {
                content: " ".to_string(),
                ..input("x", None)
            }
        )
        .is_err());
        assert!(upsert_prompt(conn, &input("x", Some("nope".to_string()))).is_err());
    }

    #[test]
    fn list_filters_by_project_and_orders_by_name() {
        let db = db();
        let project_id = seed_project(&db);
        let conn = db.connection();

        upsert_prompt(conn, &input("zulu", None)).unwrap();
        upsert_prompt(conn, &input("alpha", Some(project_id.clone()))).unwrap();
        upsert_prompt(conn, &input("mike", None)).unwrap();
        upsert_prompt(
            conn,
            &PromptInput {
                enabled: false,
                ..input("other-project", None)
            },
        )
        .unwrap();

        // 全部（含未启用）。
        let all = list_prompts(conn, None).unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(
            all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "mike", "other-project", "zulu"],
            "按 name 升序"
        );

        // 项目过滤 = 项目绑定 + 全局。
        let scoped = list_prompts(conn, Some(&project_id)).unwrap();
        assert_eq!(
            scoped.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "mike", "other-project", "zulu"]
        );

        // enabled 过滤（渲染输入）。
        let enabled = enabled_for_scope(conn, Some(&project_id)).unwrap();
        assert_eq!(
            enabled.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "mike", "zulu"]
        );
    }

    #[test]
    fn remove_is_idempotent_and_project_cascade_works() {
        let db = db();
        let project_id = seed_project(&db);
        let conn = db.connection();

        let bound = upsert_prompt(conn, &input("bound", Some(project_id.clone()))).unwrap();
        assert!(remove_prompt(conn, &bound.id).unwrap());
        assert!(!remove_prompt(conn, &bound.id).unwrap(), "幂等");

        let again = upsert_prompt(conn, &input("cascade", Some(project_id.clone()))).unwrap();
        conn.execute("DELETE FROM projects WHERE id = ?1", params![project_id])
            .unwrap();
        let remaining: i64 = conn
            .query_row("SELECT count(*) FROM prompt_artifacts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(remaining, 0, "项目删除级联清掉 prompt");
        let _ = again;
    }

    /// 跨 IPC 契约测试（AGENTS.md）：前端 normalize 依赖这些键。
    #[test]
    fn prompt_dtos_serialize_with_camel_case_keys() {
        let prompt = PromptArtifact {
            id: "p-1".to_string(),
            name: "style".to_string(),
            content: "# style".to_string(),
            project_id: Some("proj-1".to_string()),
            enabled: true,
            created_at: "t".to_string(),
            updated_at: "t".to_string(),
        };
        let json = serde_json::to_value(&prompt).unwrap();
        assert_eq!(json["projectId"], "proj-1");
        assert_eq!(json["createdAt"], "t");
        assert!(json.get("project_id").is_none(), "不得两套契约并存");

        let input: PromptInput = serde_json::from_value(serde_json::json!({
            "id": null,
            "name": "style",
            "content": "正文",
            "projectId": null,
            "enabled": true
        }))
        .unwrap();
        assert!(input.project_id.is_none());
        assert!(input.enabled);
    }
}
