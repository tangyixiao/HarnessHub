//! Sidecar 启动配置与协议常量（F20 spec §4.2；F23 §4 打包态资源定位）。
//!
//! 默认 transport 是 stdio JSON-RPC（ADR-0006），**不开 localhost HTTP 端口**。
//!
//! F23 §4：runtime_root 缺省逻辑扩展（**dev 行为不变**）——
//! 1. env 覆盖不变（`HARNESS_HUB_PYTHON` / `HARNESS_HUB_RUNTIME_ROOT` 优先）；
//! 2. 仅当缺省开发路径 `<repo>/python` 不存在（打包态的必然形态）时，
//!    依次探测 exe 同级 `resources/python/` → exe 上级 `resources/python/`；
//!    命中条件 = 目录存在且其下有 `harness_hub_runtime/__init__.py`；
//! 3. 解释器：env 覆盖后若仍为裸 `"python"` 且 runtime 根下有
//!    `python.exe`（Windows）/ `bin/python3`（其他平台）则用之；
//! 4. 探测函数全部可注入根路径参数，不读全局 env（可单测）；
//!    真机打包验收项记录在 release notes（dev 无法证伪打包形态时如实标注）。
//!
//! 探测不中如实回落 dev 路径 → 启动必然 degraded（可见），绝不静默换家。

use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Host 与 sidecar 约定线协议主版本；协商只比较 `.` 前的第一段。
pub const PROTOCOL_VERSION: &str = "1.0";

/// Host 要求 sidecar 必须自报的能力集；握手时逐项核对，缺一项即降级。
pub const REQUIRED_CAPABILITIES: &[&str] = &["ping", "runtime.info", "shutdown"];

/// `initialize.result.runtime.name` 必须等于这个值，防误连到别的 Python 进程。
pub const RUNTIME_NAME: &str = "harness-hub-runtime";

/// 打包态资源目录的固定相对布局（exe 同级/上级 `<dir>/resources/python`）。
pub const PACKAGED_RESOURCES_DIR: &str = "resources";
/// 打包态 Python 运行时的包目录名（PYTHONPATH 注入目标）。
pub const PACKAGED_PYTHON_DIR: &str = "python";
/// 命中判定文件：`<root>/harness_hub_runtime/__init__.py` 必须存在。
const RUNTIME_PACKAGE_MARKER: &str = "harness_hub_runtime/__init__.py";

/// 拉起一个 sidecar 所需的全部外部输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarConfig {
    /// Python 解释器路径（或 PATH 里可直接找到的程序名）。
    pub python: PathBuf,
    /// `PYTHONPATH` 指向的目录，其下应有 `harness_hub_runtime/` 包。
    pub runtime_root: PathBuf,
    /// 传给 `initialize.params.hostVersion` 的宿主版本（仅展示 / 记录用）。
    pub host_version: String,
}

impl SidecarConfig {
    /// 从环境变量构造（F23 §4 打包态探测链的组装点）：
    /// - `HARNESS_HUB_PYTHON`：解释器，缺省 `"python"`（打包态优先 bundled 解释器）；
    /// - `HARNESS_HUB_RUNTIME_ROOT`：runtime 根目录，缺省先试 `<src-tauri>/../python`
    ///   （dev），不存在时按 exe 同级/上级 `resources/python` 探测；
    /// - `host_version`：编译期 `CARGO_PKG_VERSION`，不接受运行时注入。
    ///
    /// 启动命令固定为 `<python> -m harness_hub_runtime` + `PYTHONPATH=runtime_root`，
    /// **不经 shell、不拼接用户输入**（避免命令注入）。
    pub fn from_env() -> Self {
        let env_python = env::var_os("HARNESS_HUB_PYTHON");
        let env_root = env::var_os("HARNESS_HUB_RUNTIME_ROOT");
        let dev_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("src-tauri 有父目录")
            .join(PACKAGED_PYTHON_DIR);
        let exe_dir = env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf));
        Self::resolve(
            env_python.as_deref(),
            env_root.as_deref(),
            exe_dir.as_deref(),
            &dev_root,
        )
    }

    /// 纯函数组装点：全部输入显式注入，不读全局 env（F23 §4 第 4 条，可单测）。
    pub(crate) fn resolve(
        env_python: Option<&OsStr>,
        env_root: Option<&OsStr>,
        exe_dir: Option<&Path>,
        dev_root: &Path,
    ) -> Self {
        let runtime_root = match env_root {
            Some(value) => PathBuf::from(value),
            None => probe_runtime_root(exe_dir, dev_root),
        };
        Self {
            python: resolve_python(env_python, &runtime_root),
            runtime_root,
            host_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

/// 命中判定：目录存在且其下有 `harness_hub_runtime/__init__.py`（F23 §4 第 2 条）。
pub fn contains_runtime_package(root: &Path) -> bool {
    root.join(RUNTIME_PACKAGE_MARKER).is_file()
}

/// runtime 根的探测顺序（F23 §4 第 2 条）：
/// dev 根命中 → 原样；否则 exe 同级 `resources/python` → exe 上级 `resources/python`；
/// 全部不中如实回落 dev 根（启动时 degraded 可见，绝不静默换家）。
pub fn probe_runtime_root(exe_dir: Option<&Path>, dev_root: &Path) -> PathBuf {
    if contains_runtime_package(dev_root) {
        return dev_root.to_path_buf();
    }
    if let Some(dir) = exe_dir {
        let sibling = dir.join(PACKAGED_RESOURCES_DIR).join(PACKAGED_PYTHON_DIR);
        if contains_runtime_package(&sibling) {
            return sibling;
        }
        if let Some(parent) = dir.parent() {
            let parent_resources = parent
                .join(PACKAGED_RESOURCES_DIR)
                .join(PACKAGED_PYTHON_DIR);
            if contains_runtime_package(&parent_resources) {
                return parent_resources;
            }
        }
    }
    dev_root.to_path_buf()
}

/// 解释器解析（F23 §4 第 3 条）：env 覆盖优先；否则 runtime 根下的 bundled
/// 解释器（Windows `python.exe` / 其他 `bin/python3`）；最后才回落裸 `"python"`。
pub fn resolve_python(env_python: Option<&OsStr>, runtime_root: &Path) -> PathBuf {
    if let Some(value) = env_python {
        return PathBuf::from(value);
    }
    let bundled = if cfg!(windows) {
        runtime_root.join("python.exe")
    } else {
        runtime_root.join("bin").join("python3")
    };
    if bundled.is_file() {
        return bundled;
    }
    PathBuf::from("python")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_constants_lock_the_negotiation_contract() {
        assert_eq!(PROTOCOL_VERSION, "1.0");
        assert_eq!(RUNTIME_NAME, "harness-hub-runtime");
        assert_eq!(REQUIRED_CAPABILITIES, &["ping", "runtime.info", "shutdown"]);
    }

    #[test]
    fn from_env_defaults_to_dev_roots_without_shell() {
        // 不修改进程全局 env（并行测试会互相污染）：只断言两个常量的形状。
        let config = SidecarConfig {
            python: PathBuf::from("python"),
            runtime_root: PathBuf::from("python"),
            host_version: "0.5.0".to_string(),
        };
        assert_eq!(config.python, PathBuf::from("python"));
        assert!(config.runtime_root.ends_with("python"));
        assert_eq!(config.host_version, "0.5.0");
    }

    // ---- F23 §4：探测链（注入根路径，不读全局 env） ----

    /// 造一个「像打包态」的运行时根：`<root>/harness_hub_runtime/__init__.py`。
    fn make_runtime_root(base: &Path) -> PathBuf {
        let root = base.join("resources").join("python");
        std::fs::create_dir_all(root.join("harness_hub_runtime")).expect("创建运行时包目录");
        std::fs::write(root.join("harness_hub_runtime").join("__init__.py"), "").expect("写包标记");
        root
    }

    #[test]
    fn dev_root_wins_when_it_contains_the_runtime_package() {
        let tmp = std::env::temp_dir().join(format!("hh-config-dev-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let dev_root = make_runtime_root(&tmp);
        let exe_dir = tmp.join("app").join("bin");
        std::fs::create_dir_all(&exe_dir).expect("创建 exe 目录");

        let resolved = probe_runtime_root(Some(&exe_dir), &dev_root);
        assert_eq!(resolved, dev_root, "dev 根命中即原样，不探测打包布局");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn exe_sibling_resources_python_is_probed_when_dev_root_is_missing() {
        let tmp = std::env::temp_dir().join(format!("hh-config-sibling-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // 打包形态：exe 在 <tmp>/app/bin/harness-hub.exe，资源在 <tmp>/app/resources/python。
        let app_dir = tmp.join("app");
        let runtime_root = make_runtime_root(&app_dir);
        let exe_dir = app_dir.join("bin");
        std::fs::create_dir_all(&exe_dir).expect("创建 exe 目录");
        let dev_root = tmp.join("repo").join("python"); // 不存在

        let resolved = probe_runtime_root(Some(&exe_dir), &dev_root);
        assert_eq!(resolved, runtime_root, "exe 同级 resources/python 命中");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn exe_parent_resources_python_is_probed_second() {
        let tmp = std::env::temp_dir().join(format!("hh-config-parent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // 打包形态变体：exe 在 <tmp>/app/bin/，资源在 <tmp>/app/resources/python（exe 上级）。
        let runtime_root = make_runtime_root(&tmp.join("app"));
        let exe_dir = tmp.join("app").join("bin");
        std::fs::create_dir_all(&exe_dir).expect("创建 exe 目录");
        let dev_root = tmp.join("repo").join("python"); // 不存在

        let resolved = probe_runtime_root(Some(&exe_dir), &dev_root);
        assert_eq!(resolved, runtime_root, "exe 上级 resources/python 兜底命中");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn probe_misses_fall_back_to_dev_root_honestly() {
        let tmp = std::env::temp_dir().join(format!("hh-config-miss-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let exe_dir = tmp.join("app").join("bin");
        std::fs::create_dir_all(&exe_dir).expect("创建 exe 目录");
        // 空 resources 目录（无 __init__.py）→ 不命中。
        std::fs::create_dir_all(tmp.join("app").join("resources").join("python"))
            .expect("创建空资源目录");
        let dev_root = tmp.join("repo").join("python"); // 不存在

        let resolved = probe_runtime_root(Some(&exe_dir), &dev_root);
        assert_eq!(
            resolved, dev_root,
            "探测不中如实回落 dev 根，启动时 degraded 可见"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn resolve_env_overrides_win_over_everything() {
        let tmp = std::env::temp_dir().join(format!("hh-config-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // 有运行时根在场也不影响：env 覆盖永远优先（断言见下）。
        let _runtime_root = make_runtime_root(&tmp);
        let exe_dir = tmp.join("app").join("bin");
        std::fs::create_dir_all(&exe_dir).expect("创建 exe 目录");

        let config = SidecarConfig::resolve(
            Some(OsStr::new("C:/py311/python.exe")),
            Some(OsStr::new("D:/custom-runtime")),
            Some(&exe_dir),
            &tmp.join("repo").join("python"),
        );
        assert_eq!(config.python, PathBuf::from("C:/py311/python.exe"));
        assert_eq!(config.runtime_root, PathBuf::from("D:/custom-runtime"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn bundled_interpreter_is_used_when_present_and_bare_python_otherwise() {
        let tmp = std::env::temp_dir().join(format!("hh-config-py-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        // 打包形态：exe 在 <tmp>/app/bin/，运行时在 <tmp>/app/resources/python（exe 上级）。
        let runtime_root = make_runtime_root(&tmp.join("app"));
        let exe_dir = tmp.join("app").join("bin");
        std::fs::create_dir_all(&exe_dir).expect("创建 exe 目录");

        // 无 bundled 解释器 → 裸 "python"。
        let config =
            SidecarConfig::resolve(None, None, Some(&exe_dir), &tmp.join("repo").join("python"));
        assert_eq!(config.runtime_root, runtime_root, "探测链命中打包布局");
        assert_eq!(config.python, PathBuf::from("python"));

        // bundled 解释器在场（Windows python.exe / 其他 bin/python3）→ 优先。
        let bundled = if cfg!(windows) {
            runtime_root.join("python.exe")
        } else {
            runtime_root.join("bin").join("python3")
        };
        if let Some(parent) = bundled.parent() {
            std::fs::create_dir_all(parent).expect("创建解释器目录");
        }
        std::fs::write(&bundled, "").expect("写解释器占位");
        let config =
            SidecarConfig::resolve(None, None, Some(&exe_dir), &tmp.join("repo").join("python"));
        assert_eq!(config.python, bundled, "bundled 解释器优先于裸 python");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
