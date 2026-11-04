//! 标记块渲染器（ROADMAP E19）：canonical 内容 ↔ harness 记忆文件的合成。
//!
//! **纯函数核心**（`compose`）：不碰文件系统，幂等契约在此锁定——
//! - 目标无文件 / 纯用户文件 / 已有标记块 / 只有 begin 没有end，四种输入各有语义；
//! - 块外用户内容**逐字节保留**；二次渲染输出与首次逐字节相同。
//!
//! 写盘走 `profile::projection::apply_text`（backup → temp → verify → rename）。

use serde::{Deserialize, Serialize};

use super::registry;
use super::PromptArtifact;
use crate::error::{Error, Result};

/// Hub 内容标记行（独占一行）。
pub const MARKER_BEGIN: &str = "<!-- harness-hub:begin -->";
pub const MARKER_END: &str = "<!-- harness-hub:end -->";

/// harness → 记忆文件名（v0.4 的 per-harness renderer 就是文件名映射 + 原文入块；
/// 语义差异后续在本层演进，不破坏幂等契约）。
pub fn memory_file_name(harness_id: &str) -> Result<&'static str> {
    match harness_id {
        "claude" => Ok("CLAUDE.md"),
        "codex" => Ok("AGENTS.md"),
        "gemini" => Ok("GEMINI.md"),
        other => Err(Error::InvalidInput(format!(
            "{other} 的记忆文件投影尚未实现（当前支持 claude/codex/gemini）"
        ))),
    }
}

/// 渲染预览（**纯读**；apply 与 preview 同参同结果，前端 confirm 的就是这份内容）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRenderPreview {
    pub harness_id: String,
    /// 投影目标绝对路径（`<repo>/<CLAUDE.md|AGENTS.md|GEMINI.md>`）。
    pub target_path: String,
    /// 将写盘的全文（含标记块与用户内容）。
    pub content: String,
    /// true = 目标文件尚不存在，apply 会新建（备份区显示为空）。
    pub creates_file: bool,
}

/// 同一 scope 的多条 prompt 拼接（name 升序由 `registry::enabled_for_scope` 保证）。
fn join_canonical(prompts: &[PromptArtifact]) -> String {
    prompts
        .iter()
        .map(|prompt| prompt.content.trim_end())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 纯函数核心：existing markdown（None = 文件不存在）+ canonical 块 → 合成结果。
///
/// 语义（spec）：
/// - `None` → 内容 = 标记块（首渲，创建文件）；
/// - 有原文、无标记块 → 头部插入标记块，原文**逐字节**保留（仅以换行衔接）；
/// - 已有标记块 → 旧块整体替换（begin 到 end 含标记行），块外逐字节保留；
/// - 只有 begin 没有 end（用户手删）→ 明确报错，绝不落盘。
pub fn compose(existing: Option<&str>, canonical: &str) -> Result<String> {
    let block = format!("{MARKER_BEGIN}\n{canonical}\n{MARKER_END}");

    let Some(existing) = existing else {
        return Ok(format!("{block}\n"));
    };

    let begin = existing.find(MARKER_BEGIN);
    let end = existing.find(MARKER_END);
    match (begin, end) {
        (None, None) => {
            // 纯用户文件：头部插入，原文逐字节保留。
            Ok(format!("{block}\n{existing}"))
        }
        (Some(begin), Some(end)) => {
            if end < begin {
                return Err(Error::InvalidInput(
                    "标记块次序异常（end 在 begin 之前），文件可能被手工改坏；拒绝渲染".to_string(),
                ));
            }
            let end_line_end = existing[end..]
                .find('\n')
                .map(|offset| end + offset)
                .unwrap_or(existing.len());
            let mut result = String::with_capacity(existing.len() + block.len());
            result.push_str(&existing[..begin]);
            result.push_str(&block);
            result.push_str(&existing[end_line_end..]);
            Ok(result)
        }
        (Some(_), None) => Err(Error::InvalidInput(
            "已有 begin 标记但没有 end 标记（可能被手工截断）；拒绝渲染以保护用户内容".to_string(),
        )),
        (None, Some(_)) => Err(Error::InvalidInput(
            "只有 end 标记没有 begin 标记；拒绝渲染".to_string(),
        )),
    }
}

/// 计算渲染预览（纯读文件系统 + SQLite）。**不写任何文件**。
pub fn render_preview(
    conn: &rusqlite::Connection,
    project_root: &std::path::Path,
    harness_id: &str,
    project_id: Option<&str>,
) -> Result<PromptRenderPreview> {
    let file_name = memory_file_name(harness_id)?;
    let target = project_root.join(file_name);
    let prompts = registry::enabled_for_scope(conn, project_id)?;
    let canonical = join_canonical(&prompts);
    let existing = std::fs::read_to_string(&target).ok();
    let content = compose(existing.as_deref(), &canonical)?;

    Ok(PromptRenderPreview {
        harness_id: harness_id.to_string(),
        target_path: target.to_string_lossy().into_owned(),
        creates_file: !target.is_file(),
        content,
    })
}

/// 执行投影（**显式写动作**）：走 E15 文本管线（backup → temp → verify → rename）。
///
/// 目标文件允许新建（记忆文件投影是「创建或更新」，与 JSON 投影的
/// 「必须已存在」约束不同——见 spec）；新建时报告的 `backupId` 为空串。
pub fn render_apply(
    conn: &rusqlite::Connection,
    project_root: &std::path::Path,
    harness_id: &str,
    project_id: Option<&str>,
) -> Result<crate::profile::projection::ProjectionReport> {
    use crate::harness::probe::HostProbe;
    let probe = crate::harness::probe::SystemHostProbe::new();
    let home = probe
        .home_dir()
        .ok_or_else(|| Error::InvalidInput("无法确定 home 目录".to_string()))?;
    let backup_root = home.join(".harness-hub").join("backups").join(harness_id);
    render_apply_in(conn, project_root, harness_id, project_id, &backup_root)
}

/// [`render_apply`] 的可注入变体（测试用临时 home）。
pub(crate) fn render_apply_in(
    conn: &rusqlite::Connection,
    project_root: &std::path::Path,
    harness_id: &str,
    project_id: Option<&str>,
    backup_root: &std::path::Path,
) -> Result<crate::profile::projection::ProjectionReport> {
    let preview = render_preview(conn, project_root, harness_id, project_id)?;
    let target = std::path::PathBuf::from(&preview.target_path);
    crate::profile::projection::apply_text_in(&target, &preview.content, harness_id, backup_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 首渲（文件不存在）：内容 = 标记块 + 换行。
    #[test]
    fn compose_creates_marker_block_when_missing() {
        let result = compose(None, "# 规范\n- 用中文").unwrap();
        assert_eq!(
            result,
            format!("{MARKER_BEGIN}\n# 规范\n- 用中文\n{MARKER_END}\n")
        );
    }

    /// 纯用户文件：头部插入，原文逐字节保留。
    #[test]
    fn compose_prepends_to_user_content_byte_for_byte() {
        let user = "# 我的手写说明\n保持原样\n";
        let result = compose(Some(user), "canonical").unwrap();
        assert!(result.starts_with(MARKER_BEGIN));
        assert!(result.ends_with(user), "原文逐字节保留：{result:?}");
        assert!(result.contains("canonical"));
        // 不重复：原文中间不被触碰。
        let marker_end_pos = result.find(MARKER_END).unwrap();
        assert_eq!(&result[marker_end_pos + MARKER_END.len() + 1..], user);
    }

    /// 已有标记块：旧块整体替换，块外逐字节保留。
    #[test]
    fn compose_replaces_existing_block_preserving_surroundings() {
        let first = compose(None, "v1").unwrap();
        let with_user = format!("头部说明\n{first}\n尾部说明");
        let second = compose(Some(&with_user), "v2\n更长的内容").unwrap();

        assert!(second.contains("头部说明\n"));
        assert!(second.contains("\n尾部说明"));
        assert!(second.contains("v2\n更长的内容"));
        assert!(!second.contains("v1"), "旧块整体替换");

        // 幂等：对结果再渲染一次，逐字节相同。
        let third = compose(Some(&second), "v2\n更长的内容").unwrap();
        assert_eq!(third, second, "幂等 round-trip");
    }

    /// 只有 begin 没有 end（用户手删）→ 明确报错。
    #[test]
    fn compose_rejects_unclosed_begin_marker() {
        let broken = format!("{MARKER_BEGIN}\ncanonical\n被截断的文件");
        let error = compose(Some(&broken), "new").unwrap_err();
        assert!(error.to_string().contains("没有 end"), "{error}");
    }

    /// 只有 end 没有 begin → 明确报错。
    #[test]
    fn compose_rejects_orphan_end_marker() {
        let broken = format!("正文\n{MARKER_END}\n尾部");
        let error = compose(Some(&broken), "new").unwrap_err();
        assert!(error.to_string().contains("begin"), "{error}");
    }

    /// end 在 begin 之前 → 明确报错。
    #[test]
    fn compose_rejects_reversed_markers() {
        let broken = format!("正文\n{MARKER_END}\n中间\n{MARKER_BEGIN}\n尾部");
        let error = compose(Some(&broken), "new").unwrap_err();
        assert!(error.to_string().contains("次序"), "{error}");
    }

    /// 多条 prompt：按传入顺序拼接（升序由 registry 保证），空行分隔。
    #[test]
    fn join_canonical_joins_with_blank_lines() {
        let prompts = vec![
            PromptArtifact {
                id: "1".into(),
                name: "a".into(),
                content: "内容 A\n".into(),
                project_id: None,
                enabled: true,
                created_at: "t".into(),
                updated_at: "t".into(),
            },
            PromptArtifact {
                id: "2".into(),
                name: "b".into(),
                content: "内容 B".into(),
                project_id: None,
                enabled: true,
                created_at: "t".into(),
                updated_at: "t".into(),
            },
        ];
        assert_eq!(join_canonical(&prompts), "内容 A\n\n内容 B");
    }

    /// memory_file_name 映射：三实现 + 未实现明确报错。
    #[test]
    fn memory_file_names_map_per_harness() {
        assert_eq!(memory_file_name("claude").unwrap(), "CLAUDE.md");
        assert_eq!(memory_file_name("codex").unwrap(), "AGENTS.md");
        assert_eq!(memory_file_name("gemini").unwrap(), "GEMINI.md");
        assert!(memory_file_name("copilot").is_err());
    }

    /// DTO 契约（AGENTS.md）。
    #[test]
    fn render_preview_serializes_with_camel_case_keys() {
        let preview = PromptRenderPreview {
            harness_id: "claude".to_string(),
            target_path: "D:/work/demo/CLAUDE.md".to_string(),
            content: "x".to_string(),
            creates_file: true,
        };
        let json = serde_json::to_value(&preview).unwrap();
        assert_eq!(json["harnessId"], "claude");
        assert_eq!(json["targetPath"], "D:/work/demo/CLAUDE.md");
        assert_eq!(json["createsFile"], true);
        assert!(json.get("creates_file").is_none(), "不得两套契约并存");
    }

    /// 端到端：registry 数据 + compose + apply_text_in 写临时文件（首渲创建、二渲幂等）。
    #[test]
    fn render_apply_creates_and_updates_a_real_file() {
        use crate::test_support::empty_db;

        let db = empty_db();
        let conn = db.connection();
        conn.execute(
            "INSERT INTO prompt_artifacts (id, name, content, project_id, enabled, created_at, updated_at)
             VALUES ('p1', 'style', '# 规范', NULL, 1, 't', 't')",
            [],
        )
        .unwrap();

        let root = std::env::temp_dir().join(format!("hh-render-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let backup_root = root.join(".hub-backups");

        // 首渲：创建文件（无备份，backupId 空串）。
        let report = render_apply_in(conn, &root, "claude", None, &backup_root).unwrap();
        assert_eq!(report.target_path, root.join("CLAUDE.md").to_string_lossy());
        assert_eq!(report.backup_id, "", "新建场景没有旧内容可备份");
        let written = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        assert!(written.contains("# 规范"));
        assert!(written.starts_with(MARKER_BEGIN));

        // 二次渲染幂等；此时文件已存在 → 有备份。
        let report2 = render_apply_in(conn, &root, "claude", None, &backup_root).unwrap();
        let again = std::fs::read_to_string(root.join("CLAUDE.md")).unwrap();
        assert_eq!(again, written, "二次渲染逐字节相同");
        assert!(!report2.backup_id.is_empty(), "更新场景有备份");

        let _ = std::fs::remove_dir_all(&root);
    }
}
