//! 测试夹具：内存库 + 满足外键约束的最小种子数据。
//!
//! 只在 `cfg(test)` 下编译，不进入发布产物。

use rusqlite::params;

use crate::db::Database;

/// 测试用 Harness id，与 `harnesses.id` 对应。
pub const HARNESS_ID: &str = "codex";

/// 测试用运行目标 id。**引用生产常量**，避免两处硬编码漂移。
pub const RUNTIME_TARGET_ID: &str = crate::runtime::local::LOCAL_TARGET_ID;

/// 固定时间戳：测试里不要依赖真实当前时间。
pub const NOW: &str = "2026-01-01T00:00:00Z";

/// 只有 schema、没有业务数据的库。
pub fn empty_db() -> Database {
    Database::open_in_memory().expect("打开内存库")
}

/// 带 harness 与 runtime target 的库，满足 `sessions` 的外键约束。
pub fn seeded_db() -> Database {
    let db = empty_db();
    seed_baseline(&db);
    db
}

/// 写入最小基线数据。重复调用会因主键冲突失败，调用方自行保证只调用一次。
pub fn seed_baseline(db: &Database) {
    db.connection()
        .execute(
            "INSERT INTO harnesses (id, display_name, created_at, updated_at)
             VALUES (?1, 'Codex', ?2, ?2)",
            params![HARNESS_ID, NOW],
        )
        .expect("插入 harness");

    db.connection()
        .execute(
            "INSERT INTO runtime_targets (id, kind, display_name, created_at)
             VALUES (?1, 'local', '本机', ?2)",
            params![RUNTIME_TARGET_ID, NOW],
        )
        .expect("插入 runtime target");
}
