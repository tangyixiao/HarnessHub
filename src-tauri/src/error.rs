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
