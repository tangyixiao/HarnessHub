//! Eval / 回归层。
//!
//! 职责：运行 Adapter 的 golden fixture 回归、contract test 与 promptfoo 集成，
//! 作为发布门禁（规格 4.30）。
//!
//! 准入原则（ADR-0022）：新 Harness Adapter 必须有 fixture + contract test，
//! 「能检测到」不算支持，「可稳定回归」才算。
//!
//! 当前为占位模块（Phase 9.4）。Rust 侧的 fixture 回归测试放在 `src-tauri/tests/`，
//! 夹具放 `fixtures/<harness>/`。
