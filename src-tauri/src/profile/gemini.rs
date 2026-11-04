//! Gemini CLI 配置的只读发现：`~/.gemini/.env` 的 API key 存在性。
//!
//! **只报 `set` / `unset`，绝不把 key 值带出 Rust 层**（spec 安全红线）。
//! 真机取证（2026-10-25）：本机无 `.gemini/.env`（Google 认证被拒的机器）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::{presence, ProfileSummary};
use crate::error::{Error, Result};

/// 发现 Gemini 的当前配置。文件不存在 → `Ok(None)`。
pub fn discover(home: &std::path::Path) -> Result<Option<ProfileSummary>> {
    let path = home.join(".gemini").join(".env");
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)?;
    let mut fields = BTreeMap::new();

    // .env 是 KEY=VALUE 行；只关心 GEMINI_API_KEY / GOOGLE_API_KEY 的存在性。
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            match key {
                "GEMINI_API_KEY" | "GOOGLE_API_KEY" => {
                    // 安全红线：只报 set/unset。
                    fields.insert(format!("{key}_PRESENT"), presence(Some(value)).to_string());
                }
                _ => {}
            }
        }
    }

    if fields.is_empty() {
        return Err(Error::InvalidInput(
            "~/.gemini/.env 存在但没有任何 GEMINI_API_KEY / GOOGLE_API_KEY 行".to_string(),
        ));
    }

    Ok(Some(ProfileSummary {
        harness_id: "gemini".to_string(),
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
        let home = std::env::temp_dir().join(format!("hh-gemini-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        std::fs::write(home.join(".gemini/.env"), contents).unwrap();
        home
    }

    /// key 值绝不跨层——只有 set/unset。
    #[test]
    fn reports_only_key_presence_never_the_value() {
        let home = temp_home("GEMINI_API_KEY=AIzaSyA-real-secret-key-value\n");

        let summary = discover(&home).unwrap().expect("文件存在");
        assert_eq!(
            summary
                .fields
                .get("GEMINI_API_KEY_PRESENT")
                .map(String::as_str),
            Some("set"),
            "只报 set，绝不带值"
        );
        let serialized = serde_json::to_string(&summary).unwrap();
        assert!(
            !serialized.contains("AIzaSyA"),
            "序列化结果不得含 key 原文：{serialized}"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn missing_file_is_none() {
        let home = std::env::temp_dir().join(format!("hh-none-{}", uuid::Uuid::new_v4()));
        assert!(discover(&home).unwrap().is_none());
    }

    #[test]
    fn file_without_relevant_keys_is_a_readable_error() {
        let home = temp_home("OTHER_VAR=x\n");
        let error = discover(&home).unwrap_err();
        assert!(error.to_string().contains("没有任何"), "{error}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
