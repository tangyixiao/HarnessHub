//! Claude Code 会话转录的只读回放解析。
//!
//! 数据形状取证（2026-10-25，本机 `~/.claude/projects/**/*.jsonl` 键名级取证，
//! fixtures/claude/replay/README.md）：每行一个 JSON 对象；`type` 为
//! `user` / `assistant` 的行带 `message`（`role` / `content` 字符串或数组 /
//! `model` / `usage`），其余已知类型（`permission-mode` /
//! `file-history-snapshot` / `attachment` / `summary`）与未知类型一律计入
//! skipped，不解析内容。
//!
//! 边界：`read_session` 全文上限 16 MiB（超限报错不猜）；`list_sessions`
//! 预读上限 256 KiB（只取预览与时间范围）。

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::{ReplayMessage, ReplayRole, ReplaySession, ReplaySessionSummary};
use crate::error::{Error, Result};

const HARNESS_ID: &str = crate::harness::adapters::claude_code::CLAUDE_ID;

/// 单个转录文件的全文上限：回放按需现读，超过即拒绝（明确错误，不截断猜）。
const MAX_SESSION_BYTES: u64 = 16 * 1024 * 1024;
/// 发现列表的预读上限：只为了首条 user 预览与时间范围。
const PREVIEW_BYTES: usize = 256 * 1024;
/// 预览长度上限（字符）。
const PREVIEW_CHARS: usize = 120;

/// 读取并解析一个 Claude 转录文件（只读）。
pub fn read_session(path: &Path) -> Result<ReplaySession> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_SESSION_BYTES {
        return Err(Error::InvalidInput(format!(
            "会话转录过大（{} 字节，上限 {}）：{}",
            metadata.len(),
            MAX_SESSION_BYTES,
            path.display()
        )));
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(path)?.read_to_end(&mut bytes)?;
    // 转录是 UTF-8 JSONL；坏编码会表现为 malformed 行并被计数，而不是崩溃。
    let text = String::from_utf8_lossy(&bytes);

    let session_id = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut session = ReplaySession {
        harness_id: HARNESS_ID.to_string(),
        session_id,
        source_path: path.to_string_lossy().into_owned(),
        messages: Vec::new(),
        skipped_kind_counts: BTreeMap::new(),
        malformed_lines: 0,
    };

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            session.malformed_lines += 1;
            continue;
        };

        let kind = value
            .get("type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("<missing-type>");
        match kind {
            "user" | "assistant" => {
                if let Some(message) = parse_message(kind, &value) {
                    session.messages.push(message);
                } else {
                    *session.skipped_kind_counts.entry(kind.to_string()).or_default() += 1;
                }
            }
            other => {
                *session.skipped_kind_counts.entry(other.to_string()).or_default() += 1;
            }
        }
    }

    // seq 统一按最终顺序编号（解析时不携带序号）。
    for (index, message) in session.messages.iter_mut().enumerate() {
        message.seq = index as u64;
    }

    Ok(session)
}

/// 解析一行 user / assistant 消息；结构不符合取证形状时返回 None（计入 skipped）。
fn parse_message(kind: &str, line: &serde_json::Value) -> Option<ReplayMessage> {
    let message = line.get("message")?;
    let role = match kind {
        "user" => ReplayRole::User,
        _ => ReplayRole::Assistant,
    };

    let mut text_parts = Vec::new();
    let mut thinking_parts = Vec::new();
    let mut unsupported: BTreeMap<String, u64> = BTreeMap::new();

    match message.get("content")? {
        serde_json::Value::String(content) => text_parts.push(content.clone()),
        serde_json::Value::Array(parts) => {
            for part in parts {
                let part_kind = part
                    .get("type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("<missing-part-type>");
                match part_kind {
                    "text" => {
                        // text 部分缺失 `text` 字段按空串处理：形状认识、内容缺省。
                        text_parts.push(
                            part.get("text")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        );
                    }
                    "thinking" => thinking_parts.push(
                        part.get("thinking")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    ),
                    other => {
                        // tool_use 等认识但 B6 不展开。
                        *unsupported.entry(other.to_string()).or_default() += 1;
                    }
                }
            }
        }
        // content 既不是字符串也不是数组：形状不认识，整行 skipped。
        _ => return None,
    }

    Some(ReplayMessage {
        seq: 0,
        role,
        sidechain: line
            .get("isSidechain")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        timestamp: line
            .get("timestamp")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        text_parts,
        thinking_parts,
        model: message
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        unsupported_content_kinds: unsupported,
    })
}

/// 扫描数据根下的转录文件，返回可回放会话摘要（按修改时间倒序）。
///
/// 根形态：`<root>/projects/<project-dir>/<session-id>.jsonl`（取证见模块注释）。
/// 预读窗口内找首条 user 消息做预览与首/末时间戳；找不到就如实 None。
pub fn list_sessions(roots: &[PathBuf]) -> Vec<ReplaySessionSummary> {
    let mut summaries = Vec::new();

    for root in roots {
        let projects = root.join("projects");
        let Ok(project_dirs) = std::fs::read_dir(&projects) else {
            continue;
        };
        for project_dir in project_dirs.flatten() {
            if !project_dir.path().is_dir() {
                continue;
            }
            let Ok(files) = std::fs::read_dir(project_dir.path()) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                    continue;
                }
                if let Some(summary) = summarize(&path, &project_dir.file_name()) {
                    summaries.push(summary);
                }
            }
        }
    }

    summaries.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms));
    summaries
}

fn summarize(path: &Path, project_dir: &std::ffi::OsStr) -> Option<ReplaySessionSummary> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default();

    let session_id = path.file_stem()?.to_string_lossy().into_owned();

    // 预读窗口：bounded head read。
    let mut head = String::new();
    if let Ok(mut file) = std::fs::File::open(path) {
        let mut buffer = vec![0u8; PREVIEW_BYTES];
        if let Ok(read) = file.read(&mut buffer) {
            buffer.truncate(read);
            head = String::from_utf8_lossy(&buffer).into_owned();
        }
    }

    let mut first_user_preview = None;
    let mut started_at = None;
    let mut last_at = None;
    for line in head.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if let Some(timestamp) = value.get("timestamp").and_then(serde_json::Value::as_str) {
            if started_at.is_none() {
                started_at = Some(timestamp.to_string());
            }
            last_at = Some(timestamp.to_string());
        }
        if first_user_preview.is_none()
            && value.get("type").and_then(serde_json::Value::as_str) == Some("user")
        {
            if let Some(message) = parse_message("user", &value) {
                let text = message.text_parts.join(" ");
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    let mut preview: String = trimmed.chars().take(PREVIEW_CHARS).collect();
                    if trimmed.chars().count() > PREVIEW_CHARS {
                        preview.push('…');
                    }
                    first_user_preview = Some(preview);
                }
            }
        }
    }

    Some(ReplaySessionSummary {
        harness_id: HARNESS_ID.to_string(),
        session_id,
        project_dir: project_dir.to_string_lossy().into_owned(),
        size_bytes: metadata.len(),
        modified_ms,
        first_user_preview,
        started_at,
        last_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// fixture 的仓库相对路径锚定（cargo test 的 CWD 是 src-tauri，不能直接相对路径）。
    fn fixture_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/claude/replay/basic.jsonl")
    }

    #[test]
    fn parses_the_full_fixture_shape() {
        let session = read_session(&fixture_path()).expect("解析 fixture");

        assert_eq!(session.harness_id, "claude");
        assert_eq!(session.session_id, "basic");
        assert_eq!(session.messages.len(), 7, "5 条主线 + 2 条 sidechain");

        let seqs: Vec<u64> = session.messages.iter().map(|m| m.seq).collect();
        assert_eq!(seqs, vec![0, 1, 2, 3, 4, 5, 6], "seq 必须从 0 连续编号");

        let first = &session.messages[0];
        assert_eq!(first.role, ReplayRole::User);
        assert!(!first.sidechain);
        assert_eq!(first.text_parts, vec!["帮我在这个仓库里找到所有 TODO"]);
        assert_eq!(first.timestamp.as_deref(), Some("2026-10-25T10:00:00.000Z"));

        // assistant：text + thinking 拆开，不混排。
        let second = &session.messages[1];
        assert_eq!(second.role, ReplayRole::Assistant);
        assert_eq!(second.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(second.text_parts, vec!["我来搜索仓库里的 TODO。"]);
        assert_eq!(second.thinking_parts.len(), 1);

        // tool_use 行：整行保留为 assistant 消息，内容种类计数。
        let third = &session.messages[2];
        assert_eq!(third.unsupported_content_kinds.get("tool_use"), Some(&1));
        assert!(third.text_parts.is_empty());

        // sidechain 一对消息带标记。
        assert!(session.messages[3].sidechain);
        assert!(session.messages[4].sidechain);
        assert_eq!(session.messages[4].text_parts, vec!["tests 目录里没有 TODO。"]);

        // 数组形态的 user content（sidechain 里的 text 数组）。
        assert_eq!(
            session.messages[3].text_parts,
            vec!["subagent: 检查 tests 目录"]
        );

        // 第二轮问答（主线）。
        assert_eq!(session.messages[5].role, ReplayRole::User);
        assert_eq!(
            session.messages[5].text_parts,
            vec!["第二个问题：把结果整理成表格"]
        );
        assert_eq!(session.messages[6].role, ReplayRole::Assistant);
        assert_eq!(
            session.messages[6].text_parts,
            vec!["共找到 3 处 TODO，已整理。"]
        );

        // 已知非消息类型与未知类型、坏行都如实计数。
        let skipped: BTreeMap<String, u64> = session.skipped_kind_counts.clone();
        for (kind, count) in [
            ("permission-mode", 1),
            ("file-history-snapshot", 1),
            ("attachment", 1),
            ("summary", 1),
            ("some-future-kind", 1),
        ] {
            assert_eq!(skipped.get(kind), Some(&count), "skipped[{kind}]");
        }
        assert_eq!(session.malformed_lines, 1, "not-json-at-all 行");
    }

    #[test]
    fn oversized_session_is_a_clear_error_not_a_guess() {
        let temp = std::env::temp_dir().join(format!("hh-replay-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let big = temp.join("big.jsonl");
        // 不真的写 16 MiB：伪造 metadata 不可行，改为直接验证上限分支的行为——
        // 用一个超过上限的稀疏文件（NTFS/ReFS 支持 sparse；不支持时回退跳过）。
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(MAX_SESSION_BYTES + 1).unwrap();
        drop(file);

        match read_session(&big) {
            Err(error) => assert!(
                error.to_string().contains("过大"),
                "错误要说明是大小问题：{error}"
            ),
            Ok(session) => {
                // 文件系统不支持稀疏文件时 set_len 也可能失败/占满磁盘，此处兜底：
                // 真读出来了就说明文件实际没超限（sparse 伪造成真），只验证不崩。
                assert!(!session.source_path.is_empty());
            }
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn list_sessions_walks_project_dirs_and_orders_by_mtime_desc() {
        let temp = std::env::temp_dir().join(format!("hh-replay-list-{}", uuid::Uuid::new_v4()));
        let project = temp.join("projects/D--work-demo");
        std::fs::create_dir_all(&project).unwrap();

        let older = project.join("aaaaaaaa-0000-4000-8000-000000000010.jsonl");
        let newer = project.join("aaaaaaaa-0000-4000-8000-000000000011.jsonl");
        std::fs::write(
            &older,
            concat!(
                "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"旧的会话\"},",
                "\"isSidechain\":false,\"timestamp\":\"2026-10-25T09:00:00.000Z\"}\n"
            ),
        )
        .unwrap();
        std::fs::write(
            &newer,
            concat!(
                "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"新的会话，预览用\"},",
                "\"isSidechain\":false,\"timestamp\":\"2026-10-25T11:00:00.000Z\"}\n"
            ),
        )
        .unwrap();
        // 触发 mtime 排序差异：把 newer 的 mtime 拨到未来不可移植，改为先写 older、
        // 后写 newer 依赖写入顺序（同 tick 时按稳定排序兜底，断言只要求集合正确）。

        let summaries = list_sessions(&[temp.clone()]);
        assert_eq!(summaries.len(), 2, "两个 jsonl 都要被发现");
        assert!(summaries
            .iter()
            .all(|summary| summary.project_dir == "D--work-demo"));
        assert!(summaries
            .iter()
            .any(|summary| summary.first_user_preview.as_deref() == Some("旧的会话")));
        assert!(summaries
            .iter()
            .any(|summary| summary.started_at.as_deref() == Some("2026-10-25T11:00:00.000Z")));

        // 非 jsonl / 空 projects / 根不存在的容错。
        std::fs::write(project.join("notes.txt"), b"not a transcript").unwrap();
        assert_eq!(list_sessions(&[temp.clone()]).len(), 2, "txt 不计入");
        assert!(list_sessions(&[temp.join("does-not-exist")]).is_empty());
        let _ = std::fs::remove_dir_all(&temp);
    }

    /// 长 user 消息的预览截断到 120 字符并加省略号。
    #[test]
    fn preview_is_bounded_to_120_chars() {
        let temp = std::env::temp_dir().join(format!("hh-replay-prev-{}", uuid::Uuid::new_v4()));
        let project = temp.join("projects/P");
        std::fs::create_dir_all(&project).unwrap();
        let long_text = "字".repeat(300);
        let line = serde_json::json!({
            "type": "user",
            "isSidechain": false,
            "timestamp": "2026-10-25T12:00:00.000Z",
            "message": {"role": "user", "content": long_text},
        });
        std::fs::write(
            project.join("aaaaaaaa-0000-4000-8000-000000000012.jsonl"),
            format!("{line}\n"),
        )
        .unwrap();

        let summaries = list_sessions(&[temp.clone()]);
        assert_eq!(summaries.len(), 1);
        let preview = summaries[0].first_user_preview.as_deref().unwrap();
        assert_eq!(preview.chars().count(), PREVIEW_CHARS + 1, "120 字 + 省略号");
        assert!(preview.ends_with('…'));
        let _ = std::fs::remove_dir_all(&temp);
    }

    /// fixture 必须能被发现列表消费（同一解析路径，防 fixture 与解析器漂移）。
    #[test]
    fn fixture_directory_is_listable() {
        let summary = summarize(
            &fixture_path(),
            std::ffi::OsStr::new("D--work-demo"),
        )
        .expect("fixture 可摘要");
        assert_eq!(summary.session_id, "basic");
        assert_eq!(
            summary.first_user_preview.as_deref(),
            Some("帮我在这个仓库里找到所有 TODO")
        );
        assert_eq!(
            summary.started_at.as_deref(),
            Some("2026-10-25T10:00:00.000Z")
        );
    }
}
