//! PTY 传输层。
//!
//! 分层（见 docs/adr/0009-pty-is-raw-transport.md）：
//!
//! ```text
//! pty::backend               trait：spawn / read / write / resize / kill / wait / pid
//! pty::portable_pty_backend  真实实现（唯一依赖 portable-pty 的文件）
//! pty::manager               读线程 + 顺序转发 + EOF 后取退出码
//! ```
//!
//! 本模块**不解析任何终端转义序列**，也不理解 Harness 语义。
//! DSR / CSI / OSC 的应答属于终端模拟器（GUI 用 xterm.js；无头测试用 test-only responder）。
//!
//! 复用而非自研（规格 1.1）：PTY 直接用成熟 crate `portable-pty`。

pub mod backend;
pub mod containment;
pub mod manager;
pub mod portable_pty_backend;
pub mod session;

#[cfg(test)]
mod input_tests;

pub use backend::{PtyBackend, PtyProcessHandle, PtySpawnRequest};
pub use manager::{ExitSink, OutputSink, PtyManager};
pub use portable_pty_backend::PortablePtyBackend;
