//! Project 表读写（Task 2 GREEN 实现）。

use crate::error::Result;
use crate::project::ProjectRecord;
use rusqlite::{params, Connection};

const SELECT_COLUMNS: &str = "id, name, repo_root, created_at, updated_at, last_opened_at";

fn map_project_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRecord> {
    Ok(ProjectRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        repo_root: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        last_opened_at: row.get(5)?,
    })
}

/// `projects` 表的读写入口（与 `SessionStore` 同构）。
pub struct ProjectStore<'conn> {
    conn: &'conn Connection,
}

impl<'conn> ProjectStore<'conn> {
    pub fn new(conn: &'conn Connection) -> Self {
        Self { conn }
    }

    /// 首次插入。repo_root 的 UNIQUE 约束会让重复登记失败 —— 由 service 决定
    /// 是 upsert 还是报错，store 不吞。
    pub fn insert(&self, id: &str, name: &str, repo_root: &str, now: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO projects (id, name, repo_root, created_at, updated_at, last_opened_at)
             VALUES (?1, ?2, ?3, ?4, ?4, NULL)",
            params![id, name, repo_root, now],
        )?;
        Ok(())
    }

    /// 以 repo_root 为唯一键的幂等登记：命中则刷新 name/updated_at/last_opened_at，
    /// 未命中则插入。返回登记后的行。
    pub fn upsert(
        &self,
        id: &str,
        name: &str,
        repo_root: &str,
        now: &str,
    ) -> Result<ProjectRecord> {
        self.conn.execute(
            "INSERT INTO projects (id, name, repo_root, created_at, updated_at, last_opened_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?4)
             ON CONFLICT (repo_root) DO UPDATE SET
                name = excluded.name,
                updated_at = excluded.updated_at,
                last_opened_at = excluded.last_opened_at",
            params![id, name, repo_root, now],
        )?;

        self.get_by_repo_root(repo_root)?
            .ok_or_else(|| crate::error::Error::InvalidInput("刚写入的项目读不回来".to_string()))
    }

    pub fn get(&self, id: &str) -> Result<Option<ProjectRecord>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {SELECT_COLUMNS} FROM projects WHERE id = ?1"
        ))?;
        let mut rows = statement.query_map(params![id], map_project_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    pub fn get_by_repo_root(&self, repo_root: &str) -> Result<Option<ProjectRecord>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {SELECT_COLUMNS} FROM projects WHERE repo_root = ?1"
        ))?;
        let mut rows = statement.query_map(params![repo_root], map_project_row)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// 最近项目：有 last_opened_at 的优先（新→旧），其余按 created_at（新→旧）；
    /// 时间相同按 id 保证确定性。
    pub fn list_recent(&self, limit: u32) -> Result<Vec<ProjectRecord>> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM projects
             ORDER BY COALESCE(last_opened_at, created_at) DESC, id ASC LIMIT ?1"
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params![i64::from(limit)], map_project_row)?;

        let mut projects = Vec::new();
        for row in rows {
            projects.push(row?);
        }
        Ok(projects)
    }

    /// 刷新 last_opened_at（不更新 name）。
    pub fn touch(&self, id: &str, now: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET last_opened_at = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        Ok(())
    }

    /// 删除，返回是否真的删了一行（幂等）。
    pub fn delete(&self, id: &str) -> Result<bool> {
        let affected = self
            .conn
            .execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        Ok(affected > 0)
    }

    pub fn count(&self) -> Result<i64> {
        let count = self
            .conn
            .query_row("SELECT count(*) FROM projects", [], |row| row.get(0))?;
        Ok(count)
    }
}
