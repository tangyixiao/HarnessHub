// Windows release 构建不弹出额外的控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    harness_hub_lib::run();
}
