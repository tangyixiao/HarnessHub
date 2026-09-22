//! `HarnessAdapter`：Harness 生命周期接口与能力矩阵。
//!
//! 关键约束（ADR-0002）：日志解析**不得**塞进这里。能启动与能可靠解析 Usage 是两件事。

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Harness 的规范标识，例如 `codex` / `claude-code` / `gemini-cli`。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HarnessId(String);

impl HarnessId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for HarnessId {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl std::fmt::Display for HarnessId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// 能力矩阵。刻意不用单个 boolean：每个 Harness 的能力独立演进，
/// UI 需要能逐项灰度（launch ✓ / usage ✗ / replay ?）。
///
/// 序列化用 camelCase：与其余 IPC 载荷一致（`binaryPath` / `dataPaths` / `displayName`），
/// 前端因此读 `toolCalls` / `liveState`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCapabilities {
    pub launch: bool,
    pub terminal: bool,
    pub resume: bool,
    pub usage: bool,
    pub replay: bool,
    pub tool_calls: bool,
    pub subagents: bool,
    pub live_state: bool,
    pub worktree: bool,
}

/// 检测结果：是否安装、binary 在哪、版本、以及可供只读访问的数据路径。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectResult {
    pub installed: bool,
    pub binary_path: Option<String>,
    pub version: Option<String>,
    pub data_paths: Vec<String>,
}

/// 启动一次 Harness 会话所需的全部输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchRequest {
    /// 会话归属项目；v0.1 允许为空（临时会话）。
    pub project_id: Option<String>,
    pub cwd: String,
    pub args: Vec<String>,
    /// 运行目标（ADR-0021）：v0.1 恒为 `local`，接口为远程预留。
    pub runtime_target_id: String,
}

/// 恢复一次已有会话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeRequest {
    pub hub_session_id: String,
    /// 外部 Harness 自己的 session id（若该 Harness 支持 resume）。
    pub source_session_id: Option<String>,
    pub cwd: String,
    pub runtime_target_id: String,
}

/// 启动后的进程句柄。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessHandle {
    pub hub_session_id: String,
    pub pid: Option<u32>,
}

/// Harness 生命周期接口。实现者必须可跨线程共享（注册表按 `&dyn` 持有）。
pub trait HarnessAdapter: Send + Sync {
    fn id(&self) -> HarnessId;

    /// 扫描本机：是否安装、binary 路径、数据路径。不得有副作用。
    fn detect(&self) -> DetectResult;

    /// 读取版本号；未安装或读取失败时返回 `Ok(None)` / `Err`。
    fn version(&self) -> Result<Option<String>>;

    fn capabilities(&self) -> HarnessCapabilities;

    fn launch(&self, request: LaunchRequest) -> Result<ProcessHandle>;

    fn resume(&self, request: ResumeRequest) -> Result<ProcessHandle>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_serialize_as_camel_case_for_ipc() {
        let capabilities = HarnessCapabilities {
            launch: true,
            tool_calls: true,
            live_state: true,
            ..HarnessCapabilities::default()
        };

        // HarnessCapabilities 是 Copy，按值传入即可（clippy: needless_borrows_for_generic_args）。
        let json = serde_json::to_value(capabilities).expect("序列化");

        assert_eq!(json["launch"], true);
        assert_eq!(json["toolCalls"], true, "IPC 契约是 camelCase");
        assert_eq!(json["liveState"], true);
        assert!(
            json.get("tool_calls").is_none(),
            "不得输出 snake_case 键：前端读的是 toolCalls"
        );
    }

    #[test]
    fn capabilities_default_is_all_unsupported() {
        let capabilities = HarnessCapabilities::default();

        assert_eq!(
            capabilities,
            HarnessCapabilities {
                launch: false,
                terminal: false,
                resume: false,
                usage: false,
                replay: false,
                tool_calls: false,
                subagents: false,
                live_state: false,
                worktree: false,
            },
            "默认必须是「全不支持」，避免未验证就宣称能力"
        );
    }
}
