//! Claude Code 配置的只读发现：`~/.claude/settings.json` 的 `env` 块。
//!
//! 真机取证（2026-10-25）：`env` 含 `ANTHROPIC_AUTH_TOKEN`（secret，redact）、
//! `ANTHROPIC_BASE_URL`、`ANTHROPIC_MODEL`、`ANTHROPIC_DEFAULT_*_MODEL`。
//! 只提取这些键；未知键忽略不报错（用户手编文件，schema 会漂移）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

use super::{redact_if_secret, ProfileSummary};
use crate::error::{Error, Result};

/// 发现 Claude 的当前配置。文件不存在 → `Ok(None)`（未初始化，不是错误）。
pub fn discover(home: &std::path::Path) -> Result<Option<ProfileSummary>> {
    let path = home.join(".claude").join("settings.json");
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)?;
    let value: Value = serde_json::from_str(&raw).map_err(|error| {
        Error::InvalidInput(format!("~/.claude/settings.json 语法错误：{error}"))
    })?;

    let mut fields = BTreeMap::new();
    // env 块：选定键前缀 ANTHROPIC_ / OPENAI_（常见 provider 指向），逐键 redact。
    if let Some(env) = value.get("env").and_then(Value::as_object) {
        for (key, val) in env {
            let key_text = key.as_str();
            let upper = key_text.to_ascii_uppercase();
            if (upper.starts_with("ANTHROPIC_") || upper.starts_with("OPENAI_"))
                && !upper.ends_with("_KEY")
            {
                if let Some(text) = val.as_str() {
                    fields.insert(format!("env.{key_text}"), redact_if_secret(key_text, text));
                }
            }
        }
    }

    Ok(Some(ProfileSummary {
        harness_id: crate::harness::adapters::claude_code::CLAUDE_ID.to_string(),
        source_path: path.to_string_lossy().into_owned(),
        fields,
        parsed_at: crate::clock::now_rfc3339(),
    }))
}

#[allow(dead_code)]
fn _unused(_: PathBuf) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(contents: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("hh-profile-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude/settings.json"), contents).unwrap();
        home
    }

    /// 真机形状（脱敏后）：env 块的 ANTHROPIC_ 键提取，TOKEN redacted。
    #[test]
    fn reads_the_env_block_and_redacts_tokens() {
        let home = temp_home(
            r#"{
  "env": {
    "ANTHROPIC_AUTH_TOKEN": "sk-real-secret-value",
    "ANTHROPIC_BASE_URL": "https://api.deepseek.com/anthropic",
    "ANTHROPIC_MODEL": "deepseek-v4-pro",
    "ANTHROPIC_DEFAULT_SONNET_MODEL": "deepseek-v4-pro",
    "UNRELATED": "ignored"
  },
  "includeCoAuthoredBy": false
}"#,
        );

        let summary = discover(&home).unwrap().expect("文件存在");
        assert_eq!(summary.harness_id, "claude");
        assert_eq!(
            summary
                .fields
                .get("env.ANTHROPIC_AUTH_TOKEN")
                .map(String::as_str),
            Some("<redacted>"),
            "token 必须被替换"
        );
        assert_eq!(
            summary
                .fields
                .get("env.ANTHROPIC_BASE_URL")
                .map(String::as_str),
            Some("https://api.deepseek.com/anthropic")
        );
        assert_eq!(
            summary
                .fields
                .get("env.ANTHROPIC_MODEL")
                .map(String::as_str),
            Some("deepseek-v4-pro")
        );
        assert!(
            !summary.fields.contains_key("env.UNRELATED"),
            "非 ANTHROPIC_/OPENAI_ 前缀的键不提取"
        );
        assert!(summary.source_path.ends_with("settings.json"));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn missing_file_is_none_not_an_error() {
        let home = std::env::temp_dir().join(format!("hh-none-{}", uuid::Uuid::new_v4()));
        assert!(discover(&home).unwrap().is_none());
    }

    #[test]
    fn broken_json_is_a_readable_error() {
        let home = temp_home("{ not json");
        let error = discover(&home).unwrap_err();
        assert!(error.to_string().contains("语法错误"), "{error}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
