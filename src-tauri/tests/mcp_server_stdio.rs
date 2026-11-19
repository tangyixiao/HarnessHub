//! F22 S3 真机证据（spec §4.3）：spawn 自身 exe 走完整的 stdio JSON-RPC 会话。
//!
//! 用 `CARGO_BIN_EXE_harness-hub` 启动 `--mcp-serve`，stdin/stdout 走真实管道：
//! initialize → tools/list 断言五工具 → tools/call 断言 JSON → EOF / shutdown
//! 优雅退出。前置状态**只用生产代码填充**（AGENTS.md 冷启动真路径纪律）：
//! - 建库 = 生产 `Database::open`（空文件跑完全部迁移）；
//! - harness 清单 = 应用启动同款 `ensure_local_target` + `reconcile_harnesses`；
//! - 项目 = `ProjectStore::upsert`（service 层调用的同一个生产写路径）；
//! - 会话 = `SessionService::start_from_installation`。
//!
//! 另有一条**完全空库**用例：子进程自己 `Database::open`（生产迁移），
//! initialize + tools/call 不炸（spec §4.3）。

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};

/// 每个用例独立的临时目录（与 `real_*` 系列同约定：temp_dir + 进程 id + uuid）。
fn temp_root(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "hh-mcp-server-{tag}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4().simple()
    ))
}

/// 一个跑起来的 `harness-hub --mcp-serve` 子进程。
struct McpServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl McpServer {
    fn spawn(db_path: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_harness-hub"))
            .arg("--mcp-serve")
            .env("HARNESS_HUB_DB_PATH", db_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn harness-hub --mcp-serve");
        let stdin = child.stdin.take().expect("拿到子进程 stdin");
        let stdout = BufReader::new(child.stdout.take().expect("拿到子进程 stdout"));
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, line: &str) {
        writeln!(self.stdin, "{line}").expect("写入子进程 stdin");
        self.stdin.flush().expect("flush 子进程 stdin");
    }

    fn request(&mut self, id: u64, method: &str, params: Value) {
        let line = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        })
        .to_string();
        self.send(&line);
    }

    fn notification(&mut self, method: &str) {
        let line = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": {},
        })
        .to_string();
        self.send(&line);
    }

    /// 读一行响应并断言它是合法 JSON-RPC 2.0。
    fn read_response(&mut self) -> Value {
        let mut line = String::new();
        self.stdout
            .read_line(&mut line)
            .expect("从子进程 stdout 读一行");
        let value: Value = serde_json::from_str(line.trim())
            .unwrap_or_else(|error| panic!("响应必须是合法 JSON 行：{error}；原文 {line:?}"));
        assert_eq!(value["jsonrpc"], "2.0", "响应必须是 JSON-RPC 2.0：{value}");
        value
    }

    /// 读一个成功响应并返回其 result（断言 id 匹配、没有 error）。
    fn read_result(&mut self, id: u64) -> Value {
        let response = self.read_response();
        assert_eq!(response["id"], id, "响应 id 必须匹配请求：{response}");
        assert!(
            response.get("error").is_none(),
            "不期望错误响应：{response}"
        );
        response["result"].clone()
    }

    /// 关 stdin（EOF）→ 排干 stdout → 等待退出。返回（退出码，stderr 文本）。
    fn finish(mut self) -> (Option<i32>, String) {
        drop(self.stdin);
        // EOF 之后服务端必须安静退出：任何多余的输出行都是协议缺陷
        // （一行足够抓现行；多余的行只会在 panic 消息里丢掉首行之外的内容）。
        let mut extra = String::new();
        let read = self
            .stdout
            .read_line(&mut extra)
            .expect("排干子进程 stdout");
        if read > 0 {
            panic!("EOF 后服务端不应再输出：{extra:?}");
        }
        let status = self.child.wait().expect("等待子进程退出");
        let mut stderr = String::new();
        if let Some(mut pipe) = self.child.stderr.take() {
            use std::io::Read;
            let _ = pipe.read_to_string(&mut stderr);
        }
        (status.code(), stderr)
    }
}

/// initialize → tools/list → tools/call 的完整会话：带 seed 的真路径。
#[test]
fn serves_the_readonly_tool_catalog_over_stdio() {
    let root = temp_root("seeded");
    std::fs::create_dir_all(&root).expect("创建临时目录");
    let db_path = root.join("harness-hub.sqlite3");

    // —— 冷启动真路径：只调用生产代码填充前置状态，不手写 SQL ——
    {
        let database = harness_hub_lib::db::Database::open(&db_path).expect("生产迁移建库");
        let connection = database.connection();
        harness_hub_lib::runtime::local::ensure_local_target(connection)
            .expect("引导 local runtime target");

        // 应用启动同款的清单同步（harness::inventory 的「明确写路径」）。
        let registry = harness_hub_lib::build_harness_registry();
        let summaries = registry.summaries(harness_hub_lib::runtime::local::LOCAL_TARGET_ID);
        harness_hub_lib::harness::inventory::reconcile_harnesses(
            connection,
            &summaries,
            harness_hub_lib::runtime::local::LOCAL_TARGET_ID,
            "2026-11-14T00:00:00Z",
        )
        .expect("同步 harness 清单");

        harness_hub_lib::project::store::ProjectStore::new(connection)
            .upsert(
                "proj-mcp-1",
                "MCP Demo",
                root.to_string_lossy().as_ref(),
                "2026-11-14T00:00:00Z",
            )
            .expect("生产写路径登记项目");

        harness_hub_lib::session::SessionService::new(connection)
            .start_from_installation(
                "codex@local",
                Some("proj-mcp-1"),
                Some(root.to_string_lossy().as_ref()),
            )
            .expect("生产写路径登记会话");
    }
    // database 在块尾 drop：释放文件锁，子进程才能打开同一个库。

    let mut server = McpServer::spawn(&db_path);

    // initialize：受支持版本原样回 + capabilities.tools + serverInfo（spec §4.2）。
    server.request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "integration-test", "version": "0.0.0"},
        }),
    );
    let initialize = server.read_result(1);
    assert_eq!(initialize["protocolVersion"], "2025-06-18");
    assert!(
        initialize["capabilities"]["tools"].is_object(),
        "capabilities.tools 必须存在：{initialize}"
    );
    assert_eq!(initialize["serverInfo"]["name"], "harness-hub");
    assert_eq!(
        initialize["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );

    // notifications/initialized：通知不回包（下一个响应必须是 id=2）。
    server.notification("notifications/initialized");

    // tools/list：恰好五个只读工具（spec §4.2 白名单）。
    server.request(2, "tools/list", json!({}));
    let tools = server.read_result(2);
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .expect("tools 必须是数组")
        .iter()
        .map(|tool| tool["name"].as_str().expect("每个工具必须有 name"))
        .collect();
    assert_eq!(
        names,
        vec![
            "harness_list",
            "project_list",
            "session_list",
            "usage_summary",
            "activity_timeline",
        ]
    );

    // tools/call harness_list：持久化清单投影里有 codex 定义（真实检测 → reconcile）。
    server.request(
        3,
        "tools/call",
        json!({"name": "harness_list", "arguments": {}}),
    );
    let harnesses = server.read_result(3);
    assert_eq!(harnesses["isError"], false);
    assert_eq!(harnesses["content"][0]["type"], "text");
    let text = harnesses["content"][0]["text"]
        .as_str()
        .expect("text block");
    let payload: Value = serde_json::from_str(text).expect("工具结果必须是 JSON 串（spec §4.2）");
    let definitions = payload["definitions"].as_array().expect("definitions 数组");
    assert!(
        definitions
            .iter()
            .any(|definition| definition["id"] == "codex"),
        "codex 定义应在持久化清单里：{payload}"
    );

    // tools/call project_list：生产 seed 出的项目。
    server.request(
        4,
        "tools/call",
        json!({"name": "project_list", "arguments": {}}),
    );
    let projects = server.read_result(4);
    assert_eq!(projects["isError"], false);
    let payload: Value =
        serde_json::from_str(projects["content"][0]["text"].as_str().expect("text block"))
            .expect("工具结果必须是 JSON 串");
    let rows = payload.as_array().expect("project_list 返回数组");
    assert!(
        rows.iter().any(|project| project["id"] == "proj-mcp-1"),
        "seed 的项目应出现：{payload}"
    );
    assert!(
        rows.iter().all(|project| project.get("repoRoot").is_some()),
        "ProjectRecord 的 camelCase 投影：{payload}"
    );

    // tools/call session_list：limit 传了也传了多余键 —— 多余键忽略、limit clamp。
    server.request(
        5,
        "tools/call",
        json!({"name": "session_list", "arguments": {"limit": 3, "unknown_key": true}}),
    );
    let sessions = server.read_result(5);
    assert_eq!(sessions["isError"], false);
    let payload: Value =
        serde_json::from_str(sessions["content"][0]["text"].as_str().expect("text block"))
            .expect("工具结果必须是 JSON 串");
    let rows = payload.as_array().expect("session_list 返回数组");
    assert_eq!(rows.len(), 1, "生产 seed 恰好一条会话：{payload}");
    assert!(
        rows.iter()
            .any(|session| session.get("hubSessionId").is_some()),
        "SessionRecord 的 camelCase 投影：{payload}"
    );

    // 未知名工具：isError:true + 文本，进程不崩（spec §4.2）。
    server.request(
        6,
        "tools/call",
        json!({"name": "not_a_tool", "arguments": {}}),
    );
    let unknown = server.read_result(6);
    assert_eq!(unknown["isError"], true);
    let text = unknown["content"][0]["text"].as_str().expect("text block");
    assert_eq!(text, "unknown tool: not_a_tool");

    // ping：空对象结果。
    server.request(7, "ping", json!({}));
    let ping = server.read_result(7);
    assert!(ping.is_object(), "ping 返回空结果对象：{ping}");

    // EOF：关 stdin → 退出码 0。
    let (code, stderr) = server.finish();
    assert_eq!(code, Some(0), "EOF 必须优雅退出 0；stderr: {stderr}");
}

/// 冷启动真路径：完全空库（子进程自己跑生产迁移）下 initialize + tools/call 不炸。
#[test]
fn a_fresh_database_is_served_without_crashing() {
    let root = temp_root("cold");
    std::fs::create_dir_all(&root).expect("创建临时目录");
    let db_path = root.join("harness-hub.sqlite3");
    // 故意不预建库：子进程的 Database::open 就是冷启动生产路径。

    let mut server = McpServer::spawn(&db_path);

    // 未知版本 → 回首选版本（客户端自行断，spec §4.2）。
    server.request(1, "initialize", json!({"protocolVersion": "2099-01-01"}));
    let initialize = server.read_result(1);
    assert_eq!(initialize["protocolVersion"], "2025-06-18");

    server.request(
        2,
        "tools/call",
        json!({"name": "harness_list", "arguments": {}}),
    );
    let harnesses = server.read_result(2);
    assert_eq!(harnesses["isError"], false);
    let payload: Value = serde_json::from_str(
        harnesses["content"][0]["text"]
            .as_str()
            .expect("text block"),
    )
    .expect("工具结果必须是 JSON 串");
    assert_eq!(
        payload["definitions"]
            .as_array()
            .expect("definitions")
            .len(),
        0,
        "空库下清单为空：{payload}"
    );

    server.request(
        3,
        "tools/call",
        json!({"name": "session_list", "arguments": {}}),
    );
    let sessions = server.read_result(3);
    assert_eq!(sessions["isError"], false);
    let payload: Value =
        serde_json::from_str(sessions["content"][0]["text"].as_str().expect("text block"))
            .expect("工具结果必须是 JSON 串");
    assert_eq!(payload.as_array().expect("数组").len(), 0, "{payload}");

    // 未知方法 → -32601，且进程继续服务下一个请求（spec §4.2）。
    server.request(4, "no/such/method", json!({}));
    let missing = server.read_response();
    assert_eq!(missing["id"], 4);
    assert_eq!(missing["error"]["code"], -32601);

    server.request(5, "ping", json!({}));
    server.read_result(5);

    // shutdown：先回包，再优雅退出（spec §4.2）。
    server.request(6, "shutdown", json!({}));
    let shutdown = server.read_result(6);
    assert!(shutdown.is_object(), "shutdown 回包：{shutdown}");
    let (code, stderr) = server.finish();
    assert_eq!(code, Some(0), "shutdown 必须优雅退出 0；stderr: {stderr}");
}
