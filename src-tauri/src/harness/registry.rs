//! Harness 适配器注册表。
//!
//! 注册表只负责「有哪些 Harness、各自能力如何」，不关心进程与 PTY 细节。

use crate::harness::adapter::{HarnessAdapter, HarnessCapabilities, HarnessId};

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
}
