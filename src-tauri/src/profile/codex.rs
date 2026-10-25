//! Codex 配置的只读发现：`~/.codex/config.toml` 顶层模型键 + provider base_url。
//!
//! 真机取证（2026-10-25）：顶层 `model` / `model_provider` /
//! `model_reasoning_effort`；`[model_providers.<name>]` 块的 `base_url` /
//! `env_key`（env_key 是 secret 引用名，本身不是 secret——保留键名但 redact 值）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{redact_if_secret, ProfileSummary};
use crate::error::{Error, Result};

/// 发现 Codex 的当前配置。文件不存在 → `Ok(None)`。
pub fn discover(home: &std::path::Path) -> Result<Option<ProfileSummary>> {
    let path = home.join(".codex").join("config.toml");
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)?;
    let value: toml::Value = raw
        .parse()
        .map_err(|error| Error::InvalidInput(format!("~/.codex/config.toml 语法错误：{error}")))?;

    let mut fields = BTreeMap::new();
    // 顶层选定键。
    for key in ["model", "model_provider", "model_reasoning_effort"] {
        if let Some(text) = value.get(key).and_then(toml::Value::as_str) {
            fields.insert(key.to_string(), redact_if_secret(key, text));
        }
    }
    // [model_providers.<name>] 的 base_url / env_key。
    if let Some(providers) = value.get("model_providers").and_then(toml::Value::as_table) {
        for (name, block) in providers {
            for key in ["base_url", "env_key", "wire_api"] {
                if let Some(text) = block.get(key).and_then(toml::Value::as_str) {
                    fields.insert(
                        format!("model_providers.{name}.{key}"),
                        redact_if_secret(key, text),
                    );
                }
            }
        }
    }

    Ok(Some(ProfileSummary {
        harness_id: "codex".to_string(),
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
        let home = std::env::temp_dir().join(format!("hh-codex-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(home.join(".codex/config.toml"), contents).unwrap();
        home
    }

    /// 真机形状：顶层模型键 + provider 块。
    #[test]
    fn reads_top_level_and_provider_blocks() {
        let home = temp_home(
            r#"
model = "gpt-6.1-sol"
model_reasoning_effort = "high"
service_tier = "default"

[model_providers.deepseek]
base_url = "https://api.deepseek.com/codex"
env_key = "DEEPSEEK_API_KEY"
wire_api = "responses"
"#,
        );

        let summary = discover(&home).unwrap().expect("文件存在");
        assert_eq!(summary.harness_id, "codex");
        assert_eq!(
            summary.fields.get("model").map(String::as_str),
            Some("gpt-6.1-sol")
        );
        assert_eq!(
            summary
                .fields
                .get("model_reasoning_effort")
                .map(String::as_str),
            Some("high")
        );
        assert_eq!(
            summary
                .fields
                .get("model_providers.deepseek.base_url")
                .map(String::as_str),
            Some("https://api.deepseek.com/codex")
        );
        // env_key 是引用名不是值本身，保留展示（用户需要知道引用了哪个变量）。
        assert_eq!(
            summary
                .fields
                .get("model_providers.deepseek.env_key")
                .map(String::as_str),
            Some("DEEPSEEK_API_KEY")
        );
        // 未选定键不提取。
        assert!(!summary.fields.contains_key("service_tier"));

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn missing_file_is_none() {
        let home = std::env::temp_dir().join(format!("hh-none-{}", uuid::Uuid::new_v4()));
        assert!(discover(&home).unwrap().is_none());
    }

    #[test]
    fn broken_toml_is_a_readable_error() {
        let home = temp_home("= not toml [");
        let error = discover(&home).unwrap_err();
        assert!(error.to_string().contains("语法错误"), "{error}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
