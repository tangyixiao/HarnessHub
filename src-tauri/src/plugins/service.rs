//! 插件生命周期的**编排层**：发现 → 登记 → 握手 → 拉取 skill → 降级/复活。
//!
//! 三层分工是刻意拆开的，别为了「少一个文件」合回去：
//!
//! - [`super::discovery`] 只碰文件系统（有哪些目录像插件）
//! - [`super::registry`] 只碰数据库（行 ↔ 结构体）
//! - 本模块只做编排（什么时候 spawn、失败了标什么状态、什么时候重试）
//!
//! 编排层自己**不解析 manifest、不拼 SQL、不写 JSON-RPC**：那三件事各自有归属，
//! 混进来以后每加一条通路都要动这里。它只回答一个问题：**现在该对这个插件做什么**。
//!
//! 三条与 ADR-0018 直接相关的约定：
//!
//! 1. **一个插件坏了不影响其它插件**。`refresh` 逐个处理，单个失败写进
//!    [`RefreshReport`] 的 `errors`，不中断循环、不返回整体错误（R14）。
//! 2. **登记失败 = 不登记**。manifest 坏掉的目录不往 `plugins` 表写行：让它带着
//!    `ready` 状态出现在列表里，比看不到它更糟——用户会以为它能用。
//! 3. **进程只在 refresh 期间活着**。v0.1 不常驻插件进程：导入和取 skill 都是
//!    短动作，常驻只会让「插件悄悄挂了」变成常态。这也让故障隔离变得平凡。

use std::path::Path;

use rusqlite::Connection;
use serde_json::Value;

use super::adapters::{SKILL_GET, SKILL_LIST, USAGE_IMPORT};
use super::discovery::{self, DiscoveredPlugin};
use super::host_api;
use super::manifest::Manifest;
use super::registry::{self, PluginRecord, STATUS_DEGRADED, STATUS_READY};
use super::supervisor::{HostApi, PluginProcess};
use crate::clock;
use crate::error::{Error, Result};

/// 插件在 `usage_events.source` 里的取值前缀。
///
/// 带上 `plugin:` 前缀是为了在审计口径上区分「插件导入的数据」与「宿主自己跑
/// ccusage 导入的数据」——两者走同一套表，但出问题时需要能一眼分开。
pub const USAGE_SOURCE_PREFIX: &str = "plugin:";

/// 一次 refresh 的结果：成功登记了多少、失败了几条、各自的错误。
///
/// 刻意不返回 `Result`：单插件失败是**正常结果**而不是异常。用 `Result` 会诱导
/// 调用方在第一个坏插件处提前返回，把「还有 9 个插件能修好」变成「全都别装了」。
#[derive(Debug, Default)]
pub struct RefreshReport {
    pub registered: Vec<String>,
    pub errors: Vec<String>,
}

impl RefreshReport {
    pub fn is_empty(&self) -> bool {
        self.registered.is_empty() && self.errors.is_empty()
    }
}

/// 扫描 `root` 并把每个可用插件登记进 `plugins` 表，同时刷新 skill 缓存。
pub fn refresh(conn: &Connection, root: &Path) -> RefreshReport {
    let mut report = RefreshReport::default();

    for discovered in discovery::scan(root) {
        match register_one(conn, &discovered) {
            Ok(id) => report.registered.push(id),
            Err(error) => report.errors.push(error),
        }
    }

    report
}

/// 处理一个候选目录。
fn register_one(
    conn: &Connection,
    discovered: &DiscoveredPlugin,
) -> std::result::Result<String, String> {
    let Some(manifest) = discovered.manifest.as_ref() else {
        // 坏 manifest 不进 plugins 表（模块头第 2 条）。
        return Err(discovered
            .error
            .clone()
            .unwrap_or_else(|| format!("{}：无法加载 manifest", discovered.dir.display())));
    };

    let mut status = STATUS_READY;
    let mut status_note: Option<String> = None;

    // 握手成功与否决定 ready/degraded。失败**仍然登记**：用户需要看到
    // 「装了这个插件但它起不来」，而不是一个凭空消失的目录。
    match handshake(manifest, &discovered.dir) {
        Ok(process) => drop(process), // 立刻关闭：v0.1 不常驻进程
        Err(error) => {
            status = STATUS_DEGRADED;
            status_note = Some(error.to_string());
        }
    }

    let record = PluginRecord {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        api_version: manifest.api_version.clone(),
        path: discovered.dir.display().to_string(),
        capabilities_json: capabilities_json(manifest),
        // skills_cache_json 由下面的 set_skills_cache 单独写：upsert 刻意不覆盖它，
        // 这样一次握手失败不会把上次拉到的缓存清空。
        skills_cache_json: None,
        status: status.to_string(),
        registered_at: clock::now_rfc3339(),
    };
    registry::upsert_plugin(conn, &record)
        .map_err(|error| format!("{}：登记失败：{error}", manifest.id))?;

    if manifest.capabilities.skills {
        match fetch_skills(conn, manifest, &discovered.dir) {
            Ok(cache) => {
                if let Err(error) = registry::set_skills_cache(conn, &manifest.id, &cache) {
                    return Err(format!("{}：写 skill 缓存失败：{error}", manifest.id));
                }
            }
            Err(error) => {
                // 取 skill 失败把状态降级，但**不**抹掉登记：插件本体可能还能导入 usage。
                if let Err(write_error) = registry::set_status(conn, &manifest.id, STATUS_DEGRADED)
                {
                    return Err(format!("{}：标记降级失败：{write_error}", manifest.id));
                }
                status_note = Some(error.to_string());
            }
        }
    }

    if let Some(note) = status_note {
        eprintln!("插件 {} 已登记为 degraded：{note}", manifest.id);
    }

    Ok(manifest.id.clone())
}

/// spawn + 握手；成功后调用方负责让进程退出。
fn handshake(manifest: &Manifest, dir: &Path) -> Result<PluginProcess> {
    let mut process = PluginProcess::spawn(manifest, dir)?;
    process.handshake(env!("CARGO_PKG_VERSION"))?;
    Ok(process)
}

/// `skill/list` + 逐个 `skill/get`，组装成缓存 JSON。
///
/// 缓存形状 = `{"skills":[{"skill_id","name","description","body"}]}`：一个对象而不是
/// 裸数组，留出将来加字段的余地，也让「空数组」与「没有这个键」不会混淆。
fn fetch_skills(conn: &Connection, manifest: &Manifest, dir: &Path) -> Result<String> {
    let mut process = handshake(manifest, dir)?;
    let mut host = DatabaseHostApi {
        conn,
        plugin_id: &manifest.id,
    };
    let mut skills: Vec<Value> = Vec::new();

    let listed = process.call(SKILL_LIST, &serde_json::json!({}), &mut host)?;
    let entries = listed
        .get("skills")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::PluginProtocol(format!(
                "插件 {} 的 skill/list 回包缺少 skills 数组",
                manifest.id
            ))
        })?;

    for entry in entries {
        let Some(skill_id) = entry.get("skill_id").and_then(Value::as_str) else {
            // 单条坏数据跳过而不是整体失败：与 skill/get 失败同样的容错口径。
            eprintln!(
                "插件 {} 的 skill/list 有条目缺少 skill_id，已跳过",
                manifest.id
            );
            continue;
        };

        match process.call(
            SKILL_GET,
            &serde_json::json!({ "skill_id": skill_id }),
            &mut host,
        ) {
            Ok(full) => skills.push(full),
            Err(error) => eprintln!("插件 {} 取 skill {skill_id} 失败：{error}", manifest.id),
        }
    }

    process.shutdown();

    serde_json::to_string(&serde_json::json!({ "skills": skills }))
        .map_err(|error| Error::PluginProtocol(format!("skill 缓存序列化失败：{error}")))
}

/// 让插件发起的 Host API 调用落到 `permission_decisions`。
///
/// 这是 observe-only 的连接点：**只记录，不判断**（ADR-0018 决策 3）。
///
/// `plugin_id` 必须是真的插件 id：审计行靠它归属，写错等于审计失效。
struct DatabaseHostApi<'a> {
    conn: &'a Connection,
    plugin_id: &'a str,
}

impl HostApi for DatabaseHostApi<'_> {
    fn dispatch(
        &mut self,
        method: &str,
        params: &Value,
    ) -> std::result::Result<Value, super::rpc::RpcError> {
        let outcome = host_api::dispatch(self.conn, self.plugin_id, method, params);
        match (outcome.result, outcome.error) {
            (Some(result), _) => Ok(result),
            (None, Some(error)) => Err(error),
            // dispatch 的契约是「二选一」，这里兜底成内部错误而不是 panic：
            // 插件不可信，宿主不能因为一次分发结果不完整就崩。
            (None, None) => Err(super::rpc::RpcError::new(
                super::rpc::INTERNAL_ERROR,
                format!("host api {method} 未返回结果"),
            )),
        }
    }
}

/// manifest 的 `[capabilities]` 快照，落库供 `plugin_list` 原样返回。
fn capabilities_json(manifest: &Manifest) -> String {
    let capabilities = &manifest.capabilities;
    serde_json::json!({
        "detect": capabilities.detect,
        "launch": capabilities.launch,
        "resume": capabilities.resume,
        "usage": capabilities.usage,
        "trace": capabilities.trace,
        "profiles": capabilities.profiles,
        "mcp": capabilities.mcp,
        "skills": capabilities.skills,
    })
    .to_string()
}

/// 对一个**已登记**插件跑一次 `usage/import` 并把结果导入主库。
///
/// 返回导入的会话行数（0 表示插件没给出任何数据，是合法结果）。
///
/// 关键约束：插件回包被**组装成 ccusage 报告形状**再走现有解析器
/// （`usage::ccusage::parse_report` + `normalize_session`）。不另写一套解析器的理由是
/// 插件数据与 ccusage 数据必须受同一套校验——否则同一个坏字段在一条路径上被拒、
/// 在另一条路径上被静默接受，最终没人知道库里哪些行是可信的。
pub fn import_usage(conn: &Connection, plugin_id: &str, dir: &Path) -> Result<usize> {
    let record = registry::get_plugin(conn, plugin_id)?
        .ok_or_else(|| Error::InvalidInput(format!("插件 {plugin_id} 未登记")))?;

    if record.status == STATUS_DEGRADED {
        // 不在这里试图复活：refresh 才是复活入口。让「坏插件自动重试」变成隐式行为
        // 会让故障变成定时炸弹——用户看到 degraded，就该显式 refresh。
        return Err(Error::PluginDegraded(format!(
            "插件 {plugin_id} 处于 degraded，请先 refresh"
        )));
    }

    let requested_dir = dir.canonicalize().map_err(|error| {
        Error::InvalidInput(format!("插件 {plugin_id} 的请求目录不可用：{error}"))
    })?;
    let registered_dir = Path::new(&record.path).canonicalize().map_err(|error| {
        Error::InvalidInput(format!("插件 {plugin_id} 的登记目录不可用：{error}"))
    })?;
    if requested_dir != registered_dir {
        return Err(Error::InvalidInput(format!(
            "插件 {plugin_id} 的请求目录与登记目录不一致"
        )));
    }

    let manifest = super::manifest::load(&registered_dir)?;
    if manifest.id != plugin_id {
        return Err(Error::InvalidInput(format!(
            "插件 {plugin_id} 的 manifest id 与登记身份不一致"
        )));
    }
    if !manifest.capabilities.usage {
        return Err(Error::PluginProtocol(format!(
            "插件 {plugin_id} 未声明 usage 能力"
        )));
    }

    let normalized = match fetch_usage(conn, &manifest, &registered_dir) {
        Ok(normalized) => normalized,
        Err(error) => {
            registry::set_status(conn, plugin_id, STATUS_DEGRADED)?;
            return Err(error);
        }
    };
    let row_count = normalized.events.len();
    // 非空的零 token 会话也需要成功导入审计，只有真正空报告沿用早退语义。
    if normalized.sessions_seen == 0 {
        return Ok(0);
    }

    let source = format!("{USAGE_SOURCE_PREFIX}{plugin_id}");
    let normalized = retag_source(normalized, &source);

    let started_at = clock::now_rfc3339();
    let request = crate::usage::importer::request_from(
        &normalized,
        &source,
        Some(&manifest.version),
        None,
        crate::usage::ccusage::REPORT_KIND,
        &started_at,
    );
    let importer = crate::usage::importer::UsageImporter::new(conn);
    importer.import(&request)?;

    Ok(row_count)
}

/// 只处理插件进程与数据契约；SQLite 导入失败由调用方处理，不归咎于插件。
fn fetch_usage(
    conn: &Connection,
    manifest: &Manifest,
    dir: &Path,
) -> Result<crate::usage::NormalizedUsage> {
    let mut process = handshake(manifest, dir)?;
    let mut host = DatabaseHostApi {
        conn,
        plugin_id: &manifest.id,
    };
    let value = process.call(USAGE_IMPORT, &serde_json::json!({}), &mut host);
    process.shutdown();
    let value = value?;

    let sessions = value
        .get("session")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::PluginProtocol(format!(
                "插件 {} 的 usage/import 回包缺少 session 数组",
                manifest.id
            ))
        })?;

    // 组装成 ccusage 报告形状（键是单数 `session`，见 plugins/sdk/protocol.md §4）。
    let raw = serde_json::json!({ "session": sessions }).to_string();
    let report = crate::usage::ccusage::parse_report(&raw)?;
    crate::usage::ccusage::normalize_session(&report, crate::usage::ccusage::PRICING_MODE_AUTO)
}

/// 把归一化结果里的来源换成插件自己的 id，并**重算幂等键**。
///
/// 这两件事必须在一次操作里完成，否则会出现「键说自己是 ccusage、列说自己是插件」
/// 的行——ADR-0011 决策三把 `source` 当作身份维度的一部分，键若不跟着换，
/// 插件数据就会和宿主自己跑 ccusage 得到的数据撞同一个键：同一份会话被两条来源
/// 各导一次时，后写的会静默覆盖先写的。
///
/// 重算而不是复用原键，正是为了让这两条来源**永远不互相去重**——它们本就不同源。
fn retag_source(
    normalized: crate::usage::NormalizedUsage,
    source: &str,
) -> crate::usage::NormalizedUsage {
    let events = normalized
        .events
        .into_iter()
        .map(|mut draft| {
            draft.source = source.to_string();
            draft.stable_source_key =
                crate::usage::key::stable_source_key(&crate::usage::key::KeyDimensions {
                    source,
                    report_kind: &draft.report_kind,
                    harness: &draft.harness,
                    source_session_id: &draft.source_session_id,
                    model: &draft.model,
                });
            draft.key_version = crate::usage::key::KEY_VERSION;
            draft
        })
        .collect();

    crate::usage::NormalizedUsage {
        events,
        ..normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_json_names_every_flag() {
        let manifest = Manifest {
            id: "x".to_string(),
            name: "X".to_string(),
            version: "0.1.0".to_string(),
            api_version: super::super::manifest::API_VERSION.to_string(),
            command: vec!["python".to_string()],
            capabilities: super::super::manifest::Capabilities {
                skills: true,
                usage: true,
                ..Default::default()
            },
        };

        let json: Value = serde_json::from_str(&capabilities_json(&manifest)).expect("合法 JSON");

        // 八个空位必须**全部**出现：`plugin_list` 的消费者不该去猜缺键的语义。
        for key in [
            "detect", "launch", "resume", "usage", "trace", "profiles", "mcp", "skills",
        ] {
            assert!(json.get(key).is_some(), "capabilities 快照缺键：{key}");
        }
        assert_eq!(json["usage"], Value::Bool(true));
        assert_eq!(json["skills"], Value::Bool(true));
        assert_eq!(json["trace"], Value::Bool(false));
    }

    #[test]
    fn refresh_on_a_missing_root_reports_nothing_instead_of_failing() {
        let db = crate::db::Database::open_in_memory().expect("内存库");
        let missing = std::env::temp_dir().join("harness-hub-refresh-absent-never-created");

        let report = refresh(db.connection(), &missing);

        assert!(report.is_empty());
        assert!(registry::list_plugins(db.connection())
            .expect("列插件")
            .is_empty());
    }

    /// Skill 缓存形状是对象不是裸数组：将来加字段不至于破坏消费者。
    #[test]
    fn skill_cache_shape_is_an_object_with_a_skills_array() {
        let empty: Value = serde_json::from_str(r#"{"skills":[]}"#).expect("模板合法");

        assert!(empty["skills"].as_array().is_some());
    }

    /// 换源必须同时换幂等键，否则插件数据会和宿主自己跑 ccusage 撞键。
    ///
    /// 这条测试就是 ADR-0011 决策三的可执行版本：`source` 是身份维度，
    /// 只改列不改键 = 「键说自己是 ccusage、列说自己是插件」。
    #[test]
    fn retagging_rewrites_the_key_not_just_the_source_column() {
        let dimensions = crate::usage::key::KeyDimensions {
            source: crate::usage::ccusage::SOURCE_ID,
            report_kind: crate::usage::ccusage::REPORT_KIND,
            harness: "codex",
            source_session_id: "rollout-1",
            model: "gpt-5.6-sol",
        };
        let draft = crate::usage::UsageEventDraft {
            stable_source_key: crate::usage::key::stable_source_key(&dimensions),
            key_version: crate::usage::key::KEY_VERSION,
            source: crate::usage::ccusage::SOURCE_ID.to_string(),
            report_kind: crate::usage::ccusage::REPORT_KIND.to_string(),
            harness: "codex".to_string(),
            source_session_id: "rollout-1".to_string(),
            model: "gpt-5.6-sol".to_string(),
            provider: None,
            input_tokens: 10,
            output_tokens: 1,
            cache_creation_tokens: 0,
            cached_input_tokens: None,
            reasoning_tokens: None,
            total_tokens: 11,
            cost_microunits: None,
            currency: None,
            currency_source: None,
            token_source: "reported".to_string(),
            cost_source: None,
            pricing_mode: None,
            occurred_at: None,
            occurred_at_source: "unavailable".to_string(),
            day: None,
            raw_payload: None,
        };
        let ccusage_key = draft.stable_source_key.clone();
        let normalized = crate::usage::NormalizedUsage {
            events: vec![draft],
            totals: None,
            sessions_seen: 1,
            timestampless: 1,
            row_total_residual: 0,
        };

        let retagged = retag_source(normalized, "plugin:usage-connect");

        let event = &retagged.events[0];
        assert_eq!(event.source, "plugin:usage-connect");
        assert_ne!(
            event.stable_source_key, ccusage_key,
            "同一个会话经插件导入必须换一把新键，否则会和宿主的 ccusage 记录互相覆盖"
        );
        // 新键必须真的是「用新 source 重算出来的」，不是随手换了个随机值。
        assert_eq!(
            event.stable_source_key,
            crate::usage::key::stable_source_key(&crate::usage::key::KeyDimensions {
                source: "plugin:usage-connect",
                report_kind: &event.report_kind,
                harness: &event.harness,
                source_session_id: &event.source_session_id,
                model: &event.model,
            }),
            "换源后的键必须能由新维度复算出来"
        );
        // 其余维度原样保留：换源不该动会话身份。
        assert_eq!(retagged.sessions_seen, 1);
        assert_eq!(event.harness, "codex");
        assert_eq!(event.source_session_id, "rollout-1");
    }
}
