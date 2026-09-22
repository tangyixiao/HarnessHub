//! Harness 适配器注册表。
//!
//! 注册表只负责「有哪些 Harness、各自能力如何」，不关心进程与 PTY 细节。

use serde::Serialize;

use crate::harness::adapter::{HarnessAdapter, HarnessCapabilities, HarnessId};

/// 一个 Harness 的对外快照：检测结果 + 能力矩阵。
///
/// 这是 IPC 契约类型，字段名序列化为 camelCase，前端 `src/lib/ipc.ts` 的
/// `HarnessSummary` 必须与之同形；改名即破坏契约，必须同步两侧测试。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessSummary {
    pub id: String,
    pub display_name: String,
    pub installed: bool,
    pub binary_path: Option<String>,
    pub version: Option<String>,
    pub capabilities: HarnessCapabilities,
    pub data_paths: Vec<String>,
}

/// 展示名映射。未登记的 Harness 直接回显 id，避免编造名字。
fn display_name_for(id: &str) -> String {
    match id {
        "codex" => "Codex".to_string(),
        "claude-code" => "Claude Code".to_string(),
        "gemini-cli" => "Gemini CLI".to_string(),
        "opencode" => "OpenCode".to_string(),
        other => other.to_string(),
    }
}

/// 已注册适配器的集合。
#[derive(Default)]
pub struct HarnessRegistry {
    adapters: Vec<Box<dyn HarnessAdapter>>,
}

impl HarnessRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个适配器。同一 id 重复注册时返回 `false`，不覆盖已有实现。
    pub fn register(&mut self, adapter: Box<dyn HarnessAdapter>) -> bool {
        if self.get(&adapter.id()).is_some() {
            return false;
        }
        self.adapters.push(adapter);
        true
    }

    pub fn get(&self, id: &HarnessId) -> Option<&dyn HarnessAdapter> {
        self.adapters
            .iter()
            .find(|adapter| adapter.id() == *id)
            .map(|adapter| adapter.as_ref())
    }

    pub fn iter(&self) -> impl Iterator<Item = &dyn HarnessAdapter> {
        self.adapters.iter().map(|adapter| adapter.as_ref())
    }

    pub fn ids(&self) -> Vec<HarnessId> {
        self.iter().map(|adapter| adapter.id()).collect()
    }

    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }

    /// 所有已注册 Harness 的检测结果。
    pub fn detect_all(&self) -> Vec<(HarnessId, bool)> {
        self.iter()
            .map(|adapter| (adapter.id(), adapter.detect().installed))
            .collect()
    }

    /// 某个 Harness 的能力矩阵；未注册时返回 `None`。
    pub fn capabilities(&self, id: &HarnessId) -> Option<HarnessCapabilities> {
        self.get(id).map(|adapter| adapter.capabilities())
    }

    /// 面向 UI 的快照：逐个适配器执行真实检测，并带上其能力矩阵。
    ///
    /// `detect()` 不返回 `Result`（见 `HarnessAdapter`）：检测失败必须表现为
    /// 「不可用但可展示」的状态，而不是让整个 IPC 调用失败。
    pub fn summaries(&self) -> Vec<HarnessSummary> {
        self.iter()
            .map(|adapter| {
                let id = adapter.id();
                let detect = adapter.detect();

                HarnessSummary {
                    display_name: display_name_for(id.as_str()),
                    id: id.as_str().to_string(),
                    installed: detect.installed,
                    binary_path: detect.binary_path,
                    version: detect.version,
                    capabilities: adapter.capabilities(),
                    data_paths: detect.data_paths,
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{Error, Result};
    use crate::harness::adapter::{
        DetectResult, HarnessCapabilities, LaunchRequest, ProcessHandle, ResumeRequest,
    };

    struct FakeAdapter {
        id: HarnessId,
        installed: bool,
        capabilities: HarnessCapabilities,
    }

    impl FakeAdapter {
        fn new(id: &str, installed: bool) -> Self {
            Self {
                id: HarnessId::from(id),
                installed,
                capabilities: HarnessCapabilities {
                    launch: true,
                    terminal: true,
                    ..HarnessCapabilities::default()
                },
            }
        }
    }

    impl HarnessAdapter for FakeAdapter {
        fn id(&self) -> HarnessId {
            self.id.clone()
        }

        fn detect(&self) -> DetectResult {
            DetectResult {
                installed: self.installed,
                binary_path: self.installed.then(|| format!("/usr/bin/{}", self.id)),
                version: self.installed.then(|| "1.0.0".to_string()),
                data_paths: Vec::new(),
            }
        }

        fn version(&self) -> Result<Option<String>> {
            Ok(self.installed.then(|| "1.0.0".to_string()))
        }

        fn capabilities(&self) -> HarnessCapabilities {
            self.capabilities
        }

        fn launch(&self, _request: LaunchRequest) -> Result<ProcessHandle> {
            Err(Error::InvalidInput("fake adapter 不启动进程".to_string()))
        }

        fn resume(&self, _request: ResumeRequest) -> Result<ProcessHandle> {
            Err(Error::InvalidInput("fake adapter 不恢复进程".to_string()))
        }
    }

    #[test]
    fn new_registry_is_empty() {
        let registry = HarnessRegistry::new();

        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(registry.ids().is_empty());
    }

    #[test]
    fn register_then_lookup_by_id() {
        let mut registry = HarnessRegistry::new();

        assert!(registry.register(Box::new(FakeAdapter::new("codex", true))));

        let adapter = registry
            .get(&HarnessId::from("codex"))
            .expect("应能查到 codex");
        assert!(adapter.capabilities().launch);
        assert!(registry.get(&HarnessId::from("nope")).is_none());
    }

    #[test]
    fn duplicate_registration_is_rejected_without_overwriting() {
        let mut registry = HarnessRegistry::new();
        registry.register(Box::new(FakeAdapter::new("codex", true)));

        let registered_again = registry.register(Box::new(FakeAdapter::new("codex", false)));

        assert!(!registered_again);
        assert_eq!(registry.len(), 1);
        assert!(
            registry
                .get(&HarnessId::from("codex"))
                .expect("codex")
                .detect()
                .installed,
            "重复注册不得覆盖已有实现"
        );
    }

    #[test]
    fn detect_all_reports_installation_state() {
        let mut registry = HarnessRegistry::new();
        registry.register(Box::new(FakeAdapter::new("codex", true)));
        registry.register(Box::new(FakeAdapter::new("gemini-cli", false)));

        let mut results = registry.detect_all();
        results.sort();

        assert_eq!(
            results,
            vec![
                (HarnessId::from("codex"), true),
                (HarnessId::from("gemini-cli"), false),
            ]
        );
    }

    #[test]
    fn capabilities_are_reported_per_harness() {
        let mut registry = HarnessRegistry::new();
        registry.register(Box::new(FakeAdapter::new("codex", true)));

        let capabilities = registry
            .capabilities(&HarnessId::from("codex"))
            .expect("应有能力矩阵");

        assert!(capabilities.launch);
        assert!(!capabilities.usage, "未声明的能力必须默认关闭");
        assert!(registry.capabilities(&HarnessId::from("missing")).is_none());
    }

    #[test]
    fn summaries_describe_every_registered_adapter() {
        let mut registry = HarnessRegistry::new();
        registry.register(Box::new(FakeAdapter::new("codex", true)));

        let summaries = registry.summaries();

        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "codex");
        assert_eq!(summaries[0].display_name, "Codex");
        assert!(summaries[0].installed);
        assert_eq!(summaries[0].binary_path.as_deref(), Some("/usr/bin/codex"));
        assert_eq!(summaries[0].version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn summaries_carry_the_adapters_capability_matrix() {
        let mut registry = HarnessRegistry::new();
        registry.register(Box::new(FakeAdapter::new("codex", true)));

        let capabilities = registry.summaries()[0].capabilities;

        assert!(capabilities.launch, "能力必须来自适配器，而不是注册表猜的");
        assert!(capabilities.terminal);
        assert!(!capabilities.usage);
    }

    #[test]
    fn summaries_of_uninstalled_adapter_report_unavailable_without_lying() {
        let mut registry = HarnessRegistry::new();
        registry.register(Box::new(FakeAdapter::new("codex", false)));

        let summary = &registry.summaries()[0];

        assert!(!summary.installed);
        assert!(summary.binary_path.is_none());
        assert!(summary.version.is_none());
    }

    #[test]
    fn summaries_serialize_with_camel_case_keys() {
        // 前后端契约测试：src/features/harnesses 依赖这些键名。
        // Rust 字段是 snake_case，JSON 必须是 camelCase，否则前端读到 undefined。
        let mut registry = HarnessRegistry::new();
        registry.register(Box::new(FakeAdapter::new("codex", true)));

        let json = serde_json::to_value(&registry.summaries()[0]).expect("序列化");

        assert_eq!(json["id"], "codex");
        assert_eq!(json["displayName"], "Codex");
        assert_eq!(json["installed"], true);
        assert_eq!(json["binaryPath"], "/usr/bin/codex");
        assert_eq!(json["version"], "1.0.0");
        assert_eq!(json["dataPaths"], serde_json::json!([]));
        assert_eq!(json["capabilities"]["launch"], true);
        assert_eq!(json["capabilities"]["toolCalls"], false);
        assert_eq!(json["capabilities"]["liveState"], false);
        assert!(
            json["capabilities"].get("tool_calls").is_none(),
            "不得同时输出 snake_case 键，避免两套契约并存"
        );
        assert!(json.get("display_name").is_none());
        assert!(json.get("binary_path").is_none());
    }

    #[test]
    fn display_name_falls_back_to_id_for_unknown_harness() {
        assert_eq!(display_name_for("codex"), "Codex");
        assert_eq!(display_name_for("claude-code"), "Claude Code");
        assert_eq!(
            display_name_for("mystery-cli"),
            "mystery-cli",
            "未登记的名字直接回显 id，不要编造"
        );
    }
}
