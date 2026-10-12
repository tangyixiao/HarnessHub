//! Project registry：本地 Git 仓库的注册表。
//!
//! 职责（§20 第 14 步，spec：`docs/specs/2026-10-12-project-registry-design.md`）：
//! 把一个本地目录登记成「项目」—— 自动识别 repo root，以 repo root 为唯一键
//! 幂等 upsert；提供最近项目列表与删除。项目级 Harness 设置（`project_harness_settings`）
//! 与终端启动挂接不在本模块范围。
//!
//! 依赖方向：`project → db/error/clock`；`commands → project`。
//! repo root 检测走 [`git_root::GitRootProbe`] 注入，服务层不直接 `Command::new`。

pub mod git_root;
pub mod service;
pub mod store;

use serde::{Deserialize, Serialize};

/// 一条已注册项目。字段与 `projects` 表一一对应（migration 0001，未新增迁移）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub repo_root: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_opened_at: Option<String>,
}

impl ProjectRecord {
    /// 跨 IPC 契约测试用：锁 6 个 camelCase 键。
    pub fn keys() -> &'static [&'static str] {
        &[
            "id",
            "name",
            "repoRoot",
            "createdAt",
            "updatedAt",
            "lastOpenedAt",
        ]
    }
}

/// 登记请求：用户只给一个本地目录路径，名字可选（缺省取 repo root 的最后一段）。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRequest {
    pub path: String,
    pub name: Option<String>,
}

#[cfg(test)]
mod tests;
