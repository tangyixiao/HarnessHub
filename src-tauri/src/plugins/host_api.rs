//! 受限 Host API 分发点 + observe-only 权限记录（ADR-0018 决策 2/3）。
//!
//! 插件进程**没有**数据库句柄、**没有** Secret Store 句柄；它能做的每件事都必须
//! 表达成一次 `host_api` 调用。v0.1 的语义是 **observed**：
//!
//! ```text
//! 调用进来 → 分发（或返回 not_implemented）→ 无论成败都写一行 permission_decisions
//! ```
//!
//! 为什么「失败也要留痕」：Phase 9.2 的 Permission Engine 需要知道插件**尝试过**
//! 哪些能力，而不只是成功过哪些。只记成功会把「想读 secret 但没实现」这类信号丢掉。
//!
//! 为什么不在这里做拒绝：见 ADR-0018 —— 没有策略来源时拒绝会误杀合法调用，
//! 且会与 9.2 争夺所有权。`-32002 capability_denied` 只为 9.2 预留错误码，
//! v0.1 不产生它。

use rusqlite::{params, Connection};
use serde_json::Value;

use crate::clock;
use crate::error::Result;
use crate::plugins::rpc::{RpcError, METHOD_NOT_FOUND, NOT_IMPLEMENTED};

/// `host/log` 是 v0.1 唯一真实实现的 Host API。
pub const HOST_LOG: &str = "host/log";

/// 计划书列出的其余受限 API：协议已冻结，v0.1 一律返回 `-32003`。
pub const HOST_API_RESERVED: &[&str] = &[
    "fs.read_scoped",
    "secret.resolve_ref",
    "db.emit_event",
    "config.read_owned",
    "config.write_projected",
    "process.spawn_declared",
];

/// v0.1 的决策取值只有这一个。
///
/// 之所以用 CHECK 约束把它钉在数据库里（而不是靠 Rust 侧约定），
/// 是为了让 9.2 引入 `allowed` / `denied` 时必须**新增迁移**——
/// 隐式扩展枚举会让审计口径悄悄改变。
pub const DECISION_OBSERVED: &str = "observed";

/// `args_json` 的硬上限。
///
/// 插件可以把任意大的 JSON 塞进参数；不截断等于允许它把主库撑爆。
/// 512 字符足够人工诊断（能看到是哪个路径、哪个 ref），又不会让一行膨胀到 KiB 级。
pub const ARGS_JSON_LIMIT: usize = 512;

/// 截断后的 `args_json`：**按字符边界**切，不是按字节。
///
/// 直接 `&text[..512]` 会在中文/emoji 中间切开 UTF-8 序列并 panic —— 插件数据不可信，
/// 一条正常的中文参数就能把宿主打崩。
pub fn truncate_args(raw: &str) -> String {
    if raw.chars().count() <= ARGS_JSON_LIMIT {
        return raw.to_string();
    }

    let mut truncated: String = raw.chars().take(ARGS_JSON_LIMIT).collect();
    truncated.push('…');
    truncated
}

/// 一次 Host API 调用的分发结果。
///
/// 刻意不用 `Result`：observe-only 语义下，**拒绝与成功都是正常返回值**，
/// 都需要调用方把结果回给插件并写决策行。用 `Result` 会诱导调用方
/// 在 `Err` 分支里提前 return，从而漏记决策。
pub struct DispatchOutcome {
    pub result: Option<Value>,
    pub error: Option<RpcError>,
}

impl DispatchOutcome {
    fn ok(value: Value) -> Self {
        Self {
            result: Some(value),
            error: None,
        }
    }

    fn failed(error: RpcError) -> Self {
        Self {
            result: None,
            error: Some(error),
        }
    }

    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }
}

/// 把一次 Host API 调用落成决策数据。
///
/// `plugin_id` 不做外键指向 `plugins` 表：插件被卸载后审计行必须留下
/// （表定义见 `db/migrations/0008_permission_decisions.sql`）。
pub fn record_decision(
    conn: &Connection,
    plugin_id: &str,
    api: &str,
    reason: Option<&str>,
    args_json: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO permission_decisions (plugin_id, api, decision, reason, args_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            plugin_id,
            api,
            DECISION_OBSERVED,
            reason,
            truncate_args(args_json),
            clock::now_rfc3339(),
        ],
    )?;

    Ok(())
}

/// 分发一次插件发起的 Host API 调用，**并写决策行**。
///
/// 顺序是「先分发、后记录」，记录失败只记 `tracing::warn!` 不影响返回值：
/// 决策记录是观察数据，不能因为写库失败就让插件的合法调用失败
/// （v0.1 不做强制拒绝，所以这里的失败不会改变插件的可观测行为）。
pub fn dispatch(
    conn: &Connection,
    plugin_id: &str,
    method: &str,
    params: &Value,
) -> DispatchOutcome {
    let outcome = route(plugin_id, method, params);

    let args_json = serde_json::to_string(params).unwrap_or_else(|_| "null".to_string());
    let reason = outcome.error.as_ref().map(|error| error.message.clone());
    if let Err(error) = record_decision(conn, plugin_id, method, reason.as_deref(), &args_json) {
        eprintln!("插件 {plugin_id} 的权限观察记录写入失败：{error}");
    }

    outcome
}

/// 纯路由：无副作用，方便单测。
fn route(plugin_id: &str, method: &str, params: &Value) -> DispatchOutcome {
    match method {
        HOST_LOG => {
            // 只收结构化字段，不落 raw params 的其余内容：
            // 插件可能把 secret 原文塞进 log 参数，落日志就等于落盘。
            let level = params
                .get("level")
                .and_then(Value::as_str)
                .unwrap_or("info");
            let message = params.get("message").and_then(Value::as_str).unwrap_or("");
            eprintln!("插件 {plugin_id} [{level}] {message}");
            DispatchOutcome::ok(serde_json::json!({ "logged": true }))
        }
        other if HOST_API_RESERVED.contains(&other) => DispatchOutcome::failed(RpcError::new(
            NOT_IMPLEMENTED,
            format!("host api not implemented in v0.1: {other}"),
        )),
        other => DispatchOutcome::failed(RpcError::new(
            METHOD_NOT_FOUND,
            format!("unknown host api: {other}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let mut conn = Connection::open_in_memory().expect("内存库");
        crate::db::migrations::apply_pending(&mut conn).expect("迁移");
        conn
    }

    #[test]
    fn host_log_is_the_only_implemented_api() {
        let conn = database();
        let outcome = dispatch(
            &conn,
            "demo",
            HOST_LOG,
            &serde_json::json!({"level": "warn", "message": "hello"}),
        );

        assert!(!outcome.is_error());
        assert_eq!(outcome.result.unwrap()["logged"], true);
    }

    #[test]
    fn reserved_apis_return_not_implemented_and_are_still_recorded() {
        let conn = database();

        for api in HOST_API_RESERVED {
            let outcome = dispatch(
                &conn,
                "demo",
                api,
                &serde_json::json!({"path": "/etc/passwd"}),
            );
            assert!(outcome.is_error(), "{api} 必须返回错误");
            assert_eq!(outcome.error.unwrap().code, NOT_IMPLEMENTED);
        }

        // 失败也留痕：9.2 需要「尝试过」的信号，不只是成功信号。
        let observed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM permission_decisions WHERE plugin_id = 'demo'",
                [],
                |row| row.get(0),
            )
            .expect("计数");
        assert_eq!(observed, HOST_API_RESERVED.len() as i64);
    }

    #[test]
    fn unknown_api_is_method_not_found_but_recorded() {
        let conn = database();
        let outcome = dispatch(&conn, "demo", "fs.everything", &serde_json::json!({}));

        assert_eq!(outcome.error.unwrap().code, METHOD_NOT_FOUND);

        let api: String = conn
            .query_row("SELECT api FROM permission_decisions", [], |row| row.get(0))
            .expect("审计行");
        assert_eq!(api, "fs.everything");
    }

    #[test]
    fn decision_is_always_observed_in_v01() {
        let conn = database();
        dispatch(&conn, "demo", HOST_LOG, &serde_json::json!({}));

        let decision: String = conn
            .query_row("SELECT decision FROM permission_decisions", [], |row| {
                row.get(0)
            })
            .expect("决策");
        assert_eq!(decision, DECISION_OBSERVED);
    }

    #[test]
    fn args_json_is_truncated_by_chars_not_bytes() {
        // 中文：每个字符 3 字节。按字节切会在字符中间 panic。
        let raw = "中".repeat(ARGS_JSON_LIMIT * 2);
        let truncated = truncate_args(&raw);

        assert_eq!(truncated.chars().count(), ARGS_JSON_LIMIT + 1); // +1 = 省略号
        assert!(truncated.ends_with('…'));

        // 边界：刚好等于上限时不加省略号。
        let exact = "a".repeat(ARGS_JSON_LIMIT);
        assert_eq!(truncate_args(&exact), exact);
    }

    #[test]
    fn oversized_params_land_in_sql_truncated() {
        let conn = database();
        let huge: String = "耗".repeat(4_000);
        dispatch(
            &conn,
            "demo",
            "secret.resolve_ref",
            &serde_json::json!({ "ref": huge }),
        );

        let stored: String = conn
            .query_row("SELECT args_json FROM permission_decisions", [], |row| {
                row.get(0)
            })
            .expect("参数");

        assert!(
            stored.chars().count() <= ARGS_JSON_LIMIT + 1,
            "落库参数必须截断：{} 字符",
            stored.chars().count()
        );
    }

    #[test]
    fn reason_records_the_failure_message() {
        let conn = database();
        dispatch(&conn, "demo", "db.emit_event", &serde_json::json!({}));

        let reason: String = conn
            .query_row("SELECT reason FROM permission_decisions", [], |row| {
                row.get(0)
            })
            .expect("reason");
        assert!(reason.contains("not implemented"), "{reason}");
    }
}
