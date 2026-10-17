//! 六类 Adapter 的契约，以及 v0.1 的支持范围与「不支持」应答。
//!
//! 契约在这里**全冻结**（计划书 9.3 的 Adapter contracts 项），但 v0.1 只有两条
//! 通路有真实消费者：`usage/import`（connect 型）与 `skill/list` / `skill/get`
//! （skill 型）。其余四类先定形状、实现一律返回 `-32001 not_supported`——
//! 宁可让插件作者看到明确的「这版不做」，也不要让宿主猜一个语义出来。
//!
//! 为什么 trait 里只有方法、没有泛型参数与关联类型：宿主与插件之间传的是
//! JSON（`jsonrpc` 的 `params` / `result`），把载荷类型写成泛型只会逼着每个
//! 实现方在边界上再转换一次，而真正需要类型安全的地方在**宿主内部的归一化**
//! （`usage::ccusage`），不在这层线缆契约上。

use serde_json::Value;

use crate::plugins::rpc::{RpcError, NOT_SUPPORTED};

pub const USAGE_IMPORT: &str = "usage/import";
pub const SKILL_LIST: &str = "skill/list";
pub const SKILL_GET: &str = "skill/get";

/// Host 侧的 Adapter 调用统一用这个返回类型。
///
/// 写成别名而不是 `crate::error::Result` 的两参数版本，是因为这里的失败
/// **不是宿主内部错误**，而是要原样回给插件进程的 JSON-RPC 错误对象。
pub type AdapterResult = std::result::Result<Value, RpcError>;

/// 五类「连接外部世界」的 Adapter 共用的方法名。
///
/// harness/profile/trace/runtime 的方法名在此集中定义：它们同时出现在
/// manifest 的 capability 映射、SDK 文档与将来的实现里，散落各处就会漂移。
pub const HARNESS_DETECT: &str = "harness/detect";
pub const HARNESS_LAUNCH: &str = "harness/launch";
pub const HARNESS_RESUME: &str = "harness/resume";
pub const PROFILE_LIST: &str = "profile/list";
pub const PROFILE_ACTIVATE: &str = "profile/activate";
pub const TRACE_QUERY: &str = "trace/query";
pub const RUNTIME_INFO: &str = "runtime/info";

/// connect 型：把外部 usage 数据接进宿主。
///
/// 返回值形状在 spec §3 冻结为 ccusage 的 `{"session": [...]}`——
/// 宿主据此直接走现成的 `usage::parse_report`，插件作者不必学第二套 schema。
pub trait UsageAdapter {
    fn import(&mut self, params: &Value) -> AdapterResult;
}

/// skill 型：把 SKILL.md 指令包暴露给宿主。
///
/// `body` 是 SKILL.md **原文**：宿主只透传、不解释，也不替插件决定
/// frontmatter 与正文的分界。
pub trait SkillAdapter {
    fn list(&mut self) -> AdapterResult;
    fn get(&mut self, params: &Value) -> AdapterResult;
}

/// harness 型：探测 / 启动 / 恢复外部 Harness 会话。
pub trait HarnessAdapter {
    fn detect(&mut self, params: &Value) -> AdapterResult;
    fn launch(&mut self, params: &Value) -> AdapterResult;
    fn resume(&mut self, params: &Value) -> AdapterResult;
}

/// profile 型：Harness 的凭据/配置档案（v0.1 无消费者，HHAR 9.5 才接）。
pub trait ProfileAdapter {
    fn list(&mut self, params: &Value) -> AdapterResult;
    fn activate(&mut self, params: &Value) -> AdapterResult;
}

/// trace 型：会话事件流查询。
pub trait TraceAdapter {
    fn query(&mut self, params: &Value) -> AdapterResult;
}

/// runtime 型：运行时（Node/Python/…）探测。
pub trait RuntimeAdapter {
    fn info(&mut self, params: &Value) -> AdapterResult;
}

/// 四类未打通 Adapter 的统一实现：每个方法都返回点名该方法的 `not_supported`。
///
/// 存在的意义不是「占位」，而是**把「这版不做」变成可执行的契约**：
/// 将来谁接上 harness 通路，必须在这里换掉实现，而不是悄悄让调用落到
/// 一个空结果上（空结果会被前端当成「探测到零个 Harness」，比报错更难查）。
pub struct Unsupported;

impl HarnessAdapter for Unsupported {
    fn detect(&mut self, _params: &Value) -> AdapterResult {
        Err(not_supported_error(HARNESS_DETECT))
    }
    fn launch(&mut self, _params: &Value) -> AdapterResult {
        Err(not_supported_error(HARNESS_LAUNCH))
    }
    fn resume(&mut self, _params: &Value) -> AdapterResult {
        Err(not_supported_error(HARNESS_RESUME))
    }
}

impl ProfileAdapter for Unsupported {
    fn list(&mut self, _params: &Value) -> AdapterResult {
        Err(not_supported_error(PROFILE_LIST))
    }
    fn activate(&mut self, _params: &Value) -> AdapterResult {
        Err(not_supported_error(PROFILE_ACTIVATE))
    }
}

impl TraceAdapter for Unsupported {
    fn query(&mut self, _params: &Value) -> AdapterResult {
        Err(not_supported_error(TRACE_QUERY))
    }
}

impl RuntimeAdapter for Unsupported {
    fn info(&mut self, _params: &Value) -> AdapterResult {
        Err(not_supported_error(RUNTIME_INFO))
    }
}

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

    /// 六类 trait 的方法名必须与 `Capabilities::allows` 认的那套字符串一致。
    ///
    /// 两边各写一份字符串是刻意的（模块之间不互相 import 常量），代价是可能漂移；
    /// 这条测试就是那份代价的对冲。
    #[test]
    fn every_adapter_method_is_routed_by_the_capability_map() {
        use crate::plugins::manifest::Capabilities;

        let all_on = Capabilities {
            detect: true,
            launch: true,
            resume: true,
            usage: true,
            trace: true,
            profiles: true,
            mcp: true,
            skills: true,
        };

        for method in [
            HARNESS_DETECT,
            HARNESS_LAUNCH,
            HARNESS_RESUME,
            USAGE_IMPORT,
            TRACE_QUERY,
            PROFILE_LIST,
            PROFILE_ACTIVATE,
            SKILL_LIST,
            SKILL_GET,
        ] {
            assert!(
                all_on.allows(method),
                "{method} 必须能被某个 capability 放行，否则声明了也调不到"
            );
        }

        // 例外：`runtime/info` **没有**对应的 capability 键。
        //
        // 计划书 9.3 冻结的 `[capabilities]` 只有 detect/launch/resume/usage/trace/
        // profiles/mcp/skills 八个布尔——RuntimeAdapter 在计划书里是与它们并列的
        // 六类 Adapter 之一，却没有自己的开关。字面结论是：插件不能声明 runtime
        // 能力，RuntimeAdapter 是**宿主侧**的抽象（宿主探测本地 Node/Python 等运行
        // 时），不经过插件线缆。这里把它钉成断言，免得将来有人「顺手」给
        // `allows()` 加一条 runtime 分支却没人想过它该归哪个布尔。
        assert!(
            !all_on.allows(RUNTIME_INFO),
            "runtime/info 没有 capability 归属；若将来要开放，必须先改计划书的 capability 列表"
        );
    }

    /// 未打通的三类 Adapter 必须逐方法点名，而不是共享一句「不支持」。
    #[test]
    fn unsupported_adapters_name_the_exact_method_that_was_called() {
        use super::Unsupported;

        let mut adapter = Unsupported;

        let checks: Vec<(&str, AdapterResult)> = vec![
            (HARNESS_DETECT, adapter.detect(&Value::Null)),
            (HARNESS_LAUNCH, adapter.launch(&Value::Null)),
            (HARNESS_RESUME, adapter.resume(&Value::Null)),
            (PROFILE_LIST, adapter.list(&Value::Null)),
            (PROFILE_ACTIVATE, adapter.activate(&Value::Null)),
            (TRACE_QUERY, adapter.query(&Value::Null)),
            (RUNTIME_INFO, adapter.info(&Value::Null)),
        ];

        for (method, outcome) in checks {
            let error = outcome.expect_err("v0.1 必须返回错误而不是空结果");
            assert_eq!(error.code, NOT_SUPPORTED, "{method} 的错误码");
            assert!(
                error.message.contains(method),
                "{method} 的错误信息必须点名它自己：{}",
                error.message
            );
        }
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
