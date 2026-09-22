//! 真实宿主冒烟测试：通过 `SystemHostProbe` 检测本机 Codex。
//!
//! 断言刻意是**条件式**的：没装 Codex 的机器（例如 CI）同样必须通过，
//! 否则这个测试会退化成「环境检查」而不是「行为回归」。
//!
//! 本机证据（2026-09-21）：codex 由 npm 全局安装，同时提供
//! `D:\npm-global\codex`（POSIX bash shim，Windows 无法执行）、`codex.cmd`、`codex.ps1`。
//! 真机验证曾发现：优先命中无扩展名 shim 会让版本永远读不出来。

use std::path::Path;
use std::sync::Arc;

use harness_hub_lib::harness::adapter::HarnessAdapter;
use harness_hub_lib::harness::adapters::codex::CodexAdapter;
use harness_hub_lib::harness::probe::SystemHostProbe;

/// Windows 上可被 CreateProcess 直接执行的扩展名。
#[cfg(windows)]
fn is_windows_executable(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("cmd" | "exe" | "bat" | "com")
    )
}

#[test]
fn detects_codex_without_lying_about_the_host() {
    let adapter = CodexAdapter::new(Arc::new(SystemHostProbe::new()));

    let result = adapter.detect();

    if !result.installed {
        // 没检测到：不得同时给出 binary_path（不许自相矛盾）。
        assert!(result.binary_path.is_none());
        return;
    }

    let binary = result
        .binary_path
        .expect("installed 为真时必须给出 binary_path");
    let path = Path::new(&binary);
    assert!(path.is_file(), "上报的 binary 必须真实存在：{binary}");

    // 真机回归锁：如果选中的是 Windows 可执行文件，就必须能读出它的版本。
    // 选错文件（例如无扩展名的 bash shim）时这条会失败 —— 这正是当初的缺陷。
    #[cfg(windows)]
    if is_windows_executable(path) {
        assert!(
            result.version.is_some(),
            "已选中可执行的 {binary}，却读不出 --version，说明候选顺序或解析有问题"
        );
    }

    // 版本可能读不到（例如只有 .ps1 的机器无法直接 CreateProcess），
    // 但只要能读到，就必须是版本号形状。
    if let Some(version) = &result.version {
        let first = version.chars().next().expect("版本号不应为空串");
        assert!(
            first.is_ascii_digit(),
            "版本号应以数字开头，实际：{version}"
        );
    }

    for dir in &result.data_paths {
        assert!(Path::new(dir).is_dir(), "上报的数据目录必须真实存在：{dir}");
    }
}

/// Windows 上不得把无扩展名的 POSIX shim 当作可执行文件上报。
///
/// 只有同时存在真实可执行扩展名（`.cmd` / `.exe` / …）时才断言：
/// 若机器上确实只有 bash shim，那是环境限制，不是本测试要覆盖的行为。
#[cfg(windows)]
#[test]
fn prefers_a_real_windows_executable_over_the_bare_bash_shim() {
    let adapter = CodexAdapter::new(Arc::new(SystemHostProbe::new()));
    let result = adapter.detect();

    let Some(binary) = result.binary_path.as_deref() else {
        return;
    };
    let path = Path::new(binary);

    let parent = match path.parent() {
        Some(parent) => parent,
        None => return,
    };
    let stem = match path.file_stem().and_then(|stem| stem.to_str()) {
        Some(stem) => stem,
        None => return,
    };

    let has_real_sibling = ["cmd", "exe", "bat", "com"]
        .iter()
        .any(|extension| parent.join(format!("{stem}.{extension}")).is_file());

    if has_real_sibling {
        assert!(
            is_windows_executable(path),
            "同目录存在可执行的 {stem}.cmd/.exe，却选了 {binary}"
        );
    }
}
