//! 真机只读版本检测；不启动 TUI、不读取认证、不发送模型请求。
use harness_hub_lib::build_harness_registry;
use harness_hub_lib::harness::adapter::HarnessCapabilities;
use harness_hub_lib::runtime::local::LOCAL_TARGET_ID;

#[test]
fn the_real_machine_detects_gemini_with_terminal_lifecycle_capabilities() {
    let summary = build_harness_registry()
        .summaries(LOCAL_TARGET_ID)
        .into_iter()
        .find(|summary| summary.id == "gemini")
        .expect("Gemini 已注册");
    assert_eq!(summary.installation_id.as_deref(), Some("gemini@local"));
    // launch / terminal 由 headless 真机 lifecycle 证据支撑（ADR-0005 的静态事实，
    // 与本机是否装了 Gemini 无关）；usage 与其余能力仍未验收。
    let expected = HarnessCapabilities {
        launch: true,
        terminal: true,
        ..HarnessCapabilities::default()
    };
    assert_eq!(summary.capabilities, expected);
    if !summary.installed {
        eprintln!("SKIP: Gemini CLI 不在本机 PATH 上；不算真机检测通过");
        return;
    }
    let binary = summary.binary_path.as_deref().expect("已安装就有路径");
    let version = summary.version.as_deref().expect("真机必须能读出版本");
    assert!(std::path::Path::new(binary).is_file());
    if cfg!(windows) {
        assert!(
            matches!(
                std::path::Path::new(binary)
                    .extension()
                    .and_then(|ext| ext.to_str()),
                Some("cmd" | "exe" | "bat" | "com" | "ps1")
            ),
            "必须选择 Windows shim：{binary}"
        );
    }
    assert!(version.chars().next().is_some_and(|ch| ch.is_ascii_digit()));
    eprintln!(
        "Gemini: installed=true binary={binary} version={version} data={:?}; capabilities=launch+terminal",
        summary.data_paths
    );
}
