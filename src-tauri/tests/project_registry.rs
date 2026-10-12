//! Project registry 真机只读验证：用真实的 `SystemGitRootProbe` 解析本仓库 root。
//! 不写任何数据库文件（内存库）、不改动 Git 仓库状态。
use harness_hub_lib::project::git_root::{GitRootProbe, SystemGitRootProbe};
use harness_hub_lib::project::service::ProjectService;
use harness_hub_lib::project::RegisterRequest;
use std::sync::Arc;

#[test]
fn the_real_machine_resolves_this_repository_root_and_registers_it() {
    // 本测试文件本身就在仓库里，用 CARGO_MANIFEST_DIR（src-tauri）向上找根。
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_dir = manifest_dir
        .parent()
        .expect("src-tauri 必然有父目录")
        .to_path_buf();

    let probe = Arc::new(SystemGitRootProbe);
    let root = probe
        .as_ref()
        .repo_root(&repo_dir)
        .expect("git 探测本身不应失败")
        .expect("本仓库必须能解析出 repo root");
    eprintln!("resolved repo root: {root}");
    assert!(root.contains("HarnessHub"), "root 应指向本仓库：{root}");

    // 内存库上完整走一遍 register → list → remove（生产代码填充一切前置状态）。
    let mut conn = rusqlite::Connection::open_in_memory().expect("内存库");
    harness_hub_lib::db::migrations::apply_pending(&mut conn).expect("迁移");
    let service = ProjectService::new(&conn, probe);

    // 子目录也应解析到同一个 root，并复用同一条登记（幂等）。
    let subdir = repo_dir.join("src-tauri");
    let registered_from_subdir = service
        .register(&RegisterRequest {
            path: subdir.to_string_lossy().into_owned(),
            name: None,
        })
        .expect("登记子目录");
    eprintln!(
        "registered: name={} root={}",
        registered_from_subdir.name, registered_from_subdir.repo_root
    );
    assert_eq!(registered_from_subdir.name, "HarnessHub");
    assert_eq!(registered_from_subdir.repo_root, root);

    let registered = service
        .register(&RegisterRequest {
            path: root.clone(),
            name: None,
        })
        .expect("登记 root");
    assert_eq!(
        registered.id, registered_from_subdir.id,
        "同一 repo root 必须幂等复用"
    );

    let listed = service.list_recent(10).expect("列表");
    assert_eq!(listed.len(), 1, "两条登记请求只应有一条项目");

    assert!(service.remove(&registered.id).expect("删除"));
    assert_eq!(service.list_recent(10).expect("删除后列表").len(), 0);
}
