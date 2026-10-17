//! 插件目录发现：**只碰文件系统，不启进程、不碰数据库**。
//!
//! 为什么从 `registry.rs` 拆出来：那个模块头明说了自己「不扫插件目录、不 spawn」，
//! 把扫描塞回去会让 store 层再也无法在没有文件系统的单测里被验证。这里同样不做
//! 校验决策——校验归 [`super::manifest`]，这里只负责「有哪些目录像插件」。
//!
//! 两条与故障隔离（ADR-0018 决策 1 / R14）直接相关的约定：
//!
//! 1. **坏插件不影响好插件**。某个目录的 manifest 解析失败只记在它自己的
//!    [`DiscoveredPlugin`] 上，[`scan`] 绝不因此提前返回——否则一个手滑的
//!    manifest 就能让整台宿主看不到任何插件。
//! 2. **缺失的根目录是空列表，不是错误**。首次启动时 `plugins/` 还不存在是常态。

use std::path::{Path, PathBuf};

use super::manifest::Manifest;

/// 覆盖默认扫描根的环境变量（测试与开发期指向示例目录用）。
pub const ENV_PLUGINS_DIR: &str = "HARNESS_HUB_PLUGINS_DIR";

/// 扫描根：有环境变量就用它，否则 `<app_data_dir>/plugins`。
///
/// 空字符串按「没设」处理：`HARNESS_HUB_PLUGINS_DIR=` 这种写法在各种 shell 里都能
/// 出现，把它当成「根目录 = 当前目录」会让宿主悄悄扫到进程 cwd，行为随启动方式漂移。
pub fn default_root(app_data_dir: &Path) -> PathBuf {
    let env_value = std::env::var_os(ENV_PLUGINS_DIR)
        .map(PathBuf::from)
        .filter(|value| !value.as_os_str().is_empty());
    resolve_root(env_value.as_deref(), app_data_dir)
}

/// [`default_root`] 的纯函数内核（环境变量是进程级全局状态，不便在并行单测里改）。
fn resolve_root(env_value: Option<&Path>, app_data_dir: &Path) -> PathBuf {
    match env_value {
        Some(value) => value.to_path_buf(),
        None => app_data_dir.join("plugins"),
    }
}

/// 扫描到的一个候选目录。
///
/// `manifest` 与 `error` 互斥：前者是可用插件，后者是「这里有个目录坏了，为什么」。
#[derive(Debug)]
pub struct DiscoveredPlugin {
    pub dir: PathBuf,
    pub manifest: Option<Manifest>,
    pub error: Option<String>,
}

impl DiscoveredPlugin {
    pub fn is_ok(&self) -> bool {
        self.manifest.is_some()
    }
}

/// 列出 `root` 下所有含 `manifest.toml` 的直接子目录，按目录名升序。
///
/// 只扫一层：插件契约是「一个目录一个插件」，深层嵌套属于分发问题，不是发现问题
/// （`plugins/examples/` 这种分组目录要扫描就把它本身当根，见 [`ENV_PLUGINS_DIR`]）。
pub fn scan(root: &Path) -> Vec<DiscoveredPlugin> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        // 根目录不存在 = 还没装任何插件；首次启动必然如此。
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            // 权限不足之类的问题不能装作没插件：那会让「一个都扫不到」变成静默事实。
            // 调用方（refresh）要把这条错误呈现出来，而不是当成空目录。
            return vec![DiscoveredPlugin {
                dir: root.to_path_buf(),
                manifest: None,
                error: Some(format!("读取插件根目录 {} 失败：{error}", root.display())),
            }];
        }
    };

    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        if !dir.join("manifest.toml").is_file() {
            continue;
        }
        candidates.push(dir);
    }

    // 目录名排序而不是读目录顺序：Windows 的 read_dir 顺序不保证，UI 列表会抖。
    candidates.sort();

    candidates
        .into_iter()
        .map(|dir| match super::manifest::load(&dir) {
            Ok(manifest) => DiscoveredPlugin {
                dir,
                manifest: Some(manifest),
                error: None,
            },
            // 单个插件坏掉只记它自己：这里绝不 return Err，否则一个手滑的
            // manifest 能让整台宿主看不到任何插件（R14 故障隔离）。
            Err(error) => DiscoveredPlugin {
                dir: dir.clone(),
                manifest: None,
                error: Some(format!("{}：{error}", dir.display())),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "harness-hub-discovery-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("建临时根目录");
        root
    }

    fn write_manifest(dir: &Path, id: &str) {
        fs::create_dir_all(dir).expect("建插件目录");
        fs::write(
            dir.join("manifest.toml"),
            format!(
                r#"
id = "{id}"
name = "{id}"
version = "0.1.0"
api_version = "1"

[adapter]
command = ["python", "adapter/main.py"]

[capabilities]
usage = true
"#
            ),
        )
        .expect("写 manifest");
    }

    #[test]
    fn missing_root_is_an_empty_list_not_an_error() {
        let root = std::env::temp_dir().join("harness-hub-discovery-absent-never-created");

        assert!(
            scan(&root).is_empty(),
            "首次启动时 plugins/ 不存在是常态，不能报错"
        );
    }

    /// 这是故障隔离的最小证据：一个坏 manifest 不能连累旁边的插件。
    #[test]
    fn a_broken_manifest_does_not_hide_a_healthy_sibling() {
        let root = temp_root("mixed");
        write_manifest(&root.join("good"), "good");
        // 缺 api_version + 空 command：必然校验失败。
        fs::create_dir_all(root.join("bad")).expect("建坏插件目录");
        fs::write(
            root.join("bad/manifest.toml"),
            "id = \"bad\"\nname = \"Bad\"\n",
        )
        .expect("写坏 manifest");

        let found = scan(&root);

        assert_eq!(found.len(), 2, "坏目录也要出现在结果里，否则用户看不到它");

        let good = found
            .iter()
            .find(|plugin| plugin.dir.ends_with("good"))
            .expect("好插件必须在");
        assert_eq!(
            good.manifest.as_ref().map(|manifest| manifest.id.as_str()),
            Some("good")
        );
        assert!(good.error.is_none());

        let bad = found
            .iter()
            .find(|plugin| plugin.dir.ends_with("bad"))
            .expect("坏插件也要在结果里");
        assert!(bad.manifest.is_none());
        let error = bad.error.as_deref().unwrap_or_default();
        assert!(!error.is_empty(), "失败必须带可诊断的原因，否则用户只能猜");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn directories_without_a_manifest_are_ignored() {
        let root = temp_root("no-manifest");
        fs::create_dir_all(root.join("just-some-dir")).expect("建目录");
        fs::write(root.join("README.md"), "不是插件").expect("写文件");
        write_manifest(&root.join("real"), "real");

        let found = scan(&root);

        assert_eq!(found.len(), 1, "只有含 manifest.toml 的目录才是插件候选");
        assert!(found[0].dir.ends_with("real"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn results_are_sorted_by_directory_name() {
        let root = temp_root("sorted");
        for id in ["zulu", "alpha", "mike"] {
            write_manifest(&root.join(id), id);
        }

        let ids: Vec<String> = scan(&root)
            .into_iter()
            .filter_map(|plugin| plugin.manifest.map(|manifest| manifest.id))
            .collect();

        assert_eq!(ids, vec!["alpha", "mike", "zulu"], "顺序不稳定会让 UI 抖动");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn default_root_prefers_the_env_override() {
        let app_data = Path::new("C:/app-data");

        assert_eq!(
            resolve_root(Some(Path::new("D:/custom-plugins")), app_data),
            PathBuf::from("D:/custom-plugins")
        );
        assert_eq!(
            resolve_root(None, app_data),
            PathBuf::from("C:/app-data").join("plugins")
        );
    }
}
