//! F22 MCP permission policy（spec 2026-11-09-f22-mcp-client-server-design.md §3.8）。
//!
//! 策略 v1 **冻结规则**（只读启发式，默认拒绝）：
//! 1. tool 名 trim + lowercase 后按 `_` / `-` 取第一段；
//! 2. 第一段 ∈ [`READONLY_VERBS`] → allow（reason `readonly_by_default`）；
//! 3. 否则 deny（reason `not_readonly_by_default`）；
//! 4. 空名 / 内部空白 / 非法字符（非 `[a-z0-9_-]`）→ deny（reason
//!    `invalid_tool_name`）。
//!
//! 红线（§2.3）：deny 的调用**绝不转发 Python** —— 本模块是唯一策略判定点，
//! 纯函数，不碰 sidecar / SQLite / Tauri。`reason` 是冻结的稳定字符串，
//! 前端直接渲染（§3.8：HTTP 200 语义，deny 不做成 IPC 错误）。

/// 只读动词冻结集（spec §3.8：v1 不新增、不删除、不可配置——可配置策略是 G25）。
const READONLY_VERBS: &[&str] = &[
    "get", "list", "search", "read", "fetch", "query", "find", "describe", "info", "status",
    "health", "stats",
];

const ALLOW_READONLY: &str = "readonly_by_default";
const DENY_NOT_READONLY: &str = "not_readonly_by_default";
const DENY_INVALID: &str = "invalid_tool_name";

/// 策略判定结果。`allowed` 只表达策略结论；`reason` 恒为冻结字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    pub reason: &'static str,
}

/// tool 名合法字符（lowercase 后）：小写字母 / 数字 / `_` / `-`。
/// MCP 工具名约定集；越界字符（含空白、标点、非 ASCII）一律 fail-closed。
fn is_legal_tool_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'
}

/// 唯一策略判定点（§2.3）。`server_name` 入参保留签名稳定性，v1 规则只看 tool 名
/// （冻结行为有防漂移测试锁定）；tool 名先 trim + lowercase 再校验，因此
/// `" GET_Weather "` 与 `"get_weather"` 判定一致。
pub fn evaluate_tool_call(_server_name: &str, tool_name: &str) -> Decision {
    let trimmed = tool_name.trim();
    if trimmed.is_empty() {
        return Decision {
            allowed: false,
            reason: DENY_INVALID,
        };
    }
    let lowered = trimmed.to_ascii_lowercase();
    // trim 之后仍含空白（内部空格 / 制表符 / 换行）→ 名字本身不可信。
    if lowered.chars().any(char::is_whitespace) {
        return Decision {
            allowed: false,
            reason: DENY_INVALID,
        };
    }
    if !lowered.chars().all(is_legal_tool_char) {
        return Decision {
            allowed: false,
            reason: DENY_INVALID,
        };
    }
    let first_segment = lowered.split(['_', '-']).next().unwrap_or("");
    if READONLY_VERBS.contains(&first_segment) {
        Decision {
            allowed: true,
            reason: ALLOW_READONLY,
        }
    } else {
        Decision {
            allowed: false,
            reason: DENY_NOT_READONLY,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 正例表：12 个动词 × 大小写 / 分隔符 / 空白变体。
    #[test]
    fn readonly_verbs_allow_table() {
        for tool in [
            "get",
            "GET",
            "Get_Weather",
            "list-files",
            "LIST",
            "search_web",
            "read",
            "FETCH_URL",
            "query",
            "find-user",
            "describe",
            "INFO",
            "status",
            "health",
            "stats_daily",
            "  stats  ",
        ] {
            let decision = evaluate_tool_call("weather", tool);
            assert!(decision.allowed, "{tool:?} 应放行");
            assert_eq!(decision.reason, ALLOW_READONLY, "{tool:?}");
        }
    }

    /// 反例表：动词集之外默认拒绝（含首段为空的 `_hidden`）。
    #[test]
    fn non_readonly_tools_deny_table() {
        for tool in [
            "write_file",
            "delete",
            "create",
            "update",
            "send",
            "run",
            "exec",
            "set",
            "patch",
            "push",
            "install",
            "_hidden",
            "-x",
        ] {
            let decision = evaluate_tool_call("weather", tool);
            assert!(!decision.allowed, "{tool:?} 必须默认拒绝");
            assert_eq!(decision.reason, DENY_NOT_READONLY, "{tool:?}");
        }
    }

    /// 非法名 fail-closed：空名 / 内部空白 / 非法字符（含非 ASCII 与标点）。
    #[test]
    fn invalid_names_deny_table() {
        for tool in [
            "",
            "   ",
            "\t\n",
            "get forecast",
            "read\tme",
            "get.forecast",
            "fetch:url",
            "query-db!",
            "获取数据",
            "read/me",
            "café",
        ] {
            let decision = evaluate_tool_call("weather", tool);
            assert!(!decision.allowed, "{tool:?} 必须 fail-closed");
            assert_eq!(decision.reason, DENY_INVALID, "{tool:?}");
        }
    }

    /// 判定与 server 名无关（v1 冻结行为）；Decision 可比较、可拷贝。
    #[test]
    fn decision_is_server_name_independent_and_comparable() {
        assert_eq!(
            evaluate_tool_call("a", "Get_Forecast"),
            evaluate_tool_call("b", "get-forecast")
        );
        assert_ne!(
            evaluate_tool_call("a", "get_x"),
            evaluate_tool_call("a", "set_x")
        );
    }
}
