//! PTY 传输层接口。
//!
//! **边界（docs/adr/0009-pty-is-raw-transport.md）**：这一层是 **raw transport**，
//! 只负责搬字节。它**不解析、不应答、不修改**任何终端转义序列
//! （DSR / CSI / OSC / 鼠标模式 / bracketed paste / 颜色查询…）。
//!
//! 那些属于**终端模拟器**：
//!
//! ```text
//! GUI 路径：     PTY bytes → Tauri Channel → xterm.js（解析 DSR 并回应）
//!                             ↑ input IPC                │
//!                             └──────────────────────────┘
//! 无头测试路径： PTY bytes → HeadlessTerminalResponder（test-only，必要时答 ESC[6n）
//! ```
//!
//! **禁止**在 `PortablePtyBackend` 里硬编码 `ESC[6n → ESC[1;1R`：
//! 那会把 PTY 层变成半吊子 terminal emulator，之后 OSC、鼠标模式、颜色查询
//! 都会一个个被塞进来。
//!
//! 本层负责：`spawn` / `read bytes` / `write bytes` / `resize` / `kill` / `wait` / `pid`。
//! 本层不负责：终端协议解析、Harness 语义、DSR 应答、命令字符串拼装、Session 落库策略。

use std::io::Read;

use crate::error::Result;
use crate::harness::launch::LaunchSpec;

/// 一次 spawn 请求。`spec` 由 Adapter 产出（结构化，不含 shell 字符串）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtySpawnRequest {
    pub session_id: String,
    pub spec: LaunchSpec,
    pub cols: u16,
    pub rows: u16,
}

/// spawn 成功后的进程句柄。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyProcessHandle {
    pub session_id: String,
    /// 诊断用：定位 kill 目标、核对 ghost running、host crash 后的一致性检查。
    pub pid: Option<u32>,
}

pub trait PtyBackend: Send + Sync {
    fn spawn(&self, request: PtySpawnRequest) -> Result<PtyProcessHandle>;

    /// 取读端（原始字节流）。调用方负责起线程读取并转发 —— 后端不持有回调。
    fn take_reader(&self, session_id: &str) -> Result<Box<dyn Read + Send>>;

    fn write(&self, session_id: &str, bytes: &[u8]) -> Result<()>;

    fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<()>;

    /// 终止该会话**拥有的整棵进程树**（不是「直接子进程」）。
    ///
    /// Windows 上由 Job Object 承担（ADR-0013）：containment 在 spawn 阶段建立，
    /// 因此**运行中的会话一定具备 containment**；缺失时必须返回明确错误，
    /// **不得**静默降级成 direct-child kill。
    ///
    /// 语义边界：这只是**控制动作**。会话终态仍只由 reaper/reconcile 依退出事实写成。
    fn terminate_tree(&self, session_id: &str) -> Result<()>;

    /// 非阻塞查询退出码；`Ok(None)` 表示仍在运行。**reaper 是唯一调用者。**
    fn try_wait(&self, session_id: &str) -> Result<Option<i32>>;

    fn is_running(&self, session_id: &str) -> Result<bool>;

    /// 丢弃会话句柄（终态写入之后调用）。`terminate_tree` **不得**隐式丢弃 ——
    /// 否则 reaper 读不到退出状态，终态永远写不下去。
    fn forget(&self, session_id: &str) -> Result<()>;
}
