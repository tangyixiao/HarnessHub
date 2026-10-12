//! Project registry 服务（Task 2 GREEN 实现）：repo root 检测 + 幂等登记。

use crate::error::{Error, Result};
use crate::project::git_root::GitRootProbe;
use crate::project::store::ProjectStore;
use crate::project::{ProjectRecord, RegisterRequest};
use crate::session::store::SessionRecord;
use std::path::PathBuf;
use std::sync::Arc;

pub struct ProjectService<'conn> {
    conn: &'conn rusqlite::Connection,
    probe: Arc<dyn GitRootProbe>,
}

impl<'conn> ProjectService<'conn> {
    pub fn new(conn: &'conn rusqlite::Connection, probe: Arc<dyn GitRootProbe>) -> Self {
        Self { conn, probe }
    }

    /// 登记一个本地目录：解析 repo root，以 root 为唯一键幂等 upsert。
    pub fn register(&self, request: &RegisterRequest) -> Result<ProjectRecord> {
        let path = PathBuf::from(&request.path);
        let root = self.probe.repo_root(&path)?.ok_or_else(|| {
            Error::InvalidInput(format!(
                "「{}」不是有效的本地 Git 仓库：找不到 repo root",
                request.path
            ))
        })?;

        let name = request.name.clone().unwrap_or_else(|| default_name(&root));
        let now = crate::clock::now_rfc3339();

        ProjectStore::new(self.conn).upsert(&uuid::Uuid::new_v4().to_string(), &name, &root, &now)
    }

    /// 最近项目。
    pub fn list_recent(&self, limit: u32) -> Result<Vec<ProjectRecord>> {
        ProjectStore::new(self.conn).list_recent(limit)
    }

    /// 删除项目（幂等）。FK 语义：session 保留但失去归属（SET NULL），
    /// 项目级设置级联删除（CASCADE）。
    pub fn remove(&self, project_id: &str) -> Result<bool> {
        ProjectStore::new(self.conn).delete(project_id)
    }

    /// 项目下的 Session 历史（复用 sessions.project_id 外键）。
    pub fn sessions(&self, project_id: &str, limit: u32) -> Result<Vec<SessionRecord>> {
        crate::session::store::SessionStore::new(self.conn).list_by_project(project_id, limit)
    }
}

/// repo root 的最后一段作为缺省名：`D:/work/demo` → `demo`。
/// Windows 反斜杠与正斜杠都处理。
fn default_name(repo_root: &str) -> String {
    repo_root
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(repo_root)
        .to_string()
}

#[cfg(test)]
mod default_name_tests {
    use super::default_name;

    #[test]
    fn takes_the_last_segment() {
        assert_eq!(default_name("D:/work/demo"), "demo");
        assert_eq!(default_name("D:\\work\\demo"), "demo");
        assert_eq!(default_name("/home/dev/harnesshub"), "harnesshub");
    }

    #[test]
    fn trims_trailing_separators() {
        assert_eq!(default_name("D:/work/demo/"), "demo");
        assert_eq!(default_name("D:/"), "D:");
    }
}
