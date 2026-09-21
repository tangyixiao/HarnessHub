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
