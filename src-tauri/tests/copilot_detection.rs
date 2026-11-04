//! GitHub Copilot CLI 真机只读版本检测；不启动 TUI、不读取认证、不发送模型请求。
//! 本机取证（2026-10-25）：PATH 上有 `D:\npm-global\copilot(.cmd)`，版本 1.0.91。
use harness_hub_lib::build_harness_registry;
use harness_hub_lib::harness::adapter::HarnessCapabilities;
use harness_hub_lib::runtime::local::LOCAL_TARGET_ID;

#[test]
fn the_real_machine_detects_copilot_without_claiming_runtime_support() {
    let summary = build_harness_registry()
        .summaries(LOCAL_TARGET_ID)
        .into_iter()
        .find(|summary| summary.id == "copilot")
        .expect("Copilot 已注册");
    assert_eq!(summary.display_name, "GitHub Copilot CLI");
    assert_eq!(summary.installation_id.as_deref(), Some("copilot@local"));
    // 检测阶段零宣称（ADR-0005）
    assert_eq!(summary.capabilities, HarnessCapabilities::default());
    if !summary.installed {
        eprintln!("SKIP: GitHub Copilot CLI 不在本机 PATH 上；不算真机检测通过");
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
    // 版本必须是纯 semver（真机形态 `1.0.91.`，尾点已在解析层剥掉）。
    assert!(
        version.chars().all(|ch| ch.is_ascii_digit() || ch == '.'),
        "版本含非语义化字符：{version}"
    );
    eprintln!(
        "Copilot: installed=true binary={binary} version={version} data={:?}; capabilities=all false",
        summary.data_paths
    );
}
