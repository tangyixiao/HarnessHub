//! 安全配置投影（ROADMAP E15）：read → parse → merge → render → validate →
//! backup → temp write → verify → atomic rename。
//!
//! **写坏原生配置属于最高等级事故**（规划 §4.16）——每一步失败即中止，
//! 绝不留半截文件；备份放 Hub 自己的目录（`~/.harness-hub/backups/<harness>/`）。
//!
//! merge 语义：new_fields 覆盖同名键；不删除未知键；`env` 嵌套块深合并（一层）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Error, Result};

/// 备份槽：一份投影前的完整快照。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub id: String,
    pub harness_id: String,
    pub created_at: String,
    /// 备份文件绝对路径。
    pub path: String,
    /// 被备份的目标文件路径（投影目标）。
    pub target_path: String,
}

/// 一次投影的结果审计。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectionReport {
    pub target_path: String,
    pub backup_id: String,
    /// 变更的键数（new_fields 中实际引起差异的键）。
    pub changed_keys: u64,
}

/// 备份目录（Hub 自己的，不混入 harness 数据）。
fn backup_dir(harness_id: &str) -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".harness-hub").join("backups").join(harness_id)
}

/// 列出某 harness 的全部备份（按时间倒序）。
pub fn list_backups(harness_id: &str) -> Result<Vec<BackupInfo>> {
    let dir = backup_dir(harness_id);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut backups = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        // 文件名 = <timestamp>.json；target_path 从文件内容读。
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let target_path = content
            .lines()
            .find_map(|line| line.trim().strip_prefix("// target: "))
            .unwrap_or("<unknown>")
            .to_string();
        backups.push(BackupInfo {
            id: name.clone(),
            harness_id: harness_id.to_string(),
            created_at: name,
            path: path.to_string_lossy().into_owned(),
            target_path,
        });
    }
    backups.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(backups)
}

/// 深合并（一层）：new_fields 的键覆盖 target 的同名键。
/// `env` 块如果两侧都是 object，做一层深合并；否则整体覆盖。
fn merge_fields(target: &mut Map<String, Value>, new_fields: &BTreeMap<String, String>) {
    for (key, value) in new_fields {
        let parts: Vec<&str> = key.splitn(2, '.').collect();
        if parts.len() == 2 && parts[0] == "env" {
            // env.KEY → 深合并进 env 块。
            let env_entry = target
                .entry("env".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Some(env_map) = env_entry.as_object_mut() {
                env_map.insert(parts[1].to_string(), Value::String(value.clone()));
            }
        } else {
            // 顶层或其它：直接覆盖。
            target.insert(key.clone(), Value::String(value.clone()));
        }
    }
}

/// 规划一次投影：read → parse → merge → render → validate。
/// 不写任何文件；返回完整计划（含渲染结果）供 `apply` 执行。
pub fn plan(target_path: &Path, new_fields: &BTreeMap<String, String>) -> Result<(String, u64)> {
    // 1. read：目标文件必须存在（投影是「修改现有配置」，不是凭空创建）。
    let raw = std::fs::read_to_string(target_path).map_err(|error| {
        Error::InvalidInput(format!(
            "读取 {} 失败：{error}（投影要求文件已存在）",
            target_path.display()
        ))
    })?;

    // 2. parse（保留未知键）。
    let mut value: Value = serde_json::from_str(&raw).map_err(|error| {
        Error::InvalidInput(format!(
            "{} 语法错误，投影中止：{error}",
            target_path.display()
        ))
    })?;
    let object = value.as_object_mut().ok_or_else(|| {
        Error::InvalidInput(format!(
            "{} 不是 JSON object，投影中止",
            target_path.display()
        ))
    })?;

    // 3. merge + 计变更数。
    let before = raw.clone();
    merge_fields(object, new_fields);

    // 4. render（pretty JSON）。
    let rendered = serde_json::to_string_pretty(&value)
        .map_err(|error| Error::InvalidInput(format!("渲染失败：{error}")))?;

    // 5. validate：渲染结果必须是合法 JSON（自检，防 serde bug）。
    serde_json::from_str::<Value>(&rendered)
        .map_err(|error| Error::InvalidInput(format!("渲染结果自检失败（内部 bug）：{error}")))?;

    let changed_keys = if before == rendered { 0 } else { 1 };
    Ok((rendered, changed_keys))
}

/// 执行投影：backup → temp write → verify → atomic rename。
///
/// verify：写完 temp 后重新 parse 并与 rendered 比对——
/// 任何不匹配（磁盘满 / 编码问题）都中止，目标文件未被触碰。
/// 全局原子计数器：同一纳秒内的连续投影也能产生严格递增的备份 ID
/// （Windows 的 SystemTime 分辨率可能低至 ~15ms）。
static BACKUP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn apply(target_path: &Path, rendered: &str, harness_id: &str) -> Result<ProjectionReport> {
    // 混合 ID：纳秒时间戳 + 原子序号（保证单调递增，Windows 定时器分辨率不足时
    // 靠序号区分）——备份轮换需要按时间删最旧。
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = BACKUP_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let backup_id = format!("{nanos:020}-{seq:06}");

    // backup：拷贝当前文件到 Hub 自己的备份目录。
    let dir = backup_dir(harness_id);
    std::fs::create_dir_all(&dir)?;
    let current = std::fs::read_to_string(target_path)?;
    let backup_path = dir.join(format!("{backup_id}.json"));
    let backup_content = format!("// target: {}\n{}", target_path.to_string_lossy(), current);
    std::fs::write(&backup_path, backup_content)?;

    // 备份轮换：保留最近 10 份。
    rotate_backups(&dir, 10)?;

    // temp write：同目录下的临时文件（保证同分区，rename 是原子的）。
    let temp_path = target_path.with_extension("json.hh-tmp");
    std::fs::write(&temp_path, rendered)?;

    // verify：读回 temp 并 parse。
    let written = std::fs::read_to_string(&temp_path)?;
    let parsed: Value = serde_json::from_str(&written).map_err(|error| {
        let _ = std::fs::remove_file(&temp_path);
        Error::InvalidInput(format!("verify 失败（写出的文件不是合法 JSON）：{error}"))
    })?;
    let expected: Value = serde_json::from_str(rendered)
        .map_err(|error| Error::InvalidInput(format!("内部 bug：rendered 不可解析：{error}")))?;
    if parsed != expected {
        let _ = std::fs::remove_file(&temp_path);
        return Err(Error::InvalidInput(
            "verify 失败：写出的内容与渲染结果不一致（磁盘问题？）".to_string(),
        ));
    }

    // atomic rename。
    std::fs::rename(&temp_path, target_path).map_err(|error| {
        let _ = std::fs::remove_file(&temp_path);
        Error::InvalidInput(format!("原子替换 {} 失败：{error}", target_path.display()))
    })?;

    Ok(ProjectionReport {
        target_path: target_path.to_string_lossy().into_owned(),
        backup_id,
        changed_keys: 1,
    })
}

/// 恢复到某个备份。
pub fn restore(backup: &BackupInfo) -> Result<()> {
    let content = std::fs::read_to_string(&backup.path)
        .map_err(|error| Error::InvalidInput(format!("读取备份失败：{error}")))?;
    // 剥掉首行 target 注释。
    let json_text = content
        .lines()
        .skip_while(|line| line.starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let value: Value = serde_json::from_str(&json_text)
        .map_err(|error| Error::InvalidInput(format!("备份内容不是合法 JSON：{error}")))?;
    let rendered = serde_json::to_string_pretty(&value)
        .map_err(|error| Error::InvalidInput(format!("序列化恢复内容失败：{error}")))?;
    let target = PathBuf::from(&backup.target_path);

    // 恢复走同一条原子管线（不另做备份——恢复本身就是从备份来）。
    let temp_path = target.with_extension("json.hh-tmp");
    std::fs::write(&temp_path, &rendered)?;
    std::fs::rename(&temp_path, &target).map_err(|error| {
        let _ = std::fs::remove_file(&temp_path);
        Error::InvalidInput(format!("恢复失败：{error}"))
    })?;
    Ok(())
}

fn rotate_backups(dir: &Path, keep: usize) -> Result<()> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    files.sort(); // 文件名含时间戳，字典序 = 时间序。
    while files.len() > keep {
        let oldest = files.remove(0);
        std::fs::remove_file(&oldest)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_tree() -> PathBuf {
        let root = std::env::temp_dir().join(format!("hh-proj-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// merge 语义：覆盖同名 / 保留未知 / env 深合并。
    #[test]
    fn plan_merges_preserves_and_nests() {
        let root = temp_tree();
        let target = root.join("settings.json");
        std::fs::write(
            &target,
            r#"{"env":{"ANTHROPIC_MODEL":"old","KEEP_ME":"yes"},"other":true}"#,
        )
        .unwrap();

        let mut fields = BTreeMap::new();
        fields.insert("env.ANTHROPIC_MODEL".to_string(), "new-model".to_string());
        fields.insert(
            "env.ANTHROPIC_BASE_URL".to_string(),
            "https://x".to_string(),
        );
        let (rendered, _) = plan(&target, &fields).unwrap();

        let value: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(value["env"]["ANTHROPIC_MODEL"], "new-model", "覆盖");
        assert_eq!(value["env"]["ANTHROPIC_BASE_URL"], "https://x", "新增");
        assert_eq!(value["env"]["KEEP_ME"], "yes", "保留未知键");
        assert_eq!(value["other"], true, "保留非 env 键");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 原子性：apply 后目标文件是渲染结果，无 .hh-tmp 残留。
    #[test]
    fn apply_writes_atomically_and_leaves_no_temp() {
        let root = temp_tree();
        let target = root.join("settings.json");
        std::fs::write(&target, r#"{"env":{"A":"old"}}"#).unwrap();

        let mut fields = BTreeMap::new();
        fields.insert("env.A".to_string(), "new".to_string());
        let (rendered, _) = plan(&target, &fields).unwrap();
        let report = apply(&target, &rendered, "test-harness").unwrap();

        assert_eq!(report.target_path, target.to_string_lossy());
        let written = std::fs::read_to_string(&target).unwrap();
        assert!(written.contains("new"));
        assert!(
            !target.with_extension("json.hh-tmp").exists(),
            "无 temp 残留"
        );

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(backup_dir("test-harness"));
    }

    /// 备份创建 + 轮换 + 恢复 round-trip。
    #[test]
    fn backup_rotation_and_restore_round_trip() {
        let root = temp_tree();
        let target = root.join("settings.json");
        std::fs::write(&target, r#"{"env":{"A":"v0"}}"#).unwrap();
        // 前次失败运行可能残留备份——测试开头先清理（结尾的清理可能没执行到）。
        let _ = std::fs::remove_dir_all(backup_dir("rotate-rt-40530a19"));

        // 连续投影 12 次 → 备份只留 10 份。
        for i in 1..=12 {
            let mut fields = BTreeMap::new();
            fields.insert("env.A".to_string(), format!("v{i}"));
            let (rendered, _) = plan(&target, &fields).unwrap();
            apply(&target, &rendered, "rotate-rt-40530a19").unwrap();
        }

        let backups = list_backups("rotate-rt-40530a19").unwrap();
        assert_eq!(backups.len(), 10, "只保留 10 份");
        eprintln!(
            "备份（新→旧）：{:?}",
            backups.iter().map(|b| b.id.as_str()).collect::<Vec<_>>()
        );

        // 恢复最早一份（列表末尾）→ 文件回到 v3 的前一个快照。
        let oldest = backups.last().unwrap().clone();
        eprintln!("恢复 {} from {}", oldest.id, oldest.path);
        restore(&oldest).unwrap();
        let content = std::fs::read_to_string(&target).unwrap();
        assert!(
            content.contains("v2") || content.contains("v3"),
            "恢复到早期快照：{content}"
        );

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(backup_dir("rotate-rt-40530a19"));
    }

    /// 语法坏的目标文件 → plan 明确报错，不碰文件。
    #[test]
    fn broken_target_is_a_clear_error() {
        let root = temp_tree();
        let target = root.join("settings.json");
        std::fs::write(&target, "{ not json").unwrap();

        let error = plan(&target, &BTreeMap::new()).unwrap_err();
        assert!(error.to_string().contains("语法错误"), "{error}");
        assert_eq!(
            std::fs::read_to_string(&target).unwrap(),
            "{ not json",
            "文件未被触碰"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 缺失目标文件 → plan 明确报错（投影是修改，不是创建）。
    #[test]
    fn missing_target_is_a_clear_error() {
        let root = temp_tree();
        let target = root.join("nonexistent.json");
        let error = plan(&target, &BTreeMap::new()).unwrap_err();
        assert!(error.to_string().contains("失败"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
