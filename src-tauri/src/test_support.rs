//! 测试夹具：内存库 + 最小种子数据。
//!
//! **测试纪律**（review 结论，已写入 AGENTS.md）：
//! 涉及 FK / migration / registry bootstrap 的测试，必须至少有一组从**真正空库**
//! 开始、只走生产路径填充；不允许夹具偷偷替生产代码补前置状态。
//!
//! 真实宿主上已经因此漏掉过一个 FK 缺陷：`seeded_db()` 替调用方插好了 harness 行，
//! 于是单元测试全绿，而冷启动的 `create_session` 直接
//! `FOREIGN KEY constraint failed`。所以这里有两个夹具，用途必须分清：
//!
//! - [`empty_db`] / [`cold_start_db`]：冷启动路径，**生产代码负责填数据**；
//! - [`seeded_db`]：只给存储层单测用的便捷夹具，禁止用它验证集成路径。

use rusqlite::params;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::db::Database;
use crate::error::Result;
use crate::harness::adapter::HarnessCapabilities;
use crate::harness::probe::HostProbe;
use crate::harness::registry::HarnessSummary;
use crate::usage::runner::{CommandOutput, CommandRunner, CommandSpec};

/// 测试用 Harness id，与 `harnesses.id` 对应。
pub const HARNESS_ID: &str = "codex";

/// 测试用运行目标 id。**引用生产常量**，避免两处硬编码漂移。
pub const RUNTIME_TARGET_ID: &str = crate::runtime::local::LOCAL_TARGET_ID;

/// 固定时间戳：测试里不要依赖真实当前时间。
pub const NOW: &str = "2026-01-01T00:00:00Z";

/// 只有 schema、没有任何业务数据的库。
pub fn empty_db() -> Database {
    Database::open_in_memory().expect("打开内存库")
}

/// 「检测到本机装了 Codex」的最小快照，供 reconcile 走生产路径。
pub fn detected_codex_summary() -> HarnessSummary {
    HarnessSummary {
        id: HARNESS_ID.to_string(),
        display_name: "Codex".to_string(),
        installation_id: None,
        installed: true,
        binary_path: Some("D:/npm-global/codex.cmd".to_string()),
        version: Some("0.152.1".to_string()),
        capabilities: HarnessCapabilities::default(),
        data_paths: vec!["C:/Users/dev/.codex".to_string()],
    }
}

/// 冷启动：**空库 + 只调用生产引导代码**。
pub fn cold_start_db() -> Database {
    let db = empty_db();
    crate::runtime::local::ensure_local_target(db.connection()).expect("引导 runtime target");
    crate::harness::inventory::reconcile_harnesses(
        db.connection(),
        &[detected_codex_summary()],
        RUNTIME_TARGET_ID,
        NOW,
    )
    .expect("同步 Harness 清单");
    db
}

/// 便捷夹具：手工塞好 harness 定义 + 安装 + runtime target。
///
/// **只用于存储层单测**（例如验证 SQL 约束）。集成/冷启动测试请用 [`cold_start_db`]。
pub fn seeded_db() -> Database {
    let db = empty_db();
    seed_baseline(&db);
    db
}

/// 手工写入基线数据。重复调用会因主键冲突失败，调用方自行保证只调用一次。
pub fn seed_baseline(db: &Database) {
    let conn = db.connection();

    conn.execute(
        "INSERT INTO harnesses (id, display_name, created_at, updated_at)
         VALUES (?1, 'Codex', ?2, ?2)",
        params![HARNESS_ID, NOW],
    )
    .expect("插入 harness 定义");

    conn.execute(
        "INSERT INTO runtime_targets (id, kind, display_name, created_at)
         VALUES (?1, 'local', '本机', ?2)",
        params![RUNTIME_TARGET_ID, NOW],
    )
    .expect("插入 runtime target");

    conn.execute(
        "INSERT INTO harness_installations
            (id, harness_id, runtime_target_id, binary_path, version, capabilities_json,
             data_paths_json, availability, first_detected_at, last_seen_at)
         VALUES (?1, ?2, ?3, 'D:/npm-global/codex.cmd', '0.152.1', '{}', '[]',
                 'available', ?4, ?4)",
        params![
            crate::harness::inventory::installation_id(HARNESS_ID, RUNTIME_TARGET_ID),
            HARNESS_ID,
            RUNTIME_TARGET_ID,
            NOW
        ],
    )
    .expect("插入 harness 安装");
}

/// 可编程的假宿主：不碰真实文件系统，也不起真实进程。
///
/// `with(names)` 是最简形态（只关心「装没装」）；`with_binary` / `with_dir` 用于
/// 需要真实版本输出与数据目录的检测测试。
pub struct FakeHostProbe {
    executables: std::collections::HashMap<String, PathBuf>,
    versions: std::collections::HashMap<PathBuf, String>,
    dirs: std::collections::HashSet<PathBuf>,
    home: Option<PathBuf>,
}

impl FakeHostProbe {
    pub fn new() -> Self {
        Self {
            executables: std::collections::HashMap::new(),
            versions: std::collections::HashMap::new(),
            dirs: std::collections::HashSet::new(),
            home: Some(PathBuf::from("/home/dev")),
        }
    }

    /// 只声明「这个名字在 PATH 上」。
    pub fn with(names: &[&str]) -> Self {
        let mut probe = Self::new();
        for name in names {
            probe
                .executables
                .insert(name.to_string(), PathBuf::from(format!("D:/fake/{name}")));
        }
        probe
    }

    pub fn with_binary(mut self, name: &str, path: &str, version_output: &str) -> Self {
        let path = PathBuf::from(path);
        self.executables.insert(name.to_string(), path.clone());
        self.versions.insert(path, version_output.to_string());
        self
    }

    pub fn with_dir(mut self, path: &str) -> Self {
        self.dirs.insert(PathBuf::from(path));
        self
    }
}

impl Default for FakeHostProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl HostProbe for FakeHostProbe {
    fn find_executable(&self, name: &str) -> Option<PathBuf> {
        self.executables.get(name).cloned()
    }

    fn read_version(&self, executable: &Path) -> Result<Option<String>> {
        Ok(self
            .versions
            .get(executable)
            .and_then(|raw| crate::harness::probe::parse_version(raw)))
    }

    fn dir_exists(&self, path: &Path) -> bool {
        self.dirs.contains(path)
    }

    fn home_dir(&self) -> Option<PathBuf> {
        self.home.clone()
    }
}

/// 可编程的执行器：按**命令文本子串**匹配预设输出，并记录每次调用。
///
/// 匹配用子串（`--version` / `--sections`）而不是 program，这样能表达
/// 「版本探测成功、报告调用失败」这类真实场景。
/// `calls()` 是关键：用它证明「只读路径没有起进程」。
pub struct ScriptedCommandRunner {
    outputs: Vec<(String, CommandOutput)>,
    calls: Mutex<Vec<String>>,
}

impl ScriptedCommandRunner {
    pub fn new() -> Self {
        Self {
            outputs: Vec::new(),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// 按 program 名匹配（简写，等价于 `with_output(program, …)`）。
    pub fn returning(program: &str, exit_code: i32, stdout: &str, stderr: &str) -> Self {
        Self::new().with_output(program, exit_code, stdout, stderr)
    }

    pub fn with_output(
        mut self,
        matcher: &str,
        exit_code: i32,
        stdout: &str,
        stderr: &str,
    ) -> Self {
        self.outputs.push((
            matcher.to_string(),
            CommandOutput {
                exit_code,
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
            },
        ));
        self
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().expect("调用记录").clone()
    }
}

impl Default for ScriptedCommandRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRunner for ScriptedCommandRunner {
    fn run(&self, command: &CommandSpec) -> Result<CommandOutput> {
        let described = command.describe();
        self.calls.lock().expect("调用记录").push(described.clone());

        self.outputs
            .iter()
            .find(|(matcher, _)| described.contains(matcher.as_str()))
            .map(|(_, output)| output.clone())
            .ok_or_else(|| {
                crate::error::Error::UsageUnavailable(format!(
                    "假执行器没有为 {described} 配置输出"
                ))
            })
    }
}
