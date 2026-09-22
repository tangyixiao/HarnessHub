//! 测试夹具：内存库 + 最小种子数据。
//!
//! **测试纪律**（review 结论，已写入 AGENTS.md）：
//! 涉及 FK / migration / registry bootstrap 的测试，必须至少有一组从**真正空库**
//! 开始、只走生产路径填充；不允许夹具偷偷替生产代码补前置状态。
//!
//! 真实宿主上已经因此漏掉过一个 FK 缺陷：`seeded_db()` 替调用方插好了 harness 行，
//! 于是单元测试全绿，而冷启动的 `create_session` 直接
//! `FOREIGN KEY constraint failed`。所以这里有两个夹具，用途必须分清：
//!
//! - [`empty_db`] / [`cold_start_db`]：冷启动路径，**生产代码负责填数据**；
//! - [`seeded_db`]：只给存储层单测用的便捷夹具，禁止用它验证集成路径。

use rusqlite::params;

use crate::db::Database;
use crate::harness::adapter::HarnessCapabilities;
use crate::harness::registry::HarnessSummary;

/// 测试用 Harness id，与 `harnesses.id` 对应。
pub const HARNESS_ID: &str = "codex";

/// 测试用运行目标 id。**引用生产常量**，避免两处硬编码漂移。
pub const RUNTIME_TARGET_ID: &str = crate::runtime::local::LOCAL_TARGET_ID;

/// 固定时间戳：测试里不要依赖真实当前时间。
pub const NOW: &str = "2026-01-01T00:00:00Z";

/// 只有 schema、没有任何业务数据的库。
pub fn empty_db() -> Database {
    Database::open_in_memory().expect("打开内存库")
}

/// 「检测到本机装了 Codex」的最小快照，供 reconcile 走生产路径。
pub fn detected_codex_summary() -> HarnessSummary {
    HarnessSummary {
        id: HARNESS_ID.to_string(),
        display_name: "Codex".to_string(),
        installation_id: None,
        installed: true,
        binary_path: Some("D:/npm-global/codex.cmd".to_string()),
        version: Some("0.152.1".to_string()),
        capabilities: HarnessCapabilities::default(),
        data_paths: vec!["C:/Users/dev/.codex".to_string()],
    }
}

/// 冷启动：**空库 + 只调用生产引导代码**。
pub fn cold_start_db() -> Database {
    let db = empty_db();
    crate::runtime::local::ensure_local_target(db.connection()).expect("引导 runtime target");
    crate::harness::inventory::reconcile_harnesses(
        db.connection(),
        &[detected_codex_summary()],
        RUNTIME_TARGET_ID,
        NOW,
    )
    .expect("同步 Harness 清单");
    db
}

/// 便捷夹具：手工塞好 harness 定义 + 安装 + runtime target。
///
/// **只用于存储层单测**（例如验证 SQL 约束）。集成/冷启动测试请用 [`cold_start_db`]。
pub fn seeded_db() -> Database {
    let db = empty_db();
    seed_baseline(&db);
    db
}

/// 手工写入基线数据。重复调用会因主键冲突失败，调用方自行保证只调用一次。
pub fn seed_baseline(db: &Database) {
    let conn = db.connection();

    conn.execute(
        "INSERT INTO harnesses (id, display_name, created_at, updated_at)
         VALUES (?1, 'Codex', ?2, ?2)",
        params![HARNESS_ID, NOW],
    )
    .expect("插入 harness 定义");

    conn.execute(
        "INSERT INTO runtime_targets (id, kind, display_name, created_at)
         VALUES (?1, 'local', '本机', ?2)",
        params![RUNTIME_TARGET_ID, NOW],
    )
    .expect("插入 runtime target");

    conn.execute(
        "INSERT INTO harness_installations
            (id, harness_id, runtime_target_id, binary_path, version, capabilities_json,
             data_paths_json, availability, first_detected_at, last_seen_at)
         VALUES (?1, ?2, ?3, 'D:/npm-global/codex.cmd', '0.152.1', '{}', '[]',
                 'available', ?4, ?4)",
        params![
            crate::harness::inventory::installation_id(HARNESS_ID, RUNTIME_TARGET_ID),
            HARNESS_ID,
            RUNTIME_TARGET_ID,
            NOW
        ],
    )
    .expect("插入 harness 安装");
}
