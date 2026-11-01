//! Config Doctor（ROADMAP E17）：只读体检单个 harness 的配置健康度。
//!
//! 三项检查（复用 E14 发现层 + E16 凭据层，不重复实现）：
//!   1. `config_parseable`：配置文件存在且可解析；
//!   2. `credentials_reachable`：该 harness 的凭据句柄完整（store 侧值存在）；
//!   3. `provider_pointed`：配置里有显式的 provider 指向。
//!
//! **纯读**：本阶段只诊断 + 建议，Fix 走 E15 的投影/恢复（显式动作）。

use serde::{Deserialize, Serialize};

use super::{claude, codex, gemini};
use crate::credentials::store::{FileSecretStore, SecretStore};
use crate::error::Result;

/// 体检结论的一项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckItem {
    /// 检查名（稳定标识，前端图标 key）。
    pub check: String,
    pub status: CheckStatus,
    pub message: String,
    /// 可选的修复建议（纯文本建议，不自动执行）。
    pub suggestion: Option<String>,
}

/// 体检状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Warn,
    Error,
}

/// 对单个 harness 做只读体检。
///
/// 未实现体检的 harness → 明确错误（UI 不渲染入口）。
pub fn diagnose(home: &std::path::Path, harness_id: &str) -> Result<Vec<CheckItem>> {
    match harness_id {
        "claude" => diagnose_claude(home),
        "codex" => diagnose_codex(home),
        "gemini" => diagnose_gemini(home),
        other => Err(crate::error::Error::InvalidInput(format!(
            "{other} 的配置体检尚未实现（当前支持 claude/codex/gemini）"
        ))),
    }
}

/// `credentials_reachable`（E17 spec 检查项 2）：凭据句柄与 store 侧值的完整性。
///
/// 需要 DB 连接，因此独立于 [`diagnose`]（后者保持无 DB 纯函数），
/// 由 `config_doctor` 命令层合并进 CheckItem 列表。
pub fn credentials_check(conn: &rusqlite::Connection, harness_id: &str) -> CheckItem {
    let refs = crate::credentials::registry::list(conn, harness_id).unwrap_or_default();
    if refs.is_empty() {
        return CheckItem {
            check: "credentials_reachable".into(),
            status: CheckStatus::Warn,
            message: "未登记凭据句柄（使用官方登录则无需登记）".into(),
            suggestion: Some("如需 API Key Provider，在凭据区登记后重新体检".into()),
        };
    }

    let store = FileSecretStore;
    let orphans: Vec<String> = refs
        .iter()
        .filter(|item| !store.exists(&item.id).unwrap_or(false))
        .map(|item| item.label.clone())
        .collect();
    if orphans.is_empty() {
        CheckItem {
            check: "credentials_reachable".into(),
            status: CheckStatus::Ok,
            message: format!("{} 个凭据句柄完整（值在 store）", refs.len()),
            suggestion: None,
        }
    } else {
        CheckItem {
            check: "credentials_reachable".into(),
            status: CheckStatus::Error,
            message: format!(
                "{} 个凭据句柄的值在 store 缺失（孤儿句柄）：{}",
                orphans.len(),
                orphans.join("、")
            ),
            suggestion: Some("在凭据区删除这些句柄后重新登记".into()),
        }
    }
}

fn diagnose_claude(home: &std::path::Path) -> Result<Vec<CheckItem>> {
    let mut items = Vec::new();

    match claude::discover(home) {
        Ok(Some(summary)) => {
            items.push(CheckItem {
                check: "config_parseable".into(),
                status: CheckStatus::Ok,
                message: "settings.json 可解析".into(),
                suggestion: None,
            });
            let pointed = summary.fields.contains_key("env.ANTHROPIC_BASE_URL");
            items.push(CheckItem {
                check: "provider_pointed".into(),
                status: if pointed {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Warn
                },
                message: if pointed {
                    "已指向自定义 Provider（ANTHROPIC_BASE_URL）".into()
                } else {
                    "未设置 ANTHROPIC_BASE_URL，将使用官方默认 Provider".into()
                },
                suggestion: if pointed {
                    None
                } else {
                    Some("如需第三方 Provider，可在 env 块设置 ANTHROPIC_BASE_URL".into())
                },
            });
        }
        Ok(None) => {
            items.push(CheckItem {
                check: "config_parseable".into(),
                status: CheckStatus::Warn,
                message: "未找到 ~/.claude/settings.json（未初始化）".into(),
                suggestion: Some("运行一次 claude 完成初始化后再体检".into()),
            });
        }
        Err(error) => {
            items.push(CheckItem {
                check: "config_parseable".into(),
                status: CheckStatus::Error,
                message: error.to_string(),
                suggestion: Some("修复 JSON 语法后重新体检；可用 E15 备份恢复".into()),
            });
        }
    }

    Ok(items)
}

fn diagnose_codex(home: &std::path::Path) -> Result<Vec<CheckItem>> {
    let mut items = Vec::new();

    match codex::discover(home) {
        Ok(Some(summary)) => {
            items.push(CheckItem {
                check: "config_parseable".into(),
                status: CheckStatus::Ok,
                message: "config.toml 可解析".into(),
                suggestion: None,
            });
            let pointed = summary.fields.contains_key("model_provider");
            items.push(CheckItem {
                check: "provider_pointed".into(),
                status: if pointed {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Warn
                },
                message: if pointed {
                    format!(
                        "已配置自定义 model_provider（model = {}）",
                        summary
                            .fields
                            .get("model")
                            .map(String::as_str)
                            .unwrap_or("?")
                    )
                } else {
                    "未设置 model_provider，将使用官方默认".into()
                },
                suggestion: None,
            });
        }
        Ok(None) => {
            items.push(CheckItem {
                check: "config_parseable".into(),
                status: CheckStatus::Warn,
                message: "未找到 ~/.codex/config.toml（未初始化）".into(),
                suggestion: Some("运行一次 codex 完成初始化后再体检".into()),
            });
        }
        Err(error) => {
            items.push(CheckItem {
                check: "config_parseable".into(),
                status: CheckStatus::Error,
                message: error.to_string(),
                suggestion: Some("修复 TOML 语法后重新体检；可用 E15 备份恢复".into()),
            });
        }
    }

    Ok(items)
}

fn diagnose_gemini(home: &std::path::Path) -> Result<Vec<CheckItem>> {
    let mut items = Vec::new();

    match gemini::discover(home) {
        Ok(Some(summary)) => {
            items.push(CheckItem {
                check: "config_parseable".into(),
                status: CheckStatus::Ok,
                message: ".env 可解析，API key 已登记（值不展示）".into(),
                suggestion: None,
            });
            let set = summary.fields.values().any(|value| value == "set");
            items.push(CheckItem {
                check: "provider_pointed".into(),
                status: if set {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Warn
                },
                message: if set {
                    "API key 已设置（存在性已确认，值不展示）".into()
                } else {
                    "API key 为空值".into()
                },
                suggestion: None,
            });
        }
        Ok(None) => {
            items.push(CheckItem {
                check: "config_parseable".into(),
                status: CheckStatus::Warn,
                message: "未找到 ~/.gemini/.env（未初始化）".into(),
                suggestion: Some("运行一次 gemini 完成认证后再体检".into()),
            });
        }
        Err(error) => {
            items.push(CheckItem {
                check: "config_parseable".into(),
                status: CheckStatus::Error,
                message: error.to_string(),
                suggestion: Some("检查 .env 内容后重新体检".into()),
            });
        }
    }

    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 跨 IPC 契约测试（AGENTS.md）：前端 `normalizeCheckItem` 依赖这些键
    /// 与 status 的 snake_case 序列化。
    #[test]
    fn check_item_serializes_with_camel_case_keys() {
        let item = CheckItem {
            check: "config_parseable".to_string(),
            status: CheckStatus::Warn,
            message: "未找到配置文件".to_string(),
            suggestion: Some("运行一次 claude".to_string()),
        };
        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["check"], "config_parseable");
        assert_eq!(json["status"], "warn");
        assert_eq!(json["message"], "未找到配置文件");
        assert_eq!(json["suggestion"], "运行一次 claude");
        assert!(json.get("suggestion_text").is_none(), "不得两套契约并存");

        for (status, expected) in [
            (CheckStatus::Ok, "ok"),
            (CheckStatus::Warn, "warn"),
            (CheckStatus::Error, "error"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), expected);
        }
    }

    fn temp_home_with(rel: &str, contents: &str) -> std::path::PathBuf {
        let home = std::env::temp_dir().join(format!("hh-doctor-{}", uuid::Uuid::new_v4()));
        let path = home.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
        home
    }

    /// ok 路径：claude 配置合法且有 BASE_URL → 两项 ok。
    #[test]
    fn healthy_claude_config_reports_ok() {
        let home = temp_home_with(
            ".claude/settings.json",
            r#"{"env":{"ANTHROPIC_BASE_URL":"https://api.deepseek.com/anthropic"}}"#,
        );
        let items = diagnose(&home, "claude").unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|item| item.status == CheckStatus::Ok));

        let _ = std::fs::remove_dir_all(&home);
    }

    /// warn 路径：文件不存在 → config_parseable = warn（未初始化不是错误）。
    #[test]
    fn missing_config_is_warn_not_error() {
        let home = std::env::temp_dir().join(format!("hh-doctor-none-{}", uuid::Uuid::new_v4()));
        let items = diagnose(&home, "claude").unwrap();
        assert_eq!(items[0].status, CheckStatus::Warn);
        assert!(items[0].suggestion.is_some());

        let _ = std::fs::remove_dir_all(&home);
    }

    /// error 路径：语法坏 → config_parseable = error + 建议。
    #[test]
    fn broken_config_is_error_with_suggestion() {
        let home = temp_home_with(".codex/config.toml", "= broken [");
        let items = diagnose(&home, "codex").unwrap();
        assert_eq!(items[0].status, CheckStatus::Error);
        assert!(items[0]
            .suggestion
            .as_deref()
            .is_some_and(|s| s.contains("恢复")));

        let _ = std::fs::remove_dir_all(&home);
    }

    /// 未实现体检的 harness → 明确错误。
    #[test]
    fn unsupported_harness_is_a_clear_error() {
        let home = std::env::temp_dir().join(format!("hh-doctor-x-{}", uuid::Uuid::new_v4()));
        let error = diagnose(&home, "copilot").unwrap_err();
        assert!(error.to_string().contains("尚未实现"), "{error}");
    }

    /// gemini：key set → ok；无 key 文件 → warn。
    #[test]
    fn gemini_key_presence_drives_status() {
        let home = temp_home_with(".gemini/.env", "GEMINI_API_KEY=x\n");
        let items = diagnose(&home, "gemini").unwrap();
        assert!(items.iter().all(|item| item.status == CheckStatus::Ok));
        let _ = std::fs::remove_dir_all(&home);

        let home = std::env::temp_dir().join(format!("hh-doctor-none-{}", uuid::Uuid::new_v4()));
        let items = diagnose(&home, "gemini").unwrap();
        assert_eq!(items[0].status, CheckStatus::Warn);
    }

    /// `credentials_reachable` 三态：无句柄 warn / 完整 ok / 孤儿 error。
    #[test]
    fn credentials_check_covers_all_three_statuses() {
        let db = crate::test_support::empty_db();
        let conn = db.connection();
        conn.execute(
            "INSERT INTO harnesses (id, display_name, created_at, updated_at)
             VALUES ('codex', 'Codex', 't', 't')",
            [],
        )
        .unwrap();

        // 无句柄 → warn。
        let item = credentials_check(conn, "codex");
        assert_eq!(item.status, CheckStatus::Warn);

        // 有句柄且值在 store → ok（结束时 remove 清理 store + 行）。
        let record = crate::credentials::registry::register(conn, "codex", "k1", "v1").unwrap();
        let item = credentials_check(conn, "codex");
        assert_eq!(item.status, CheckStatus::Ok, "{}", item.message);

        // 手工删掉 store 值 → 孤儿句柄 → error（消息里带 label）。
        crate::credentials::store::FileSecretStore
            .delete(&record.id)
            .unwrap();
        let item = credentials_check(conn, "codex");
        assert_eq!(item.status, CheckStatus::Error);
        assert!(item.message.contains("k1"), "{}", item.message);

        crate::credentials::registry::remove(conn, &record.id).unwrap();
    }
}
