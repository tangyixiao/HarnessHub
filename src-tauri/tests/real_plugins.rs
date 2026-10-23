//! 真机插件宿主 E2E（Phase 9.3）。
//!
//! 证明的是**真进程、真目录、真 SQLite** 上的整条链路，而不是各模块的局部正确性：
//!
//! 1. 把两个示例插件复制到临时 plugins root → `service::refresh()` → 两个都 `ready`，
//!    capabilities 与 skills_cache 与 manifest 一致；
//! 2. `usage/import` 全链路：插件回 `{"session":[…]}` → 组装成 ccusage 报告 →
//!    现有 `parse_report` / `normalize_session` → `UsageImporter` → `usage_events`
//!    行数正确、连导两次行数不变（幂等）、`source = "plugin:usage-connect"`；
//! 3. `skill/get` 拿到的 `SKILL.md` 与磁盘上的原文逐字节相同（宿主不解析、不重写）；
//! 4. degraded 插件调用 `import_usage` 返回结构化错误而不是 panic，`refresh` 后复活；
//! 5. `host/log` 在 `permission_decisions` 里留下 `observed` 行，超长参数被截断到 ≤512。
//!
//! 本机没有 `python` 时**明确跳过**（与 `common::precondition` 的惯例一致）：
//! 示例插件是 Python 脚本，没有解释器就无从验证，伪装通过比跳过更糟。

use std::path::{Path, PathBuf};

use harness_hub_lib::db::Database;
use harness_hub_lib::plugins::{discovery, host_api, registry, service};

mod common;

/// 仓库根（`CARGO_MANIFEST_DIR` 是 `src-tauri/`，往上一级才是仓库根）。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 有父目录")
        .to_path_buf()
}

fn examples_root() -> PathBuf {
    repo_root().join("plugins").join("examples")
}

/// PATH 里有没有 `python`。缺失时整份测试跳过。
fn python_available() -> bool {
    std::process::Command::new("python")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// 把两个示例插件复制进一个干净的临时 root。
///
/// **必须先复制再用**：`refresh` 会把插件的绝对路径写进 `plugins` 表，若直接指向
/// 仓库内的示例目录，测试就会断言到工作区的真实路径上——迁移仓库或并行跑测试时
/// 会变成假失败。临时 root 也让「只复制一部分插件」的场景变得可行。
fn stage_examples(root: &Path) {
    std::fs::create_dir_all(root).expect("临时 root");
    for name in ["usage-connect", "hello-skill"] {
        let from = examples_root().join(name);
        let to = root.join(name);
        copy_dir(&from, &to);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("建目标目录");
    for entry in std::fs::read_dir(from).expect("读源目录").flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("复制文件");
        }
    }
}

/// 每次测试一个独立的临时 root + 内存库，避免互相污染。
fn fresh_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("hh-plugins-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("临时目录");
    root
}

#[test]
fn refresh_registers_both_example_plugins_as_ready() {
    if !python_available() {
        eprintln!("跳过：PATH 里没有 python");
        return;
    }

    let root = fresh_root("refresh");
    stage_examples(&root);
    let db = Database::open_in_memory().expect("内存库");

    let report = service::refresh(db.connection(), &root);

    assert!(
        report.errors.is_empty(),
        "refresh 不该有错误：{:?}",
        report.errors
    );
    let mut registered = report.registered.clone();
    registered.sort();
    assert_eq!(registered, vec!["hello-skill", "usage-connect"]);

    // 两个插件都必须真能起进程：握手通过才会是 ready。
    let records = registry::list_plugins(db.connection()).expect("列插件");
    assert_eq!(records.len(), 2);
    for record in &records {
        assert_eq!(
            record.status,
            registry::STATUS_READY,
            "{} 不是 ready",
            record.id
        );
    }

    let usage = registry::get_plugin(db.connection(), "usage-connect")
        .expect("查询")
        .expect("usage-connect 已登记");
    assert!(usage.capabilities_json.contains("\"usage\":true"));
    assert!(usage.capabilities_json.contains("\"skills\":false"));

    let skill = registry::get_plugin(db.connection(), "hello-skill")
        .expect("查询")
        .expect("hello-skill 已登记");
    assert!(skill.capabilities_json.contains("\"skills\":true"));
    let cache = skill.skills_cache_json.expect("skill 缓存已写入");
    assert!(cache.contains("\"skills\""), "缓存形状是对象：{cache}");
    assert!(cache.contains("\"demo\""), "缓存里有 demo skill：{cache}");
}

#[test]
fn usage_import_is_idempotent_and_tagged_with_the_plugin_source() {
    if !python_available() {
        eprintln!("跳过：PATH 里没有 python");
        return;
    }

    let root = fresh_root("usage");
    stage_examples(&root);
    let db = Database::open_in_memory().expect("内存库");
    service::refresh(db.connection(), &root);

    let first = service::import_usage(
        db.connection(),
        "usage-connect",
        &root.join("usage-connect"),
    )
    .expect("首次导入");
    assert!(first > 0, "示例插件应当给出至少一条会话");

    let count_events = |connection: &rusqlite::Connection| -> i64 {
        connection
            .query_row("SELECT COUNT(*) FROM usage_events", [], |row| row.get(0))
            .expect("数事件")
    };
    let after_first = count_events(db.connection());
    assert!(after_first > 0);

    // 幂等：同一份数据再导一次，行数必须一字不增（stable_source_key 的职责）。
    let second = service::import_usage(
        db.connection(),
        "usage-connect",
        &root.join("usage-connect"),
    )
    .expect("二次导入");
    assert_eq!(second, first, "二次导入解析出的会话数应当相同");
    assert_eq!(
        count_events(db.connection()),
        after_first,
        "二次导入不该新增事件行"
    );

    // source 必须带 plugin: 前缀，好让审计口径区分「插件导入」与「宿主自己跑 ccusage」。
    let source: String = db
        .connection()
        .query_row("SELECT source FROM usage_events LIMIT 1", [], |row| {
            row.get(0)
        })
        .expect("读 source");
    assert_eq!(source, "plugin:usage-connect");
}

#[test]
fn usage_import_rejects_an_unregistered_directory_before_spawn() {
    if !python_available() {
        eprintln!("跳过：PATH 里没有 python");
        return;
    }
    let root = fresh_root("wrong-directory");
    stage_examples(&root);
    let db = Database::open_in_memory().expect("空库");
    service::refresh(db.connection(), &root);
    let other = root.join("other-copy");
    copy_dir(&root.join("usage-connect"), &other);
    let marker = other.join("started");
    let script = other.join("adapter/usage.py");
    let original = std::fs::read_to_string(&script).expect("读取 adapter");
    std::fs::write(
        &script,
        original.replace("PLUGIN_ID =", "Path('started').touch()\nPLUGIN_ID ="),
    )
    .expect("添加启动标记");

    let result = service::import_usage(db.connection(), "usage-connect", &other);
    assert!(
        matches!(result, Err(harness_hub_lib::error::Error::InvalidInput(_))),
        "{result:?}"
    );
    assert!(!marker.exists(), "不属于登记目录的 adapter 不应被启动");
    assert_usage_is_empty_and_plugin_is_ready(&db);
}

#[test]
fn usage_import_rejects_a_changed_manifest_identity_before_spawn() {
    if !python_available() {
        eprintln!("跳过：PATH 里没有 python");
        return;
    }
    let root = fresh_root("wrong-id");
    stage_examples(&root);
    let db = Database::open_in_memory().expect("空库");
    service::refresh(db.connection(), &root);
    let dir = root.join("usage-connect");
    let manifest_path = dir.join("manifest.toml");
    let original = std::fs::read_to_string(&manifest_path).expect("manifest");
    std::fs::write(
        &manifest_path,
        original.replace("usage-connect", "other-plugin"),
    )
    .expect("更改 manifest 身份");
    let script = dir.join("adapter/usage.py");
    let original = std::fs::read_to_string(&script).expect("adapter");
    std::fs::write(
        &script,
        original.replace(
            "PLUGIN_ID = \"usage-connect\"",
            "Path('started').touch()\nPLUGIN_ID = \"other-plugin\"",
        ),
    )
    .expect("更改 adapter 身份");

    let result = service::import_usage(db.connection(), "usage-connect", &dir);
    assert!(
        matches!(result, Err(harness_hub_lib::error::Error::InvalidInput(_))),
        "{result:?}"
    );
    assert!(!dir.join("started").exists());
    assert_usage_is_empty_and_plugin_is_ready(&db);
}

fn assert_usage_is_empty_and_plugin_is_ready(db: &Database) {
    for table in ["usage_events", "usage_imports"] {
        let count: i64 = db
            .connection()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("计数");
        assert_eq!(count, 0, "错配不得写入 {table}");
    }
    assert_eq!(
        registry::get_plugin(db.connection(), "usage-connect")
            .expect("查询")
            .expect("存在")
            .status,
        registry::STATUS_READY
    );
}

#[test]
fn usage_import_failure_marks_degraded_until_refresh_recovers() {
    if !python_available() {
        eprintln!("跳过：PATH 里没有 python");
        return;
    }
    let root = fresh_root("import-failure");
    stage_examples(&root);
    let db = Database::open_in_memory().expect("空库");
    service::refresh(db.connection(), &root);
    let dir = root.join("usage-connect");
    let script = dir.join("adapter/usage.py");
    let original = std::fs::read_to_string(&script).expect("adapter");
    std::fs::write(&script, "import sys\nsys.exit(1)\n").expect("破坏 adapter");

    service::import_usage(db.connection(), "usage-connect", &dir).expect_err("启动失败");
    assert_eq!(
        registry::get_plugin(db.connection(), "usage-connect")
            .expect("查询")
            .expect("存在")
            .status,
        registry::STATUS_DEGRADED
    );
    let result = service::import_usage(db.connection(), "usage-connect", &dir);
    assert!(matches!(
        result,
        Err(harness_hub_lib::error::Error::PluginDegraded(_))
    ));

    std::fs::write(&script, original).expect("还原 adapter");
    service::refresh(db.connection(), &root);
    assert!(
        service::import_usage(db.connection(), "usage-connect", &dir.join("."))
            .expect("refresh 后恢复，路径别名可用")
            > 0
    );
}

#[test]
fn usage_import_call_crash_marks_degraded_without_implicit_respawn() {
    if !python_available() {
        eprintln!("跳过：PATH 里没有 python");
        return;
    }

    let root = fresh_root("usage-call-crash");
    stage_examples(&root);
    let db = Database::open_in_memory().expect("从空库启动");
    let refresh = service::refresh(db.connection(), &root);
    assert!(
        refresh.registered.contains(&"usage-connect".to_string()),
        "生产 refresh 应登记 usage-connect：{:?}",
        refresh
    );
    let dir = root.join("usage-connect");
    let script = dir.join("adapter/usage.py");
    let original = std::fs::read_to_string(&script).expect("读取 adapter");
    let target = "    rows = load_fixture()\n";
    assert_eq!(
        original.matches(target).count(),
        1,
        "应恰好匹配 handle_usage_import 的函数体边界"
    );
    let modified = original.replacen(
        target,
        "    Path('called').touch()\n    sys.exit(1)\n    rows = load_fixture()\n",
        1,
    );
    assert!(
        modified.contains("def handle_usage_import(params: dict) -> dict:\n    \"\"\"usage/import")
            && modified.contains("    Path('called').touch()\n    sys.exit(1)\n"),
        "故障注入必须命中 usage/import handler，不能影响 initialize"
    );
    std::fs::write(&script, modified).expect("注入调用期崩溃");

    let first = service::import_usage(db.connection(), "usage-connect", &dir);
    assert!(first.is_err(), "usage/import 进程崩溃应使调用失败");
    let called = dir.join("called");
    assert!(called.exists(), "插件必须实际收到 usage/import 请求");
    assert_eq!(
        registry::get_plugin(db.connection(), "usage-connect")
            .expect("查询")
            .expect("refresh 已登记")
            .status,
        registry::STATUS_DEGRADED
    );
    for table in ["usage_events", "usage_imports"] {
        let count: i64 = db
            .connection()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("计数");
        assert_eq!(count, 0, "调用崩溃不得留下 {table} 半成品");
    }

    std::fs::remove_file(&called).expect("清除首次调用标记");
    let second = service::import_usage(db.connection(), "usage-connect", &dir);
    assert!(
        matches!(
            second,
            Err(harness_hub_lib::error::Error::PluginDegraded(_))
        ),
        "degraded 插件应在 spawn 前返回 PluginDegraded：{second:?}"
    );
    assert!(
        !called.exists(),
        "degraded 状态下的二次调用不得隐式重启插件"
    );

    std::fs::write(&script, original).expect("还原 adapter");
    let recovered = service::refresh(db.connection(), &root);
    assert!(
        recovered.registered.contains(&"usage-connect".to_string()),
        "refresh 应重新注册插件：{:?}",
        recovered
    );
    assert!(
        service::import_usage(db.connection(), "usage-connect", &dir).expect("refresh 后正向导入")
            > 0,
        "恢复后的示例插件应导入至少一个会话"
    );
}

#[test]
fn usage_import_invalid_payload_marks_plugin_degraded() {
    if !python_available() {
        eprintln!("跳过：PATH 里没有 python");
        return;
    }
    let root = fresh_root("invalid-usage");
    stage_examples(&root);
    let db = Database::open_in_memory().expect("空库");
    service::refresh(db.connection(), &root);
    let dir = root.join("usage-connect");
    std::fs::write(dir.join("fixtures/sample.json"), r#"{"session":[{}]}"#).expect("坏数据");

    service::import_usage(db.connection(), "usage-connect", &dir).expect_err("拒绝坏数据");
    assert_eq!(
        registry::get_plugin(db.connection(), "usage-connect")
            .expect("查询")
            .expect("存在")
            .status,
        registry::STATUS_DEGRADED
    );
    let count: i64 = db
        .connection()
        .query_row("SELECT COUNT(*) FROM usage_events", [], |row| row.get(0))
        .expect("事件数");
    assert_eq!(count, 0);
}

#[test]
fn usage_import_empty_payload_succeeds_without_degrading() {
    if !python_available() {
        eprintln!("跳过：PATH 里没有 python");
        return;
    }
    let root = fresh_root("empty-usage");
    stage_examples(&root);
    let db = Database::open_in_memory().expect("空库");
    service::refresh(db.connection(), &root);
    let dir = root.join("usage-connect");
    std::fs::write(dir.join("fixtures/sample.json"), r#"{"session":[]}"#).expect("空数据");

    assert_eq!(
        service::import_usage(db.connection(), "usage-connect", &dir).expect("空导入"),
        0
    );
    assert_usage_is_empty_and_plugin_is_ready(&db);
}

#[test]
fn usage_import_zero_token_session_keeps_the_import_audit() {
    if !python_available() {
        eprintln!("跳过：PATH 里没有 python");
        return;
    }
    let root = fresh_root("zero-token-usage");
    stage_examples(&root);
    let db = Database::open_in_memory().expect("空库");
    service::refresh(db.connection(), &root);
    let dir = root.join("usage-connect");
    std::fs::write(
        dir.join("fixtures/sample.json"),
        r#"{"session":[{"agent":"codex","period":"zero-token-session","totalTokens":0,"modelBreakdowns":[]}]}"#,
    ).expect("零 token 会话");

    assert_eq!(
        service::import_usage(db.connection(), "usage-connect", &dir).expect("导入"),
        0
    );
    let count: i64 = db.connection().query_row(
        "SELECT COUNT(*) FROM usage_imports WHERE source = 'plugin:usage-connect' AND status = 'succeeded'",
        [], |row| row.get(0),
    ).expect("导入审计数");
    assert_eq!(count, 1, "非空会话报告即使没有事件也需要保留审计");
}

#[test]
fn usage_import_database_failure_does_not_degrade_a_healthy_plugin() {
    if !python_available() {
        eprintln!("跳过：PATH 里没有 python");
        return;
    }
    let root = fresh_root("db-failure");
    stage_examples(&root);
    let db = Database::open_in_memory().expect("空库");
    service::refresh(db.connection(), &root);
    db.connection().execute_batch("CREATE TRIGGER fail_usage BEFORE INSERT ON usage_events BEGIN SELECT RAISE(ABORT, 'test database failure'); END;")
        .expect("模拟 SQLite 写失败");

    let result = service::import_usage(
        db.connection(),
        "usage-connect",
        &root.join("usage-connect"),
    );
    assert!(
        matches!(result, Err(harness_hub_lib::error::Error::Database(_))),
        "{result:?}"
    );
    assert_usage_is_empty_and_plugin_is_ready(&db);
}

#[test]
fn skill_get_returns_the_skill_markdown_verbatim() {
    if !python_available() {
        eprintln!("跳过：PATH 里没有 python");
        return;
    }

    let root = fresh_root("skill");
    stage_examples(&root);
    let db = Database::open_in_memory().expect("内存库");
    service::refresh(db.connection(), &root);

    let cache = registry::get_plugin(db.connection(), "hello-skill")
        .expect("查询")
        .expect("已登记")
        .skills_cache_json
        .expect("缓存已写入");
    let parsed: serde_json::Value = serde_json::from_str(&cache).expect("缓存是合法 JSON");
    let body = parsed["skills"][0]["body"].as_str().expect("缓存里有 body");

    // 宿主只透传：缓存里的正文必须与磁盘上的 SKILL.md 逐字节相同。
    let on_disk = std::fs::read_to_string(
        root.join("hello-skill")
            .join("skills")
            .join("demo")
            .join("SKILL.md"),
    )
    .expect("读磁盘 SKILL.md");
    assert_eq!(body, on_disk);
    assert!(body.len() > 100, "正文不该是空壳");
}

#[test]
fn degraded_plugin_returns_a_structured_error_and_refresh_revives_it() {
    if !python_available() {
        eprintln!("跳过：PATH 里没有 python");
        return;
    }

    let root = fresh_root("degraded");
    stage_examples(&root);
    let db = Database::open_in_memory().expect("内存库");
    service::refresh(db.connection(), &root);

    // 制造降级：把 adapter 脚本内容换成「立刻退出」，但保留 manifest 完好。
    // 这样复现的是「插件本体坏了」，而不是「目录不存在」——后者的错误路径完全不同。
    let adapter = root.join("usage-connect").join("adapter").join("usage.py");
    std::fs::write(&adapter, "import sys\nsys.exit(1)\n").expect("改写 adapter");

    let report = service::refresh(db.connection(), &root);
    assert!(
        report.registered.contains(&"usage-connect".to_string()),
        "坏插件仍要被登记（用户得看见它）：{:?}",
        report
    );
    let record = registry::get_plugin(db.connection(), "usage-connect")
        .expect("查询")
        .expect("已登记");
    assert_eq!(record.status, registry::STATUS_DEGRADED);

    // 降级插件被调用时必须给结构化错误，而不是 panic 或静默返回 0（R14）。
    let error = service::import_usage(
        db.connection(),
        "usage-connect",
        &root.join("usage-connect"),
    )
    .expect_err("降级插件不该导入成功");
    assert!(
        error.to_string().contains("degraded"),
        "错误里要点明 degraded：{error}"
    );

    // 把脚本还原 → refresh → 复活。
    copy_dir(
        &examples_root().join("usage-connect"),
        &root.join("usage-connect"),
    );
    let report = service::refresh(db.connection(), &root);
    assert!(report.errors.is_empty(), "复活失败：{:?}", report.errors);
    let record = registry::get_plugin(db.connection(), "usage-connect")
        .expect("查询")
        .expect("已登记");
    assert_eq!(record.status, registry::STATUS_READY);
}

#[test]
fn host_log_observes_calls_and_truncates_oversized_arguments() {
    let db = Database::open_in_memory().expect("内存库");

    // 先登记一行插件，好让 permission_decisions 有归属。
    let record = registry::PluginRecord {
        id: "obs".to_string(),
        name: "Observer".to_string(),
        version: "0.1.0".to_string(),
        api_version: "1".to_string(),
        path: "/tmp/obs".to_string(),
        capabilities_json: "{}".to_string(),
        skills_cache_json: None,
        status: registry::STATUS_READY.to_string(),
        registered_at: "2026-10-17T00:00:00Z".to_string(),
    };
    registry::upsert_plugin(db.connection(), &record).expect("登记");

    let long = "x".repeat(2000);
    let params = serde_json::json!({ "level": "info", "message": long.clone() });
    let outcome = host_api::dispatch(db.connection(), "obs", host_api::HOST_LOG, &params);
    assert!(
        outcome.error.is_none(),
        "host/log 应当成功：{:?}",
        outcome.error
    );

    let (api, decision, args_json, reason): (String, String, String, Option<String>) = db
        .connection()
        .query_row(
            "SELECT api, decision, args_json, reason FROM permission_decisions LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("读决策行");
    assert_eq!(api, host_api::HOST_LOG);
    assert_eq!(decision, host_api::DECISION_OBSERVED);
    // 迁移 0008 承诺「宿主保证 <= 512 字符」，这里按同一口径断言（省略号计入上限）。
    assert!(
        args_json.chars().count() <= host_api::ARGS_JSON_LIMIT,
        "参数必须被截断到 {} 字符以内，实际 {}",
        host_api::ARGS_JSON_LIMIT,
        args_json.chars().count()
    );
    assert!(args_json.ends_with('…'), "截断要留痕迹：{args_json}");
    // reason 只在失败时承载诊断信息；`host/log` 成功时没有「为什么」可写，
    // 断言 None 才是对的契约（写成 is_some 会逼实现编一句假理由）。
    assert_eq!(reason, None, "成功的 host/log 不该编造 reason");
}

#[test]
fn scan_lists_plugin_directories_and_ignores_everything_else() {
    // 这条不需要 python：候选筛选在解析 manifest 之前就完成了，走不到 spawn。
    let root = fresh_root("scan");

    // 没有 manifest.toml 的目录不是插件候选（连错误都不该报）。
    std::fs::create_dir_all(root.join("not-a-plugin")).expect("建目录");
    std::fs::write(root.join("not-a-plugin").join("README.md"), "hello").expect("写文件");

    // 有 manifest.toml 但内容坏掉 - 必须出现，且带可读原因。
    std::fs::create_dir_all(root.join("broken")).expect("建目录");
    std::fs::write(root.join("broken").join("manifest.toml"), "id = 42\n").expect("写 manifest");

    // 根目录下的裸文件也不是候选。
    std::fs::write(root.join("stray.toml"), "id = 'x'\n").expect("写文件");

    let discovered = discovery::scan(&root);
    assert_eq!(discovered.len(), 1, "只有含 manifest.toml 的目录才是候选");
    assert!(discovered[0].dir.ends_with("broken"));

    let broken = &discovered[0];
    assert!(broken.manifest.is_none(), "坏 manifest 不该被解析成插件");
    assert!(
        broken.error.as_deref().is_some_and(|e| !e.is_empty()),
        "失败必须带可诊断的原因，否则用户只能猜"
    );
}
