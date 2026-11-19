// Windows release 构建不弹出额外的控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // F22 S3（spec §4.1）：headless MCP stdio 服务模式的唯一 argv 分流。
    // 命中 `--mcp-serve` 时**绝不初始化 Tauri Builder / webview / 托盘**，
    // 只跑只读工具的 stdio JSON-RPC 循环；EOF / shutdown 优雅退出。
    if std::env::args().nth(1).as_deref() == Some("--mcp-serve") {
        std::process::exit(harness_hub_lib::run_mcp_server());
    }
    harness_hub_lib::run();
}
