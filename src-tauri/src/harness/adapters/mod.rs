//! 具体 Harness 适配器。
//!
//! Phase 1 Task「Codex 检测」起，每个 Harness 一个文件：
//! `codex.rs`、`claude_code.rs`、`gemini_cli.rs`、`opencode.rs`。
//!
//! 新增一个适配器的准入条件（ADR-0022）：
//!   1. `fixtures/<harness>/` 下有脱敏的真实日志夹具；
//!   2. 有覆盖 detect / capabilities / usage 解析的 contract test；
//!   3. 更新能力矩阵。
//!
//! 「能检测到」不算支持，「可稳定回归」才算。
//!
//! 当前为占位模块：这里刻意不提供未经验证的实现，避免制造「看起来能用」的假象。
