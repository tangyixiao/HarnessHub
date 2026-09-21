//! `UsageAdapter`：Usage 数据源的发现、导入与监听。
//!
//! 与 `HarnessAdapter` 分离（ADR-0002）。v0.1 的第一选择是 ccusage 的 JSON 输出，
//! 只有满足「ccusage 不支持 / 丢失必需数据 / 上游不接受改动 / 自研有明确长期价值」
//! 四条之一，才自写 source-specific parser。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::harness::HarnessId;

/// 一个可读取的 Usage 数据源。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSource {
    pub path: PathBuf,
    pub kind: DataSourceKind,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataSourceKind {
    /// ccusage 的 JSON 输出。
    CcusageJson,
    /// Harness 自己的原生日志。
    NativeLog,
    /// 尚未识别的来源。
    Unknown,
}

/// 导入时间范围（RFC3339 UTC，闭区间）。两端为 `None` 表示不限制。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportRange {
    pub start: Option<String>,
    pub end: Option<String>,
}

/// 一次导入的结果。
///
/// `records_imported + records_skipped == records_seen` 必须成立；
/// `skipped` 主要来自 dedupe_key 冲突，也就是重复执行同一份数据。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub records_seen: u64,
    pub records_imported: u64,
    pub records_skipped: u64,
    /// 该来源是否只是估算值（必须在 UI 上标注）。
    pub estimated: bool,
}

pub trait UsageAdapter: Send + Sync {
    fn source(&self) -> HarnessId;

    /// 发现本机可用的数据源。只读，不得修改外部文件。
    fn discover(&self) -> Result<Vec<DataSource>>;

    /// 导入指定范围的数据。必须幂等：重复执行不重复计数。
    fn import(&self, range: ImportRange) -> Result<ImportResult>;

    /// 监听数据源变化（文件监听）。
    fn watch(&self) -> Result<()>;
}
