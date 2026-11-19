//! F22 S3：Harness Hub 作为 MCP Server —— headless stdio 模式（spec §4）。
//!
//! 入口：`harness-hub --mcp-serve`（`main.rs` argv 分流 → `lib.rs::run_mcp_server`）。
//!
//! **此进程模式不提供任何写路径**（spec §2.4 红线）：
//! - 主循环只持有 `&Connection`，结构上不可能写库（ADR-0012 同款纪律）；
//! - 工具目录是 [`tools`] 里的只读白名单（spec §4.2 五工具），没有其他入口；
//! - 只监听 stdio，绝不开端口（spec §2.6）；
//! - 绝不初始化 Tauri Builder / webview / 托盘（spec §4.1）。
//!
//! 协议 = 行分隔 JSON-RPC 2.0（MCP stdio 传输官方形态），读入复用
//! `plugins::rpc` 的 `Incoming` codec（spec §4.2）。
//!
//! 退出语义（spec §4.1 / §4.2）：EOF 或 `shutdown`（先回包）→ 优雅退出；
//! 写失败 / 读失败 → 立即退出。全部由 [`run_stdio`] 的返回值表达，
//! 进程退出码由 `run_mcp_server` 折算。

pub mod tools;

use std::io::{BufRead, Write};

use rusqlite::Connection;
use serde_json::{json, Value};

use crate::plugins::rpc::{self, Incoming, RpcError};

/// spec §4.2：initialize 支持的 MCP 规范版本（降序，首选在前）。
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// 支持集之外（含缺失）一律回首选版本，客户端自行断（spec §4.2）。
pub const PREFERRED_PROTOCOL_VERSION: &str = "2025-06-18";

/// JSON-RPC 2.0 标准码：参数非法（codec 常量表里没有这个码，server 方向专用）。
pub const INVALID_PARAMS: i64 = -32602;

/// `serverInfo.name`（spec §4.2）。
pub const SERVER_NAME: &str = "harness-hub";

/// stdio 服务端主循环。
///
/// 一行一个 JSON-RPC 2.0 消息；EOF / `shutdown` → `Ok(())`（优雅退出），
/// 读写失败 → `Err`（立即退出）。任何单个畸形消息都不终止循环。
pub fn run_stdio<R: BufRead, W: Write>(
    db: &Connection,
    mut reader: R,
    mut writer: W,
) -> std::io::Result<()> {
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            // EOF：对端关管 → 优雅退出（spec §4.1）。
            return Ok(());
        }

        match rpc::decode_line(&line) {
            // 空行是保活，不是错误。
            Ok(None) => {}
            // 通知（notifications/initialized 等）不回包（spec §4.2）。
            Ok(Some(Incoming::Notification { .. })) => {}
            // 服务端方向不应收到响应：按 JSON-RPC 惯例静默丢弃，不终止循环。
            Ok(Some(Incoming::Response { .. })) => {}
            Ok(Some(Incoming::Request { id, method, params })) => {
                match dispatch(db, &method, &params) {
                    Outcome::Reply(result) => {
                        write_line(&mut writer, &rpc::encode_response_ok(&id, &result))?;
                    }
                    Outcome::Error(error) => {
                        write_line(&mut writer, &rpc::encode_response_err(&id, &error))?;
                    }
                    Outcome::Shutdown(result) => {
                        // spec §4.2：shutdown 先回包，再优雅退出。
                        write_line(&mut writer, &rpc::encode_response_ok(&id, &result))?;
                        return Ok(());
                    }
                }
            }
            Err(error) => {
                // 畸形行（坏 JSON / 非 JSON-RPC）：回 id=null 的 parse error，
                // 循环继续 —— 单条脏数据绝不终止服务（codec 不保留原始 id，
                // 统一按 -32700 处理）。
                let error = RpcError::new(rpc::PARSE_ERROR, error.to_string());
                write_line(&mut writer, &rpc::encode_response_err(&Value::Null, &error))?;
            }
        }
    }
}

fn write_line<W: Write>(writer: &mut W, line: &str) -> std::io::Result<()> {
    // codec 保证行内无裸换行；写完立刻 flush，让客户端逐条可见。
    writer.write_all(line.as_bytes())?;
    // 写失败 → Err 上抛 → 进程退出（spec §4.1）。
    writer.flush()
}

/// 一次请求的处理结果。
enum Outcome {
    Reply(Value),
    Error(RpcError),
    Shutdown(Value),
}

fn dispatch(db: &Connection, method: &str, params: &Value) -> Outcome {
    match method {
        "initialize" => Outcome::Reply(initialize_result(params)),
        "ping" => Outcome::Reply(json!({})),
        "tools/list" => Outcome::Reply(tools::tools_list_result()),
        "tools/call" => match tools::tools_call_result(db, params) {
            Ok(result) => Outcome::Reply(result),
            Err(message) => Outcome::Error(RpcError::new(INVALID_PARAMS, message)),
        },
        "shutdown" => Outcome::Shutdown(json!({})),
        // spec §4.2：未知方法 -32601，进程不崩。
        _ => Outcome::Error(RpcError::new(
            rpc::METHOD_NOT_FOUND,
            format!("未知方法：{method}"),
        )),
    }
}

/// `initialize` 协商（spec §4.2）：请求版本 ∈ 支持集原样回；否则回首选。
/// 返回 `capabilities: { "tools": {} }` + `serverInfo`。
fn initialize_result(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    json!({
        "protocolVersion": negotiate_version(requested),
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// 版本协商（表驱动，spec §4.2）。
pub fn negotiate_version(requested: Option<&str>) -> &'static str {
    let Some(requested) = requested else {
        return PREFERRED_PROTOCOL_VERSION;
    };
    for version in SUPPORTED_PROTOCOL_VERSIONS {
        if version == requested {
            return version;
        }
    }
    PREFERRED_PROTOCOL_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;

    use crate::test_support::empty_db;

    fn serve(db: &Connection, input: &str) -> String {
        let mut output = Vec::new();
        run_stdio(db, Cursor::new(input.to_string()), &mut output).expect("循环正常结束");
        String::from_utf8(output).expect("输出是 UTF-8")
    }

    fn request(id: u64, method: &str, params: Value) -> String {
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
    }

    // ---------- 版本协商（表驱动，spec §4.2） ----------

    #[test]
    fn negotiate_version_table_driven() {
        for (requested, expected) in [
            // 支持集内原样回。
            (Some("2025-06-18"), "2025-06-18"),
            (Some("2025-03-26"), "2025-03-26"),
            (Some("2024-11-05"), "2024-11-05"),
            // 支持集外 / 缺失 → 首选。
            (Some("2099-01-01"), PREFERRED_PROTOCOL_VERSION),
            (Some(""), PREFERRED_PROTOCOL_VERSION),
            (Some("2025-06-18 "), PREFERRED_PROTOCOL_VERSION),
            (None, PREFERRED_PROTOCOL_VERSION),
        ] {
            assert_eq!(
                negotiate_version(requested),
                expected,
                "requested = {requested:?}"
            );
        }
    }

    /// initialize 的返回形状：capabilities.tools + serverInfo（spec §4.2）。
    #[test]
    fn initialize_result_shape() {
        let result = initialize_result(&json!({"protocolVersion": "2025-03-26"}));
        assert_eq!(result["protocolVersion"], "2025-03-26");
        assert!(result["capabilities"]["tools"].is_object());
        assert_eq!(result["serverInfo"]["name"], SERVER_NAME);
        assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(result.as_object().unwrap().len(), 3, "键集合冻结：{result}");
    }

    /// initialize 对畸形 params（缺 protocolVersion / 非对象）也必须能答。
    #[test]
    fn initialize_tolerates_malformed_params() {
        for params in [json!({}), Value::Null, json!({"protocolVersion": 42})] {
            let result = initialize_result(&params);
            assert_eq!(result["protocolVersion"], PREFERRED_PROTOCOL_VERSION);
        }
    }

    // ---------- codec 循环（内存 reader/writer 注入） ----------

    #[test]
    fn the_loop_answers_requests_and_ignores_notifications_and_garbage() {
        let db = empty_db();
        let input = [
            request(1, "initialize", json!({"protocolVersion": "2024-11-05"})),
            // 通知不回包。
            json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
                .to_string(),
            // 空行保活。
            String::new(),
            request(2, "ping", json!({})),
            // 畸形行：parse error，id=null，循环继续。
            "{oops".to_string(),
            // 服务端方向收到响应：静默丢弃。
            json!({"jsonrpc": "2.0", "id": 99, "result": {}}).to_string(),
            // 未知方法：-32601。
            request(3, "no/such/method", json!({})),
            // tools/call 未知工具：isError 信封（不是 JSON-RPC 错误）。
            request(4, "tools/call", json!({"name": "nope", "arguments": {}})),
            // tools/list：五工具。
            request(5, "tools/list", json!({})),
            // CRLF 容忍。
            format!("{}\r", request(6, "ping", json!({}))),
        ]
        .join("\n");

        let output = serve(db.connection(), &input);
        let lines: Vec<Value> = output
            .lines()
            .map(|line| serde_json::from_str(line).expect("每行都是合法 JSON"))
            .collect();

        assert_eq!(
            lines.len(),
            7,
            "通知 / 空行 / 客户端响应之外恰好 7 个响应（畸形行会产生一个 id=null 的 parse error）：{output}"
        );

        // 顺序 + id 一一对应。
        assert_eq!(lines[0]["id"], 1);
        assert_eq!(lines[0]["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(lines[1]["id"], 2);
        assert_eq!(lines[1]["result"], json!({}));
        assert_eq!(lines[2]["id"], Value::Null, "parse error 无 id");
        assert_eq!(lines[2]["error"]["code"], rpc::PARSE_ERROR);
        assert_eq!(lines[3]["id"], 3);
        assert_eq!(lines[3]["error"]["code"], rpc::METHOD_NOT_FOUND);
        assert_eq!(lines[4]["id"], 4);
        assert_eq!(lines[4]["result"]["isError"], true);
        assert_eq!(
            lines[4]["result"]["content"][0]["text"],
            "unknown tool: nope"
        );
        assert_eq!(lines[5]["id"], 5);
        assert_eq!(
            lines[5]["result"]["tools"]
                .as_array()
                .expect("tools 数组")
                .len(),
            5
        );
        assert_eq!(lines[6]["id"], 6);
        assert_eq!(lines[6]["result"], json!({}));
    }

    /// shutdown：先回包，再退出循环（spec §4.2）。
    #[test]
    fn shutdown_replies_then_stops_the_loop() {
        let db = empty_db();
        let input = [
            request(1, "ping", json!({})),
            request(2, "shutdown", json!({})),
            // shutdown 之后的请求永远不会再被回答。
            request(3, "ping", json!({})),
        ]
        .join("\n");

        let output = serve(db.connection(), &input);
        let lines: Vec<Value> = output
            .lines()
            .map(|line| serde_json::from_str(line).expect("每行都是合法 JSON"))
            .collect();
        assert_eq!(lines.len(), 2, "shutdown 后不再处理后续请求：{output}");
        assert_eq!(lines[1]["id"], 2);
        assert_eq!(lines[1]["result"], json!({}));
    }

    /// EOF：优雅退出，返回 Ok。
    #[test]
    fn eof_ends_the_loop_gracefully() {
        let db = empty_db();
        let output = serve(db.connection(), "");
        assert_eq!(output, "");
    }

    /// 写失败 → 立即退出（Err），而不是带着坏管道继续跑（spec §4.1）。
    #[test]
    fn a_write_failure_ends_the_loop_with_an_error() {
        struct BrokenWriter;
        impl Write for BrokenWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("管道已断"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::other("管道已断"))
            }
        }

        let db = empty_db();
        let input = format!("{}\n", request(1, "ping", json!({})));
        let error = run_stdio(db.connection(), Cursor::new(input), BrokenWriter)
            .expect_err("写失败必须上抛");
        assert!(error.to_string().contains("管道已断"), "{error}");
    }
}
