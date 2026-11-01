//! Registry → harness 配置的同步（ROADMAP E18）：
//!
//! ```text
//! desired（global ∪ harness 绑定，enabled）
//!   → diff（现有 mcpServers 段 vs desired）
//!   → preview（纯读，逐条列出 adds / updates / removals）
//!   → apply（backup → temp → verify → atomic rename，复用 E15 管线）
//! ```
//!
//! 调和粒度（spec）：**条目级删除 + 管理键级更新**——不在 desired 的整条移除
//! （preview 显式列出）；在 desired 的条目只覆盖 Hub 管理的键（command / args /
//! url / type），用户手写的 `env` / `headers` / `timeout` 等一律保留。
//!
//! 目标文件（真机取证，spec 表格）：
//! - claude `~/.claude.json`：顶层 `mcpServers`；http 形态 `{type:"http", url}`
//! - codex `~/.codex/config.toml`：`[mcp_servers.<name>]`；http 形态 `url = …`
//! - gemini `~/.gemini/settings.json`：顶层 `mcpServers`；http 形态 `{httpUrl: …}`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::registry::desired_servers;
use super::{McpServer, McpTransport};
use crate::error::{Error, Result};
use crate::profile::projection;

/// 同步预览（**纯读**，UI 展示后用户 confirm 才 apply）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSyncPreview {
    pub harness_id: String,
    pub target_path: String,
    /// 新增条目的 server 名。
    pub additions: Vec<String>,
    /// 更新条目（管理键有差异）。
    pub updates: Vec<McpDiffEntry>,
    /// 移除条目（现有文件里有、desired 里没有）。
    pub removals: Vec<McpDiffEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpDiffEntry {
    pub name: String,
    pub summary: String,
}

/// 一次同步的报告（apply 成功后返回）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSyncReport {
    pub target_path: String,
    pub backup_id: String,
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
}

/// 一次导入的报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImportReport {
    pub harness_id: String,
    pub imported: Vec<String>,
    pub skipped: Vec<String>,
}

/// 组合根的 home 来源（真实环境经 probe 注入；测试走 `_in` 变体绕开）。
fn probe_home() -> Result<PathBuf> {
    use crate::harness::probe::HostProbe;
    let probe = crate::harness::probe::SystemHostProbe::new();
    probe
        .home_dir()
        .ok_or_else(|| Error::InvalidInput("无法确定 home 目录".to_string()))
}

/// 同步目标（文件格式 + 相对 home 的路径）。未实现的 harness 明确报错。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncTarget {
    /// `~/.claude.json`（JSON）。
    ClaudeJson,
    /// `~/.codex/config.toml`（TOML，toml_edit 保格式编辑）。
    CodexToml,
    /// `~/.gemini/settings.json`（JSON）。
    GeminiJson,
}

impl SyncTarget {
    fn resolve_in(harness_id: &str, home: &Path) -> Result<(SyncTarget, PathBuf)> {
        match harness_id {
            "claude" => Ok((SyncTarget::ClaudeJson, home.join(".claude.json"))),
            "codex" => Ok((
                SyncTarget::CodexToml,
                home.join(".codex").join("config.toml"),
            )),
            "gemini" => Ok((
                SyncTarget::GeminiJson,
                home.join(".gemini").join("settings.json"),
            )),
            other => Err(Error::InvalidInput(format!(
                "{other} 的 MCP 同步尚未实现（当前支持 claude/codex/gemini）"
            ))),
        }
    }

    fn resolve(harness_id: &str) -> Result<(SyncTarget, PathBuf)> {
        let home = probe_home()?;
        Self::resolve_in(harness_id, &home)
    }
}

/// 从 JSON 文件读 `mcpServers` object（文件不存在返回空 map）。
fn read_json_servers(path: &Path) -> Result<BTreeMap<String, Value>> {
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let raw = std::fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|error| Error::InvalidInput(format!("{} 语法错误：{error}", path.display())))?;
    let Some(servers) = value.get("mcpServers").and_then(Value::as_object) else {
        return Ok(BTreeMap::new());
    };
    let mut map = BTreeMap::new();
    for (name, entry) in servers {
        if entry.is_object() {
            map.insert(name.clone(), entry.clone());
        }
    }
    Ok(map)
}

/// 从 TOML 文件读 `mcp_servers` 表（转成 JSON Value 复用同一套 diff 逻辑）。
fn read_toml_servers(path: &Path) -> Result<BTreeMap<String, Value>> {
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let raw = std::fs::read_to_string(path)?;
    let doc: toml::Value = raw
        .parse()
        .map_err(|error| Error::InvalidInput(format!("{} 语法错误：{error}", path.display())))?;
    let Some(servers) = doc.get("mcp_servers").and_then(toml::Value::as_table) else {
        return Ok(BTreeMap::new());
    };
    let mut map = BTreeMap::new();
    for (name, entry) in servers {
        // MCP 条目（string / array / table）总是纯数据，toml → json 无损。
        let Ok(converted) = serde_json::to_value(entry) else {
            continue;
        };
        if converted.is_object() {
            map.insert(name.clone(), converted);
        }
    }
    Ok(map)
}

/// 条目的**管理键摘要**（不含 env 值——env 根本不在管理键里）。
fn managed_summary(entry: &Value) -> String {
    let Some(object) = entry.as_object() else {
        return "<非对象条目>".to_string();
    };
    if let Some(url) = object
        .get("url")
        .or_else(|| object.get("httpUrl"))
        .and_then(Value::as_str)
    {
        format!("http url={url}")
    } else if let Some(command) = object.get("command").and_then(Value::as_str) {
        let args: Vec<&str> = object
            .get("args")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        format!("stdio command={command} args={args:?}")
    } else {
        "<未知形态>".to_string()
    }
}

/// 管理键是否与 desired 相同（决定 update 还是 no-op）。
fn managed_keys_equal(existing: &Value, desired_rendered: &Value) -> bool {
    let (Some(existing), Some(desired)) = (existing.as_object(), desired_rendered.as_object())
    else {
        return false;
    };
    for key in ["command", "args", "url", "type", "httpUrl"] {
        if existing.get(key) != desired.get(key) {
            return false;
        }
    }
    true
}

/// 把一个 desired server 渲染成目标 harness 的 JSON 条目形态（只含管理键）。
fn render_json_entry(target: SyncTarget, server: &McpServer) -> Value {
    let mut object = Map::new();
    match server.transport {
        McpTransport::Http => {
            let url = server.url.clone().unwrap_or_default();
            match target {
                SyncTarget::ClaudeJson => {
                    object.insert("type".to_string(), Value::String("http".to_string()));
                    object.insert("url".to_string(), Value::String(url));
                }
                SyncTarget::GeminiJson => {
                    object.insert("httpUrl".to_string(), Value::String(url));
                }
                SyncTarget::CodexToml => {
                    object.insert("url".to_string(), Value::String(url));
                }
            }
        }
        McpTransport::Stdio => {
            object.insert(
                "command".to_string(),
                Value::String(server.command.clone().unwrap_or_default()),
            );
            object.insert(
                "args".to_string(),
                Value::Array(server.args.iter().cloned().map(Value::String).collect()),
            );
        }
    }
    Value::Object(object)
}

/// 计算某个 harness 的同步预览。**纯读**（只读文件与 SQLite）。
pub fn preview(conn: &rusqlite::Connection, harness_id: &str) -> Result<McpSyncPreview> {
    let (target, path) = SyncTarget::resolve(harness_id)?;
    preview_in(conn, harness_id, target, &path)
}

fn preview_in(
    conn: &rusqlite::Connection,
    harness_id: &str,
    target: SyncTarget,
    path: &Path,
) -> Result<McpSyncPreview> {
    let existing = match target {
        SyncTarget::CodexToml => read_toml_servers(path)?,
        _ => read_json_servers(path)?,
    };
    let desired = desired_servers(conn, harness_id)?;

    let mut result = McpSyncPreview {
        harness_id: harness_id.to_string(),
        target_path: path.to_string_lossy().into_owned(),
        additions: Vec::new(),
        updates: Vec::new(),
        removals: Vec::new(),
    };

    for (name, server) in &desired {
        match existing.get(name) {
            None => result.additions.push(name.clone()),
            Some(entry) => {
                let rendered = render_json_entry(target, server);
                if !managed_keys_equal(entry, &rendered) {
                    result.updates.push(McpDiffEntry {
                        name: name.clone(),
                        summary: format!(
                            "{} → {}",
                            managed_summary(entry),
                            managed_summary(&rendered)
                        ),
                    });
                }
            }
        }
    }
    for (name, entry) in &existing {
        if !desired.contains_key(name) {
            result.removals.push(McpDiffEntry {
                name: name.clone(),
                summary: managed_summary(entry),
            });
        }
    }

    Ok(result)
}

/// 把调和结果渲染为**整个文件**的新内容（JSON 目标）。
fn render_json_file(
    target: SyncTarget,
    raw: &str,
    path: &Path,
    desired: &BTreeMap<String, McpServer>,
) -> Result<String> {
    let mut value: Value = serde_json::from_str(raw)
        .map_err(|error| Error::InvalidInput(format!("{} 语法错误：{error}", path.display())))?;
    let object = value.as_object_mut().ok_or_else(|| {
        Error::InvalidInput(format!("{} 不是 JSON object，同步中止", path.display()))
    })?;

    // 现有条目（作为保留非管理键的底稿）。
    let existing = object
        .get("mcpServers")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let mut reconciled = Map::new();
    for (name, server) in desired {
        let rendered = render_json_entry(target, server);
        let entry = match existing.get(name) {
            Some(entry) if entry.is_object() => {
                // 管理键覆盖；其余键（env / headers / …）原样保留。
                let mut merged = entry.as_object().cloned().unwrap_or_default();
                if let Some(rendered_object) = rendered.as_object() {
                    for (key, item) in rendered_object {
                        merged.insert(key.clone(), item.clone());
                    }
                }
                Value::Object(merged)
            }
            _ => rendered,
        };
        reconciled.insert(name.clone(), entry);
    }
    object.insert("mcpServers".to_string(), Value::Object(reconciled));

    serde_json::to_string_pretty(&value)
        .map_err(|error| Error::InvalidInput(format!("渲染失败：{error}")))
}

/// 把调和结果渲染为**整个文件**的新内容（codex TOML；toml_edit 保格式）。
fn render_toml_file(
    raw: &str,
    path: &Path,
    desired: &BTreeMap<String, McpServer>,
) -> Result<String> {
    let mut doc = raw
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| Error::InvalidInput(format!("{} 语法错误：{error}", path.display())))?;

    let existing: BTreeMap<String, toml_edit::Item> = doc
        .get("mcp_servers")
        .and_then(|item| item.as_table_like())
        .map(|table| {
            table
                .iter()
                .map(|(key, item)| (key.to_string(), item.clone()))
                .collect()
        })
        .unwrap_or_default();

    let mut reconciled = toml_edit::Table::new();
    reconciled.set_implicit(true);
    for (name, server) in desired {
        let mut entry = toml_edit::Table::new();
        match server.transport {
            McpTransport::Http => {
                entry.insert(
                    "url",
                    toml_edit::value(server.url.clone().unwrap_or_default()),
                );
            }
            McpTransport::Stdio => {
                entry.insert(
                    "command",
                    toml_edit::value(server.command.clone().unwrap_or_default()),
                );
                let mut args = toml_edit::Array::new();
                for arg in &server.args {
                    args.push(arg.as_str());
                }
                entry.insert("args", toml_edit::value(args));
            }
        }
        // 用户手写的 env 等键原样保留。
        if let Some(previous) = existing.get(name).and_then(|item| item.as_table_like()) {
            for (key, item) in previous.iter() {
                if key != "command" && key != "args" && key != "url" && !entry.contains_key(key) {
                    entry.insert(key, item.clone());
                }
            }
        }
        reconciled.insert(name, toml_edit::Item::Table(entry));
    }
    doc.insert("mcp_servers", toml_edit::Item::Table(reconciled));

    Ok(doc.to_string())
}

/// 执行同步（**显式写动作**）：调和 mcpServers 段 → E15 管线落盘。
///
/// 目标文件不存在 = 明确报错（未初始化的 harness 配置不凭空创建——与 E15 同一约束）。
pub fn apply(conn: &rusqlite::Connection, harness_id: &str) -> Result<McpSyncReport> {
    let home = probe_home()?;
    let (target, path) = SyncTarget::resolve_in(harness_id, &home)?;
    let backup_root = home.join(".harness-hub").join("backups").join(harness_id);
    apply_in(conn, harness_id, target, &path, &backup_root)
}

fn apply_in(
    conn: &rusqlite::Connection,
    harness_id: &str,
    target: SyncTarget,
    path: &Path,
    backup_root: &Path,
) -> Result<McpSyncReport> {
    if !path.is_file() {
        return Err(Error::InvalidInput(format!(
            "{} 不存在（harness 未初始化）；同步要求目标文件已存在",
            path.display()
        )));
    }
    let raw = std::fs::read_to_string(path)?;
    let desired = desired_servers(conn, harness_id)?;
    let counts = preview_in(conn, harness_id, target, path)?;
    let rendered = match target {
        SyncTarget::CodexToml => render_toml_file(&raw, path, &desired)?,
        _ => render_json_file(target, &raw, path, &desired)?,
    };

    let report = match target {
        SyncTarget::CodexToml => {
            projection::apply_text_in(path, &rendered, harness_id, backup_root)?
        }
        _ => projection::apply_in(path, &rendered, harness_id, backup_root)?,
    };

    Ok(McpSyncReport {
        target_path: report.target_path,
        backup_id: report.backup_id,
        added: counts.additions.len() as u64,
        updated: counts.updates.len() as u64,
        removed: counts.removals.len() as u64,
    })
}

/// 从现有 harness 配置导入 MCP 条目（**显式动作**）。
///
/// 名字不在 Registry 的登记为 `imported:<harness>`；env 只取**变量名**；
/// 名字已存在的跳过（同名以 Hub 为准——Registry 是 source-of-truth）；
/// 不可识别的形态（既无 command 也无 url）也计入 skipped。
pub fn import(conn: &rusqlite::Connection, harness_id: &str) -> Result<McpImportReport> {
    let (target, path) = SyncTarget::resolve(harness_id)?;
    import_in(conn, harness_id, target, &path)
}

fn import_in(
    conn: &rusqlite::Connection,
    harness_id: &str,
    target: SyncTarget,
    path: &Path,
) -> Result<McpImportReport> {
    if !path.is_file() {
        return Err(Error::InvalidInput(format!(
            "{} 不存在（harness 未初始化），没有可导入的内容",
            path.display()
        )));
    }
    let existing = match target {
        SyncTarget::CodexToml => read_toml_servers(path)?,
        _ => read_json_servers(path)?,
    };

    let mut report = McpImportReport {
        harness_id: harness_id.to_string(),
        imported: Vec::new(),
        skipped: Vec::new(),
    };
    for (name, entry) in existing {
        let source = format!("imported:{harness_id}");
        match super::registry::register_imported(conn, &name, &source, &entry) {
            Ok(true) => report.imported.push(name),
            Ok(false) => report.skipped.push(name),
            Err(error) => return Err(error),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::registry::McpServerInput;

    fn stdio(name: &str, command: &str) -> McpServerInput {
        McpServerInput {
            id: None,
            name: name.to_string(),
            transport: McpTransport::Stdio,
            command: Some(command.to_string()),
            args: vec!["-y".to_string()],
            url: None,
            env_keys: vec![],
            enabled: true,
        }
    }

    fn http(name: &str, url: &str) -> McpServerInput {
        McpServerInput {
            id: None,
            name: name.to_string(),
            transport: McpTransport::Http,
            command: None,
            args: vec![],
            url: Some(url.to_string()),
            env_keys: vec![],
            enabled: true,
        }
    }

    fn bind_global(conn: &rusqlite::Connection, server_id: &str) {
        super::super::registry::add_binding(conn, server_id, "global", None).unwrap();
    }

    fn temp_home(tag: &str) -> PathBuf {
        let home = std::env::temp_dir().join(format!("hh-mcp-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    /// 测试自用的备份目录（与 projection::backup_dir 的布局一致，但用注入的 home）。
    fn backup_root(home: &Path, harness_id: &str) -> PathBuf {
        home.join(".harness-hub").join("backups").join(harness_id)
    }

    /// 真机形状（claude）的 diff：additions / updates / removals，摘要不含 env 值。
    #[test]
    fn claude_json_preview_lists_additions_updates_removals() {
        let home = temp_home("preview");
        std::fs::write(
            home.join(".claude.json"),
            r#"{"foo": {"bar": 1}, "mcpServers": {"legacy": {"command": "old"}, "fetch": {"command": "npx", "args": ["-y"], "env": {"KEY": "value"}}}}"#,
        )
        .unwrap();

        let db = crate::test_support::empty_db();
        let conn = db.connection();
        let fetch = super::super::registry::upsert_server(conn, &stdio("fetch", "bunx")).unwrap();
        bind_global(conn, &fetch.id);

        let (target, path) = SyncTarget::resolve_in("claude", &home).unwrap();
        let result = preview_in(conn, "claude", target, &path).unwrap();

        assert_eq!(result.additions, Vec::<String>::new());
        assert_eq!(result.updates.len(), 1, "fetch 的 command 变了");
        assert_eq!(result.updates[0].name, "fetch");
        assert!(
            result.updates[0].summary.contains("npx") && result.updates[0].summary.contains("bunx"),
            "摘要含新旧 command：{}",
            result.updates[0].summary
        );
        assert_eq!(result.removals.len(), 1, "legacy 不在 desired");
        assert_eq!(result.removals[0].name, "legacy");
        assert!(
            !result.updates[0].summary.contains("value"),
            "摘要不得含 env 值：{}",
            result.updates[0].summary
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    /// apply：管理键覆盖、条目内 env 保留、非 desired 条目移除、文件其余顶层键保留。
    #[test]
    fn claude_json_apply_preserves_env_and_unrelated_keys() {
        let home = temp_home("apply");
        std::fs::write(
            home.join(".claude.json"),
            r#"{"topLevel": true, "mcpServers": {"legacy": {"command": "old"}, "fetch": {"command": "npx", "env": {"API_KEY": "keep-me"}}}}"#,
        )
        .unwrap();

        let db = crate::test_support::empty_db();
        let conn = db.connection();
        let fetch = super::super::registry::upsert_server(conn, &stdio("fetch", "bunx")).unwrap();
        bind_global(conn, &fetch.id);

        let (target, path) = SyncTarget::resolve_in("claude", &home).unwrap();
        let backup_root = backup_root(&home, "claude");
        let report = apply_in(conn, "claude", target, &path, &backup_root).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();

        let value: Value = serde_json::from_str(&written).unwrap();
        let servers = value["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1, "legacy 被移除");
        assert_eq!(servers["fetch"]["command"], "bunx", "管理键被覆盖");
        assert_eq!(
            servers["fetch"]["env"]["API_KEY"], "keep-me",
            "条目内用户 env 原样保留"
        );
        assert_eq!(value["topLevel"], true, "非 mcpServers 键原样保留");
        assert_eq!(report.updated, 1);
        assert_eq!(report.removed, 1);
        assert!(!report.backup_id.is_empty());

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&backup_root);
    }

    /// codex TOML：toml_edit 保格式（注释 / 无关键逐字节保留），http 用 url 键。
    #[test]
    fn codex_toml_apply_preserves_formatting_and_comments() {
        let home = temp_home("toml");
        let dir = home.join(".codex");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.toml"),
            "# 用户手编注释\nmodel = \"gpt-6.1-sol\"\n\n[mcp_servers.fetch]\ncommand = \"npx\"\nargs = [\"-y\"]\nenv = { API_KEY = \"keep\" }\n\n[mcp_servers.legacy]\ncommand = \"old\"\n",
        )
        .unwrap();

        let db = crate::test_support::empty_db();
        let conn = db.connection();
        let fetch = super::super::registry::upsert_server(conn, &stdio("fetch", "bunx")).unwrap();
        let remote =
            super::super::registry::upsert_server(conn, &http("remote", "https://mcp.example.com"))
                .unwrap();
        bind_global(conn, &fetch.id);
        bind_global(conn, &remote.id);

        let (target, path) = SyncTarget::resolve_in("codex", &home).unwrap();
        let backup_root = backup_root(&home, "codex");
        let report = apply_in(conn, "codex", target, &path, &backup_root).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();

        assert!(written.contains("# 用户手编注释"), "注释逐字节保留");
        assert!(written.contains("model = \"gpt-6.1-sol\""), "无关键保留");
        let doc: toml::Value = written.parse().unwrap();
        assert_eq!(
            doc["mcp_servers"]["fetch"]["command"].as_str(),
            Some("bunx")
        );
        assert_eq!(
            doc["mcp_servers"]["fetch"]["env"]["API_KEY"].as_str(),
            Some("keep"),
            "用户 env 表保留"
        );
        assert_eq!(
            doc["mcp_servers"]["remote"]["url"].as_str(),
            Some("https://mcp.example.com"),
            "http 形态 = url 键"
        );
        assert!(
            doc.get("mcp_servers")
                .and_then(|t| t.get("legacy"))
                .is_none(),
            "非 desired 条目移除"
        );
        assert_eq!(report.removed, 1);

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&backup_root);
    }

    /// gemini http 形态 = httpUrl。
    #[test]
    fn gemini_json_http_uses_http_url() {
        let home = temp_home("gemini");
        std::fs::create_dir_all(home.join(".gemini")).unwrap();
        std::fs::write(home.join(".gemini/settings.json"), r#"{"mcpServers": {}}"#).unwrap();

        let db = crate::test_support::empty_db();
        let conn = db.connection();
        let server =
            super::super::registry::upsert_server(conn, &http("remote", "https://mcp.example.com"))
                .unwrap();
        bind_global(conn, &server.id);

        let (target, path) = SyncTarget::resolve_in("gemini", &home).unwrap();
        let backup_root = backup_root(&home, "gemini");
        apply_in(conn, "gemini", target, &path, &backup_root).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        let value: Value = serde_json::from_str(&written).unwrap();
        assert_eq!(
            value["mcpServers"]["remote"]["httpUrl"].as_str(),
            Some("https://mcp.example.com")
        );

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&backup_root);
    }

    /// 目标文件不存在 = 明确报错（不凭空创建）。
    #[test]
    fn missing_target_file_is_a_clear_error() {
        let home = temp_home("missing");
        let db = crate::test_support::empty_db();
        let conn = db.connection();

        let (target, path) = SyncTarget::resolve_in("claude", &home).unwrap();
        let backup_root = backup_root(&home, "claude");
        let error = apply_in(conn, "claude", target, &path, &backup_root).unwrap_err();
        assert!(error.to_string().contains("不存在"), "{error}");

        let _ = std::fs::remove_dir_all(&home);
    }

    /// import：stdio 取 command/args + env **名字**；http 取 url；同名与不可识别形态跳过。
    #[test]
    fn import_registers_missing_and_skips_existing_or_unrecognized() {
        let home = temp_home("import");
        std::fs::write(
            home.join(".claude.json"),
            r#"{"mcpServers": {"a": {"command": "npx", "args": ["-y"], "env": {"KEY": "value"}}, "b": {"type": "http", "url": "https://x"}, "c": {"weird": true}}}"#,
        )
        .unwrap();

        let db = crate::test_support::empty_db();
        let conn = db.connection();
        // 预登记同名 a → import 应跳过（以 Hub 为准）。
        super::super::registry::upsert_server(conn, &stdio("a", "hub")).unwrap();

        let (target, path) = SyncTarget::resolve_in("claude", &home).unwrap();
        let report = import_in(conn, "claude", target, &path).unwrap();

        assert_eq!(report.imported, vec!["b"]);
        assert_eq!(report.skipped, vec!["a", "c"], "同名 + 不可识别形态都跳过");

        let servers = super::super::registry::list_servers(conn).unwrap();
        let imported_b = servers.iter().find(|s| s.name == "b").unwrap();
        assert_eq!(imported_b.source, "imported:claude");
        assert_eq!(imported_b.transport, McpTransport::Http);
        assert_eq!(imported_b.url.as_deref(), Some("https://x"));

        let _ = std::fs::remove_dir_all(&home);
    }
}
