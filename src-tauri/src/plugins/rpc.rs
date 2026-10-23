//! 行分隔的 JSON-RPC 2.0 编解码——**测试先行**。
//!
//! 传输层是「一行一个 JSON 对象」的 stdio 管道，所以编解码器必须保证：
//! 编码结果里绝不能出现换行（否则一条消息会被拆成两条），解码必须容忍 CRLF。

use crate::error::Result;
use serde_json::Value;

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INTERNAL_ERROR: i64 = -32603;
pub const NOT_SUPPORTED: i64 = -32001;
pub const CAPABILITY_DENIED: i64 = -32002;
pub const NOT_IMPLEMENTED: i64 = -32003;

#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Response {
        id: Value,
        result: Value,
        error: Option<RpcError>,
    },
    Notification {
        method: String,
        params: Value,
    },
}

pub fn decode_line(line: &str) -> Result<Option<Incoming>> {
    // 空行是保活/尾随换行，不是错误：读循环靠 Ok(None) 跳过它继续等。
    let trimmed = line.trim_matches(['\r', '\n', ' ', '\t']);
    if trimmed.is_empty() {
        return Ok(None);
    }

    let value: Value = serde_json::from_str(trimmed).map_err(protocol_error)?;

    // 先卡 jsonrpc 字段：没有它的对象不是 JSON-RPC，别继续往下猜。
    let object = value
        .as_object()
        .ok_or_else(|| protocol_error_str("一行必须是一个 JSON 对象"))?;

    match object.get("jsonrpc").and_then(Value::as_str) {
        Some("2.0") => {}
        _ => return Err(protocol_error_str(r#"缺少 jsonrpc: "2.0""#)),
    }

    let has_id = object.contains_key("id");
    let method = object.get("method");
    let has_result = object.contains_key("result");
    let has_error = object.contains_key("error");

    match (method, has_result || has_error) {
        // 带 method 且不带 result/error：请求或通知，区别只在有没有 id。
        (Some(_), false) => {
            let method = object
                .get("method")
                .and_then(Value::as_str)
                .ok_or_else(|| protocol_error_str("method 必须是字符串"))?;
            if method.is_empty() {
                return Err(protocol_error_str("method 不能为空"));
            }

            // params 缺省统一成 Null：dispatch 只需要处理「没有参数」一种形态。
            let params = object.get("params").cloned().unwrap_or(Value::Null);

            if has_id {
                Ok(Some(Incoming::Request {
                    id: object["id"].clone(),
                    method: method.to_string(),
                    params,
                }))
            } else {
                Ok(Some(Incoming::Notification {
                    method: method.to_string(),
                    params,
                }))
            }
        }

        // 带 result 或 error：必须是响应，且 id 与 method 互斥。
        (None, true) => {
            let id = object
                .get("id")
                .cloned()
                .ok_or_else(|| protocol_error_str("响应缺少 id"))?;

            if has_error && has_result {
                return Err(protocol_error_str("响应不能同时带 result 与 error"));
            }

            let error = if has_error {
                Some(decode_error(&object["error"])?)
            } else {
                None
            };

            Ok(Some(Incoming::Response {
                id,
                // 错误响应没有 result：给 Null，免得调用方拿到一个假的 {}。
                result: object.get("result").cloned().unwrap_or(Value::Null),
                error,
            }))
        }

        // 两者都没有，或者 method 与 result 混在一起：都不成立。
        (Some(_), true) => Err(protocol_error_str("method 与 result/error 不能同时出现")),
        (None, false) => Err(protocol_error_str("既不是请求也不是响应")),
    }
}

fn decode_error(value: &Value) -> Result<RpcError> {
    let object = value
        .as_object()
        .ok_or_else(|| protocol_error_str("error 必须是对象"))?;

    let code = object
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| protocol_error_str("error.code 必须是整数"))?;

    let message = object
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| protocol_error_str("error.message 必须是字符串"))?;

    Ok(RpcError::new(code, message))
}

fn protocol_error(error: serde_json::Error) -> crate::error::Error {
    crate::error::Error::PluginProtocol(format!("JSON 解析失败：{error}"))
}

fn protocol_error_str(detail: &str) -> crate::error::Error {
    crate::error::Error::PluginProtocol(detail.to_string())
}

/// 编码成一行。`serde_json::to_string` 保证不产生裸换行（转义成 `\n`），
/// 所以「消息体里没有换行」这一条是天然成立的，末尾只需补一个分隔换行。
fn encode_line(value: &Value) -> String {
    let mut line = serde_json::to_string(value).expect("JSON 值序列化不会失败");
    line.push('\n');
    line
}

pub fn encode_request(id: u64, method: &str, params: &Value) -> String {
    encode_line(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
}

pub fn encode_notification(method: &str, params: &Value) -> String {
    encode_line(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    }))
}

pub fn encode_response_ok(id: &Value, result: &Value) -> String {
    encode_line(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }))
}

pub fn encode_response_err(id: &Value, error: &RpcError) -> String {
    encode_line(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": error.code,
            "message": error.message,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use serde_json::json;

    // ---------- decode：三种消息形态 ----------

    #[test]
    fn decodes_a_request_with_params() {
        let line =
            r#"{"jsonrpc":"2.0","id":7,"method":"usage/import","params":{"since":"2026-10-01"}}"#;
        let incoming = decode_line(line).expect("合法请求").expect("非空行");

        assert_eq!(
            incoming,
            Incoming::Request {
                id: json!(7),
                method: "usage/import".to_string(),
                params: json!({"since": "2026-10-01"}),
            }
        );
    }

    #[test]
    fn decodes_a_request_without_params_as_null() {
        // params 是可选的；缺省时统一成 Null，让 dispatch 只有一种「没有参数」形态。
        let line = r#"{"jsonrpc":"2.0","id":"abc","method":"skill/list"}"#;
        let incoming = decode_line(line).expect("合法请求").expect("非空行");

        assert_eq!(
            incoming,
            Incoming::Request {
                id: json!("abc"),
                method: "skill/list".to_string(),
                params: Value::Null,
            }
        );
    }

    #[test]
    fn decodes_a_notification() {
        let line = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
        let incoming = decode_line(line).expect("合法通知").expect("非空行");

        assert_eq!(
            incoming,
            Incoming::Notification {
                method: "initialized".to_string(),
                params: json!({}),
            }
        );
    }

    #[test]
    fn decodes_a_success_response() {
        let line = r#"{"jsonrpc":"2.0","id":3,"result":{"ok":true}}"#;
        let incoming = decode_line(line).expect("合法响应").expect("非空行");

        assert_eq!(
            incoming,
            Incoming::Response {
                id: json!(3),
                result: json!({"ok": true}),
                error: None,
            }
        );
    }

    #[test]
    fn decodes_an_error_response_into_rpc_error() {
        let line = r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32001,"message":"不支持"}}"#;
        let incoming = decode_line(line).expect("合法错误响应").expect("非空行");

        match incoming {
            Incoming::Response { id, result, error } => {
                assert_eq!(id, json!(3));
                assert_eq!(result, Value::Null);
                assert_eq!(error, Some(RpcError::new(NOT_SUPPORTED, "不支持")));
            }
            other => panic!("解析成了 {other:?}"),
        }
    }

    // ---------- decode：空行与脏数据 ----------

    #[test]
    fn empty_lines_are_skipped_not_errors() {
        // 读循环靠 Ok(None) 区分「没事发生」与「出错了」。
        for line in ["", "   ", "\t", "\r\n", " \r\n "] {
            assert_eq!(decode_line(line).expect("空行不是错误"), None, "{line:?}");
        }
    }

    #[test]
    fn crlf_is_tolerated() {
        let line = "{\"jsonrpc\":\"2.0\",\"method\":\"initialized\",\"params\":{}}\r";
        assert!(matches!(
            decode_line(line).expect("Windows 换行不该让整条消息报废"),
            Some(Incoming::Notification { .. })
        ));
    }

    #[test]
    fn bad_json_is_a_protocol_error_not_a_panic() {
        for line in ["{", "not json", "{\"jsonrpc\":\"2.0\",", "[]", "null", "42"] {
            let error = decode_line(line).expect_err(&format!("{line:?} 必须报错"));
            assert!(
                matches!(error, Error::PluginProtocol(_)),
                "{line:?} 应报 PluginProtocol，实际 {error:?}"
            );
        }
    }

    #[test]
    fn missing_or_wrong_jsonrpc_field_is_a_protocol_error() {
        for line in [
            r#"{"id":1,"method":"skill/list"}"#,                 // 缺 jsonrpc
            r#"{"jsonrpc":"1.0","id":1,"method":"skill/list"}"#, // 版本不对
            r#"{"jsonrpc":2.0,"id":1,"method":"skill/list"}"#,   // 不是字符串
        ] {
            let error = decode_line(line).expect_err("jsonrpc 字段必须严格");
            assert!(matches!(error, Error::PluginProtocol(_)), "{error:?}");
        }
    }

    #[test]
    fn request_without_a_valid_method_is_a_protocol_error() {
        for line in [
            r#"{"jsonrpc":"2.0","id":1}"#,             // 既不是请求也不是响应
            r#"{"jsonrpc":"2.0","id":1,"method":42}"#, // method 不是字符串
            r#"{"jsonrpc":"2.0","id":1,"method":""}"#, // 空方法名
            r#"{"jsonrpc":"2.0","method":"x","id":1,"result":null,"error":null}"#, // 请求混了响应字段
        ] {
            let error = decode_line(line).expect_err("畸形请求必须拒绝");
            assert!(
                matches!(error, Error::PluginProtocol(_)),
                "{line} → {error:?}"
            );
        }
    }

    #[test]
    fn response_with_neither_result_nor_error_is_a_protocol_error() {
        let error =
            decode_line(r#"{"jsonrpc":"2.0","id":5}"#).expect_err("响应必须有 result 或 error");
        assert!(matches!(error, Error::PluginProtocol(_)), "{error:?}");
    }

    #[test]
    fn malformed_error_object_is_a_protocol_error() {
        for line in [
            r#"{"jsonrpc":"2.0","id":5,"error":{"message":"没有 code"}}"#,
            r#"{"jsonrpc":"2.0","id":5,"error":{"code":"-32001","message":"code 不是整数"}}"#,
            r#"{"jsonrpc":"2.0","id":5,"error":{"code":-32001}}"#,
        ] {
            let error = decode_line(line).expect_err("error 对象必须完整");
            assert!(
                matches!(error, Error::PluginProtocol(_)),
                "{line} → {error:?}"
            );
        }
    }

    // ---------- encode ----------

    #[test]
    fn encoded_lines_end_with_exactly_one_newline() {
        let params = json!({"a": 1});
        for line in [
            encode_request(1, "initialize", &params),
            encode_notification("initialized", &params),
            encode_response_ok(&json!(1), &json!({"ok": true})),
            encode_response_err(&json!(1), &RpcError::new(INTERNAL_ERROR, "boom")),
        ] {
            assert!(line.ends_with('\n'), "缺尾换行：{line:?}");
            assert_eq!(line.matches('\n').count(), 1, "多出换行会拆包：{line:?}");
            assert!(
                !line.trim_end().contains('\n'),
                "消息体里不能有换行：{line:?}"
            );
        }
    }

    #[test]
    fn newlines_inside_payload_are_escaped() {
        // 插件可能把多行 stderr 塞进 message；序列化必须转义，不能断行。
        let line = encode_response_err(&json!(9), &RpcError::new(INTERNAL_ERROR, "行1\n行2"));
        assert_eq!(line.matches('\n').count(), 1, "{line:?}");

        let decoded = decode_line(&line).expect("可解").expect("非空");
        match decoded {
            Incoming::Response { error, .. } => {
                assert_eq!(error, Some(RpcError::new(INTERNAL_ERROR, "行1\n行2")));
            }
            other => panic!("解析成了 {other:?}"),
        }
    }

    #[test]
    fn encode_request_round_trips() {
        let params = json!({"api_version": "1"});
        let line = encode_request(42, "initialize", &params);
        let decoded = decode_line(&line).expect("可解").expect("非空");

        assert_eq!(
            decoded,
            Incoming::Request {
                id: json!(42),
                method: "initialize".to_string(),
                params,
            }
        );
        // id 必须是 JSON 数字而不是字符串，否则插件端 `id` 回填会类型不符。
        assert!(line.contains("\"id\":42"), "{line}");
        assert!(line.contains("\"jsonrpc\":\"2.0\""), "{line}");
    }

    #[test]
    fn encode_notification_round_trips() {
        let line = encode_notification("initialized", &json!({}));
        let decoded = decode_line(&line).expect("可解").expect("非空");

        assert_eq!(
            decoded,
            Incoming::Notification {
                method: "initialized".to_string(),
                params: json!({}),
            }
        );
        assert!(!line.contains("\"id\""), "通知不能带 id：{line}");
    }

    #[test]
    fn encode_response_ok_round_trips() {
        let line = encode_response_ok(&json!("req-1"), &json!({"plugins": []}));
        let decoded = decode_line(&line).expect("可解").expect("非空");

        assert_eq!(
            decoded,
            Incoming::Response {
                id: json!("req-1"),
                result: json!({"plugins": []}),
                error: None,
            }
        );
        assert!(
            !line.contains("\"error\""),
            "成功响应不该带 error 键：{line}"
        );
    }

    #[test]
    fn encode_response_err_round_trips() {
        let error = RpcError::new(CAPABILITY_DENIED, "未授权 usage");
        let line = encode_response_err(&json!(7), &error);
        let decoded = decode_line(&line).expect("可解").expect("非空");

        assert_eq!(
            decoded,
            Incoming::Response {
                id: json!(7),
                result: Value::Null,
                error: Some(error),
            }
        );
        assert!(line.contains("-32002"), "{line}");
    }

    // ---------- 错误码常量 ----------

    #[test]
    fn error_codes_match_json_rpc_2_0_and_the_spec() {
        // 前四个是 JSON-RPC 2.0 保留码，后三个是规格里插件专用的服务端错误段。
        assert_eq!(PARSE_ERROR, -32700);
        assert_eq!(INVALID_REQUEST, -32600);
        assert_eq!(METHOD_NOT_FOUND, -32601);
        assert_eq!(INTERNAL_ERROR, -32603);
        assert_eq!(NOT_SUPPORTED, -32001);
        assert_eq!(CAPABILITY_DENIED, -32002);
        assert_eq!(NOT_IMPLEMENTED, -32003);
    }

    #[test]
    fn rpc_error_new_keeps_code_and_message() {
        let error = RpcError::new(NOT_IMPLEMENTED, String::from("以后再说"));
        assert_eq!(error.code, NOT_IMPLEMENTED);
        assert_eq!(error.message, "以后再说");
    }
}
