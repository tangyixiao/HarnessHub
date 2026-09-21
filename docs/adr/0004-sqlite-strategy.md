# ADR-0004 — SQLite 作为统一索引库，schema 通过迁移演进

- **Status**: Accepted（2026-04）
- **Context**: 需要把 Harness、Project、Session、Turn、Usage、Git 活动、导入任务等关联起来查询，
  同时要支持"崩溃不损坏数据库""导入可重复执行不重复计数""外部数据只读"这些硬要求。
- **Decision**:
  1. 使用 **SQLite** 作为唯一主索引库，由 **Rust Control Plane 独占持有**（Python sidecar 不得直连）。
  2. 启用 WAL 模式与 `foreign_keys = ON`，并提供一致性自检（崩溃后不损坏）。
  3. schema 变更**只能新增** `src-tauri/src/db/migrations/NNNN_*.sql`，已提交的 migration 不得修改；
     `schema_migrations(version, name, applied_at)` 记录已应用版本。
  4. 去重靠**数据库唯一约束**而不是应用层判断：`usage_events.dedupe_key` 唯一，保证 import 幂等。
  5. Session 必须同时保存 `hub_session_id` 与 `source_session_id`，且
     `(harness_id, source_session_id)` 唯一（source_session_id 非空时）。
  6. 长期用户活动数据必须可导出为 HHAR；SQLite 是实现细节，不是用户数据的唯一容器。
- **Alternatives**:
  - `sqlx` + 编译期校验：类型安全更好，但需要数据库连接或离线缓存，构建复杂度更高。
  - 纯文件（JSON/NDJSON）+ 内存索引：简单，但跨维度聚合（Project × Harness × Model × Day）代价高。
  - DuckDB / 嵌入式列存：分析查询更强，但事务性与生态成熟度不如 SQLite。
- **Consequences**:
  - `rusqlite`（`bundled`）随包编译 SQLite，Windows 上需要 C 编译工具链（已在本机验证）。
  - 所有聚合查询走 SQL；重度分析场景推迟到 v0.2 评估。
  - 迁移执行器是自写的极小实现，必须有测试覆盖"空库迁移"与"重复执行幂等"。
- **Evidence**: `src-tauri/src/db/migrations.rs` 与 `0001_init.sql` 的迁移测试；
  `usage_events.dedupe_key UNIQUE`。
- **Revisit Conditions**: 若单库查询在真实数据量下出现不可接受延迟，或需要跨设备聚合。
