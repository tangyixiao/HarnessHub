//! HHAR —— Harness Hub Activity Record。
//!
//! 职责：把长期用户活动数据导出为可移植格式，保证 SQLite 只是实现细节，
//! 而不是用户数据的唯一容器（ADR-0019）。schema 草案见 `schemas/hhar/`。
//!
//! 当前为占位模块（Phase 9.5）：v0.1 不发布公开 schema，
//! 但数据模型必须保证「未来可导出」，因此不做分析侧的锁定设计。
