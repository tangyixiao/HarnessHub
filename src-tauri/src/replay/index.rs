//! 回放内容搜索的索引与查询（ROADMAP B7，migration 0011）。
//!
//! 隐私纪律（§12 / spec）：
//!   * 索引**默认关闭**；关闭时清空全部索引行；
//!   * 排除目录（project_dir）不读内容、不出现在结果里；
//!   * 源转录永远只读，只写自己的三张表。
//!
//! 性能纪律（§13）：reindex 按 (size, mtime) 指纹增量；搜索只查 SQLite
//! （ADR-0012：查询路径不读外部文件、不起进程）。
//!
//! FTS5 trigram：中文/英文/混合子串可搜；**< 3 字符查询显式报错**
//! （trigram 无 3-gram 可匹配，静默空结果会误导）。

use std::collections::HashSet;
use std::path::PathBuf;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use super::claude::{self, harness_id};
use crate::error::{Error, Result};

/// 搜索设置（单行表的应用层形状）。`excluded_dirs` 是 project_dir 名单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSettings {
    pub enabled: bool,
    pub excluded_dirs: Vec<String>,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            excluded_dirs: Vec::new(),
        }
    }
}

/// 一次 reindex 的结果审计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReindexReport {
    /// 发现的转录数（含排除的）。
    pub scanned: u64,
    /// 新建或更新的索引条目。
    pub indexed: u64,
    /// 指纹未变而跳过的。
    pub unchanged: u64,
    /// 命中排除名单而完全不读的。
    pub excluded: u64,
    /// 源文件已消失而被清理的旧索引。
    pub removed: u64,
}

/// 一条搜索命中。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub harness_id: String,
    pub session_id: String,
    pub seq: u64,
    pub role: String,
    /// 命中片段（FTS5 snippet，命中词用 «» 包裹）。
    pub snippet: String,
    /// 会话首条 user 预览（前端跳转列表用）。
    pub first_user_preview: Option<String>,
}

pub fn settings(conn: &Connection) -> Result<SearchSettings> {
    let (enabled, excluded_json): (i64, String) = conn.query_row(
        "SELECT enabled, excluded_dirs FROM replay_search_settings WHERE id = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let excluded_dirs = serde_json::from_str(&excluded_json)
        .map_err(|error| Error::Replay(format!("排除名单损坏：{error}")))?;
    Ok(SearchSettings {
        enabled: enabled != 0,
        excluded_dirs,
    })
}

/// 更新设置。**关闭时清空全部索引行**（隐私纪律：关掉就真的没了）。
pub fn set_settings(conn: &mut Connection, new_settings: &SearchSettings) -> Result<()> {
    let transaction = conn.transaction()?;
    transaction.execute(
        "UPDATE replay_search_settings
         SET enabled = ?1, excluded_dirs = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')
         WHERE id = 1",
        params![
            i64::from(new_settings.enabled),
            serde_json::to_string(&new_settings.excluded_dirs)
                .map_err(|error| Error::Replay(format!("序列化排除名单失败：{error}")))?
        ],
    )?;

    if !new_settings.enabled {
        // 关闭 = 清空：不留可搜索内容。
        transaction.execute("DELETE FROM replay_messages", [])?;
        transaction.execute("DELETE FROM replay_index_state", [])?;
    } else if !new_settings.excluded_dirs.is_empty() {
        // 收紧排除名单：已索引的排除目录立即清掉。
        let mut statement = transaction.prepare(
            "DELETE FROM replay_messages
             WHERE session_id IN (
                SELECT session_id FROM replay_index_state WHERE project_dir = ?1
                  AND harness_id = ?2)",
        )?;
        for dir in &new_settings.excluded_dirs {
            statement.execute(params![dir, harness_id()])?;
        }
        drop(statement);
        let mut statement = transaction.prepare(
            "DELETE FROM replay_index_state WHERE project_dir = ?1 AND harness_id = ?2",
        )?;
        for dir in &new_settings.excluded_dirs {
            statement.execute(params![dir, harness_id()])?;
        }
    }

    transaction.commit()?;
    Ok(())
}

/// 显式重建/增量更新索引（用户动作触发，无自动后台任务）。
///
/// - 设置未启用 → 明确报错（不是静默空跑）；
/// - 排除目录：连指纹都不写（「未读」可断言）；
/// - 指纹 (size, mtime) 未变 → 跳过不重读；
/// - 源已消失的旧索引 → 清理。
pub fn reindex(conn: &mut Connection, roots: &[PathBuf]) -> Result<ReindexReport> {
    let current = settings(&*conn)?;
    if !current.enabled {
        return Err(Error::Replay("回放索引未启用：先在设置里打开".to_string()));
    }
    let excluded: HashSet<String> = current.excluded_dirs.iter().cloned().collect();

    let mut report = ReindexReport::default();
    let discovered = claude::discover_with_paths(roots);
    report.scanned = discovered.len() as u64;

    let transaction = conn.transaction()?;
    let mut insert_state = transaction.prepare(
        "INSERT OR REPLACE INTO replay_index_state
            (harness_id, session_id, project_dir, source_path, size_bytes, modified_ms,
             message_count, indexed_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
    )?;
    let mut clear_messages = transaction.prepare(
        "DELETE FROM replay_messages WHERE harness_id = ?1 AND session_id = ?2",
    )?;
    let mut insert_message = transaction.prepare(
        "INSERT INTO replay_messages (harness_id, session_id, seq, role, text_content)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;

    let mut seen: HashSet<(String, String)> = HashSet::new();
    for (path, summary) in &discovered {
        let key = (summary.harness_id.clone(), summary.session_id.clone());
        seen.insert(key.clone());

        if excluded.contains(&summary.project_dir) {
            report.excluded += 1;
            continue;
        }

        // 指纹未变 → 跳过（不重读内容）。
        let known: Option<(i64, i64)> = transaction
            .query_row(
                "SELECT size_bytes, modified_ms FROM replay_index_state
                 WHERE harness_id = ?1 AND session_id = ?2",
                params![summary.harness_id, summary.session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map(Some)
            .or_else(|error| match error {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        if let Some((size, modified)) = known {
            if size == summary.size_bytes as i64 && modified == summary.modified_ms {
                report.unchanged += 1;
                continue;
            }
        }

        let session = claude::read_session(path)?;
        clear_messages.execute(params![session.harness_id, session.session_id])?;
        for message in &session.messages {
            let mut content = message.text_parts.join("\n\n");
            if !message.thinking_parts.is_empty() {
                content.push_str("\n\n");
                content.push_str(&message.thinking_parts.join("\n\n"));
            }
            insert_message.execute(params![
                session.harness_id,
                session.session_id,
                message.seq as i64,
                role_str(message.role),
                content
            ])?;
        }
        insert_state.execute(params![
            session.harness_id,
            session.session_id,
            summary.project_dir,
            session.source_path,
            summary.size_bytes as i64,
            summary.modified_ms,
            session.messages.len() as i64
        ])?;
        report.indexed += 1;
    }
    drop(insert_state);
    drop(clear_messages);
    drop(insert_message);

    // 源已消失的旧索引：清理。
    let stale: Vec<(String, String)> = {
        let mut statement =
            transaction.prepare("SELECT harness_id, session_id FROM replay_index_state")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.flatten()
            .filter(|key| !seen.contains(key))
            .collect()
    };
    for (harness_id, session_id) in &stale {
        transaction.execute(
            "DELETE FROM replay_messages WHERE harness_id = ?1 AND session_id = ?2",
            params![harness_id, session_id],
        )?;
        transaction.execute(
            "DELETE FROM replay_index_state WHERE harness_id = ?1 AND session_id = ?2",
            params![harness_id, session_id],
        )?;
        report.removed += 1;
    }

    transaction.commit()?;
    Ok(report)
}

/// 搜索已索引内容。只查 SQLite（不读外部文件、不起进程）。
pub fn search(conn: &Connection, query: &str) -> Result<Vec<SearchHit>> {
    let current = settings(conn)?;
    if !current.enabled {
        return Err(Error::Replay("回放索引未启用：先在设置里打开".to_string()));
    }
    if query.chars().count() < 3 {
        return Err(Error::Replay(
            "搜索词至少需要 3 个字符（trigram 分词器的下限）".to_string(),
        ));
    }

    let excluded_json = serde_json::to_string(&current.excluded_dirs)
        .map_err(|error| Error::Replay(format!("序列化排除名单失败：{error}")))?;

    let mut statement = conn.prepare(
        "SELECT m.harness_id, m.session_id, m.seq, m.role,
                snippet(replay_messages, 4, '«', '»', '…', 16) AS snip
         FROM replay_messages m
         WHERE replay_messages MATCH ?1
           AND m.session_id NOT IN (
                SELECT session_id FROM replay_index_state
                 WHERE project_dir IN (SELECT value FROM json_each(?2))
                   AND harness_id = m.harness_id
           )
         ORDER BY m.harness_id, m.session_id, m.seq
         LIMIT 50",
    )?;

    let rows = statement.query_map(params![query, excluded_json], |row| {
        Ok(SearchHit {
            harness_id: row.get(0)?,
            session_id: row.get(1)?,
            seq: row.get::<_, i64>(2)? as u64,
            role: row.get(3)?,
            snippet: row.get(4)?,
            // 由 IPC 层从发现列表补齐；搜索路径不预读文件（ADR-0012）。
            first_user_preview: None,
        })
    })?;

    let mut hits = Vec::new();
    for row in rows {
        hits.push(row?);
    }
    Ok(hits)
}

/// role 的稳定字符串（与 fixture/DTO 的 snake_case 一致）。
fn role_str(role: super::ReplayRole) -> &'static str {
    match role {
        super::ReplayRole::User => "user",
        super::ReplayRole::Assistant => "assistant",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;

    fn db() -> Database {
        crate::test_support::empty_db()
    }

    /// 临时转录树：两个 project_dir，各一条会话。
    struct Tree(PathBuf);

    impl Tree {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("hh-replay-idx-{}", uuid::Uuid::new_v4()));
            for (dir, session, content) in [
                (
                    "D--work-alpha",
                    "aaaaaaaa-0000-4000-8000-000000000101",
                    "帮我重构数据库模块的迁移逻辑",
                ),
                (
                    "D--work-beta",
                    "aaaaaaaa-0000-4000-8000-000000000102",
                    "讨论一下部署脚本的缓存策略",
                ),
            ] {
                let project = root.join("projects").join(dir);
                std::fs::create_dir_all(&project).unwrap();
                let line = serde_json::json!({
                    "type": "user",
                    "isSidechain": false,
                    "timestamp": "2026-10-25T10:00:00.000Z",
                    "message": {"role": "user", "content": content},
                });
                std::fs::write(project.join(format!("{session}.jsonl")), format!("{line}\n"))
                    .unwrap();
            }
            Self(root)
        }

        fn roots(&self) -> Vec<PathBuf> {
            vec![self.0.clone()]
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn settings_default_to_disabled_and_roundtrip() {
        let mut conn = db();
        assert_eq!(
            settings(conn.connection()).unwrap(),
            SearchSettings {
                enabled: false,
                excluded_dirs: vec![]
            }
        );

        set_settings(
            conn.connection_mut(),
            &SearchSettings {
                enabled: true,
                excluded_dirs: vec!["D--work-beta".into()],
            },
        )
        .unwrap();
        assert_eq!(
            settings(conn.connection()).unwrap(),
            SearchSettings {
                enabled: true,
                excluded_dirs: vec!["D--work-beta".into()]
            }
        );
    }

    /// 关闭索引必须清空已索引内容（隐私纪律：关掉就真的没了）。
    #[test]
    fn disabling_the_index_clears_everything() {
        let mut conn = db();
        let tree = Tree::new();
        set_settings(
            conn.connection_mut(),
            &SearchSettings {
                enabled: true,
                excluded_dirs: vec![],
            },
        )
        .unwrap();
        reindex(conn.connection_mut(), &tree.roots()).unwrap();
        let rows: i64 = conn
            .connection()
            .query_row("SELECT count(*) FROM replay_messages", [], |r| r.get(0))
            .unwrap();
        assert!(rows > 0);

        set_settings(
            conn.connection_mut(),
            &SearchSettings {
                enabled: false,
                excluded_dirs: vec![],
            },
        )
        .unwrap();
        for table in ["replay_messages", "replay_index_state"] {
            let rows: i64 = conn
                .connection()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(rows, 0, "{table} 关闭后必须清空");
        }
        // 关闭状态下 reindex 与 search 都是明确错误，不是静默空结果。
        assert!(reindex(conn.connection_mut(), &tree.roots()).is_err());
        assert!(search(conn.connection(), "数据库模块").is_err());
    }

    #[test]
    fn reindex_indexes_searches_and_skips_unchanged_incrementally() {
        let mut conn = db();
        let tree = Tree::new();
        set_settings(
            conn.connection_mut(),
            &SearchSettings {
                enabled: true,
                excluded_dirs: vec![],
            },
        )
        .unwrap();

        let report = reindex(conn.connection_mut(), &tree.roots()).unwrap();
        assert_eq!(report.scanned, 2);
        assert_eq!(report.indexed, 2);
        assert_eq!(report.unchanged, 0);

        // 二次 reindex：指纹未变 → 全部 unchanged，不重读。
        let again = reindex(conn.connection_mut(), &tree.roots()).unwrap();
        assert_eq!(again.indexed, 0, "实际：{again:?}");
        assert_eq!(again.unchanged, 2);

        // 中文子串命中（trigram）。
        let hits = search(conn.connection(), "数据库模块的迁移").unwrap();
        assert!(!hits.is_empty(), "必须命中 alpha 会话");
        assert_eq!(hits[0].session_id, "aaaaaaaa-0000-4000-8000-000000000101");
        assert_eq!(hits[0].role, "user");
        assert!(
            hits[0].snippet.contains('«'),
            "snippet 要有命中标记：{hits:?}"
        );

        // <3 字符：明确错误。
        assert!(search(conn.connection(), "数据").is_err());
    }

    /// 排除目录：连指纹都不写（「未读」可断言），搜索结果也不出现。
    #[test]
    fn excluded_dirs_are_never_read_or_returned() {
        let mut conn = db();
        let tree = Tree::new();
        set_settings(
            conn.connection_mut(),
            &SearchSettings {
                enabled: true,
                excluded_dirs: vec!["D--work-beta".to_string()],
            },
        )
        .unwrap();

        let report = reindex(conn.connection_mut(), &tree.roots()).unwrap();
        assert_eq!(report.excluded, 1);
        assert_eq!(report.indexed, 1);

        let state_rows: i64 = conn
            .connection()
            .query_row("SELECT count(*) FROM replay_index_state", [], |r| r.get(0))
            .unwrap();
        assert_eq!(state_rows, 1, "排除目录不得留下指纹");

        // 排除目录的内容不可搜。
        assert!(search(conn.connection(), "缓存策略的部署").unwrap().is_empty());
        assert!(!search(conn.connection(), "数据库模块的迁移").unwrap().is_empty());
    }

    /// 源文件消失后 reindex 清理旧索引。
    #[test]
    fn vanished_sources_are_pruned_on_reindex() {
        let mut conn = db();
        let tree = Tree::new();
        set_settings(
            conn.connection_mut(),
            &SearchSettings {
                enabled: true,
                excluded_dirs: vec![],
            },
        )
        .unwrap();
        reindex(conn.connection_mut(), &tree.roots()).unwrap();

        std::fs::remove_file(
            tree.0.join("projects/D--work-alpha/aaaaaaaa-0000-4000-8000-000000000101.jsonl"),
        )
        .unwrap();
        let report = reindex(conn.connection_mut(), &tree.roots()).unwrap();
        assert_eq!(report.removed, 1, "实际：{report:?}");
        assert!(search(conn.connection(), "数据库模块的迁移").unwrap().is_empty());
    }

    /// **冷启动真路径**（AGENTS.md）：空库 → 生产代码启用 → reindex → 搜索命中。
    #[test]
    fn cold_start_empty_database_can_search_after_reindex() {
        let mut conn = db();
        let tree = Tree::new();

        set_settings(
            conn.connection_mut(),
            &SearchSettings {
                enabled: true,
                excluded_dirs: vec![],
            },
        )
        .unwrap();
        reindex(conn.connection_mut(), &tree.roots()).unwrap();

        let hits = search(conn.connection(), "部署脚本的缓存").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id, "aaaaaaaa-0000-4000-8000-000000000102");
        assert_eq!(hits[0].harness_id, "claude");
        assert_eq!(hits[0].seq, 0);
        assert_eq!(hits[0].first_user_preview, None, "搜索路径不预读文件");
    }
}
