//! 统一错误类型。
//!
//! 领域层只产出 [`Error`]；Tauri 命令层直接把它序列化成字符串返回给前端，
//! 前端 `src/lib/ipc.ts` 再包装成 `IpcResult`。

use serde::{Serialize, Serializer};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("数据库错误：{0}")]
    Database(#[from] rusqlite::Error),

    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),

    #[error("数据迁移失败：{0}")]
    Migration(String),

    /// 数据源在本机不可用（没装、找不到 runner）。这是**正常状态**，不是崩溃。
    #[error("Usage 数据源不可用：{0}")]
    UsageUnavailable(String),

    /// 形状不认识：必须点名说清期望什么，不允许静默少算数字。
    #[error("Usage 数据形状不受支持：{0}")]
    UsageUnsupportedShape(String),

    #[error("Usage 数据损坏：{0}")]
    UsageMalformed(String),

    #[error("外部命令失败（退出码 {exit_code}）：{stderr}")]
    UsageCommandFailed { exit_code: i32, stderr: String },

    #[error("内部状态锁已中毒，需要重启应用")]
    StateLockPoisoned,

    #[error("{0}")]
    InvalidInput(String),

    /// 输入未被接受：会话的待写输入（含正在写的那一批）已达上限。
    ///
    /// 语义（spec §4.9）：**输入仍然可用**，只是本批次因容量不足被原子拒绝。
    #[error(
        "输入被丢弃：会话 {session_id} 的待写输入 {pending_bytes} 字节已达上限 {capacity_bytes}（本次 {attempted_bytes} 字节整批拒绝）"
    )]
    InputBackpressure {
        session_id: String,
        pending_bytes: usize,
        capacity_bytes: usize,
        attempted_bytes: usize,
    },

    /// 该会话的输入侧已**明确关闭**（kill 成功 / forget / shutdown）。
    #[error("输入不可用：会话 {session_id} 的输入侧已关闭")]
    InputClosed { session_id: String },

    /// 异步 writer 遇到真实 `backend.write` 错误：输入路径**永久失败**。
    ///
    /// `detail` 保留底层错误用于诊断；**终态仍只由 reaper 决定**。
    #[error("输入不可用：会话 {session_id} 的写入线程失败：{detail}")]
    InputWorkerFailed { session_id: String, detail: String },
}

pub type Result<T> = std::result::Result<T, Error>;

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 背压错误必须自带四个诊断字段：用户贴 5 MiB 时要能一眼区分
    /// 「队列本来就满」与「这一批自己超上限」（spec §4.9）。
    #[test]
    fn backpressure_error_carries_full_diagnostics() {
        let error = Error::InputBackpressure {
            session_id: "hub-1".to_string(),
            pending_bytes: 65_536,
            capacity_bytes: 65_536,
            attempted_bytes: 12,
        };
        let message = error.to_string();

        assert!(message.contains("hub-1"), "{message}");
        assert!(message.contains("65536"), "{message}");
        assert!(message.contains("12"), "{message}");
        // 跨 IPC 仍是字符串（IpcResult 形状不变）。
        assert_eq!(
            serde_json::to_value(&error).expect("序列化"),
            serde_json::json!(message)
        );
    }

    #[test]
    fn closed_and_worker_failure_are_distinguishable_variants() {
        let closed = Error::InputClosed {
            session_id: "hub-1".to_string(),
        };
        let failed = Error::InputWorkerFailed {
            session_id: "hub-1".to_string(),
            detail: "写入 PTY 失败".to_string(),
        };

        assert!(closed.to_string().contains("已关闭"));
        assert!(failed.to_string().contains("写入 PTY 失败"));
        assert!(matches!(closed, Error::InputClosed { .. }));
        assert!(matches!(failed, Error::InputWorkerFailed { .. }));
    }
}
