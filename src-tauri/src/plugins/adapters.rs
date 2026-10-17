//! v0.1 支持范围与「不支持」应答——**测试先行**。

use crate::plugins::rpc::{RpcError, NOT_SUPPORTED};

pub const USAGE_IMPORT: &str = "usage/import";
pub const SKILL_LIST: &str = "skill/list";
pub const SKILL_GET: &str = "skill/get";

/// v0.1 只打通这两条通路；其余四类前缀一律 not_supported。
///
/// 精确匹配而不是前缀匹配：`usage/import/` / `Usage/Import` 这类近似写法
/// 必须落进「不认识」，否则拼写错误会变成静默不响应。
pub fn is_supported_in_v01(method: &str) -> bool {
    matches!(method, USAGE_IMPORT | SKILL_LIST | SKILL_GET)
}

/// 构造 `not_supported` 应答。
///
/// 信息里带方法名：插件作者在日志里只能看到这一句，
/// 不点名就无从知道自己调用了哪条 v0.1 还没接的通路。
pub fn not_supported_error(method: &str) -> RpcError {
    RpcError::new(
        NOT_SUPPORTED,
        format!("{method} 不在 v0.1 支持范围内（仅 usage/import、skill/list、skill/get）"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::rpc::NOT_IMPLEMENTED;

    #[test]
    fn method_names_match_the_spec() {
        // 这些字符串同时出现在插件侧、manifest capability 映射和 TS 侧，
        // 拼错就是静默不响应，所以用测试钉死。
        assert_eq!(USAGE_IMPORT, "usage/import");
        assert_eq!(SKILL_LIST, "skill/list");
        assert_eq!(SKILL_GET, "skill/get");
    }

    #[test]
    fn only_usage_import_and_skill_methods_are_wired_in_v01() {
        for method in [USAGE_IMPORT, SKILL_LIST, SKILL_GET] {
            assert!(is_supported_in_v01(method), "{method} 属于 v0.1 通路");
        }
    }

    #[test]
    fn the_four_other_prefixes_are_not_supported_in_v01() {
        // 规格：harness/*、profile/*、trace/*、runtime/* 在 v0.1 一律 not_supported。
        for method in [
            "harness/detect",
            "harness/launch",
            "harness/resume",
            "profile/list",
            "profile/activate",
            "trace/query",
            "runtime/info",
        ] {
            assert!(!is_supported_in_v01(method), "{method} 在 v0.1 不该被放行");
        }
    }

    #[test]
    fn unknown_and_malformed_methods_are_not_supported_in_v01() {
        for method in [
            "",
            "usage",
            "usage/",
            "Usage/Import",
            "usage/import/",
            "usage/importt",
            "skill",
            "skill/list2",
            "unknown/method",
        ] {
            assert!(!is_supported_in_v01(method), "{method:?} 必须拒绝");
        }
    }

    #[test]
    fn not_supported_error_carries_the_code_and_names_the_method() {
        let error = not_supported_error("harness/launch");

        assert_eq!(error.code, NOT_SUPPORTED);
        assert!(
            error.message.contains("harness/launch"),
            "错误信息必须点名方法，否则调用方不知道该改什么：{}",
            error.message
        );
        // 刻意区分：not_supported 是「这版不做」，not_implemented 是「还没来得及做」。
        assert_ne!(error.code, NOT_IMPLEMENTED);
    }

    #[test]
    fn not_supported_error_is_per_call_not_shared() {
        // 两个不同方法必须得到两条各自点名的方法错误，不能共享同一个静态值。
        assert_ne!(
            not_supported_error("trace/query").message,
            not_supported_error("profile/list").message
        );
    }
}
