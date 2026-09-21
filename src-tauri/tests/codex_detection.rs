//! 真实宿主冒烟测试：通过 `SystemHostProbe` 检测本机 Codex。
//!
//! 断言刻意是**条件式**的：没装 Codex 的机器（例如 CI）同样必须通过，
//! 否则这个测试会退化成「环境检查」而不是「行为回归」。
//!
//! 本机证据（2026-09-21）：codex 由 npm 全局安装，同时提供
//! `D:\npm-global\codex`（bash shim）、`codex.cmd`、`codex.ps1`。

use std::path::Path;
use std::sync::Arc;

use harness_hub_lib::harness::adapter::HarnessAdapter;
use harness_hub_lib::harness::adapters::codex::CodexAdapter;
use harness_hub_lib::harness::probe::SystemHostProbe;

#[test]
fn detects_codex_without_lying_about_the_host() {
    let adapter = CodexAdapter::new(Arc::new(SystemHostProbe::new()));

    let result = adapter.detect();

    if !result.installed {
        // 没检测到：不得同时给出 binary_path（不许自相矛盾）。
        assert!(result.binary_path.is_none());
        return;
    }

    let binary = result.binary_path.expect("installed 为真时必须给出 binary_path");
    assert!(
        Path::new(&binary).is_file(),
        "上报的 binary 必须真实存在：{binary}"
    );

    // 版本可能读不到（例如只有 .ps1 的机器无法直接 CreateProcess），
    // 但只要能读到，就必须是版本号形状。
    if let Some(version) = &result.version {
        let first = version.chars().next().expect("版本号不应为空串");
        assert!(first.is_ascii_digit(), "版本号应以数字开头，实际：{version}");
    }

    for dir in &result.data_paths {
        assert!(
            Path::new(dir).is_dir(),
            "上报的数据目录必须真实存在：{dir}"
        );
    }
}
