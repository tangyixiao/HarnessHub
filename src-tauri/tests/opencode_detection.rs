//! OpenCode 真机只读版本检测；不启动 TUI、不读取认证或配置、不发送模型请求。
//!
//! OpenCode 是本机唯一由 Scoop 提供的原生 `.exe` Harness（没有 `.cmd` / `.ps1`
//! 三件套），因此这里额外断言报告出来的每个数据目录**真实存在**，
//! 避免适配器把不存在的 XDG 根也算成「发现」。
use harness_hub_lib::build_harness_registry;
use harness_hub_lib::harness::adapter::HarnessCapabilities;
use harness_hub_lib::runtime::local::LOCAL_TARGET_ID;

#[test]
fn the_real_machine_detects_opencode_without_claiming_runtime_support() {
    let summary = build_harness_registry()
        .summaries(LOCAL_TARGET_ID)
        .into_iter()
        .find(|summary| summary.id == "opencode")
        .expect("OpenCode 已注册");
    assert_eq!(summary.installation_id.as_deref(), Some("opencode@local"));
    // 检测阶段零宣称（ADR-0005）
    assert_eq!(summary.capabilities, HarnessCapabilities::default());
    if !summary.installed {
        eprintln!("SKIP: OpenCode CLI 不在本机 PATH 上；不算真机检测通过");
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
            "必须选择 Windows 可执行 shim：{binary}"
        );
    }
    assert!(version.chars().next().is_some_and(|ch| ch.is_ascii_digit()));

    // 路径发现只报真实存在的目录（data / config / state / cache 中存在的那些）。
    for path in &summary.data_paths {
        assert!(
            std::path::Path::new(path).is_dir(),
            "报告了不存在的数据目录：{path}"
        );
    }

    eprintln!(
        "OpenCode: installed=true binary={binary} version={version} data={:?}; capabilities=all false",
        summary.data_paths
    );
}
