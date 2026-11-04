//! Skills 只读发现（ROADMAP E19 缩范围）：扫描 `~/.claude/skills/<name>/SKILL.md`，
//! 解析 frontmatter 的 name / description。
//!
//! **只读**：claude 自己就是 skills 的加载方（这是它的原生目录），Hub 只做
//! 发现与展示，不做复制投影（spec Non-goals）。文件缺失 / frontmatter 坏 →
//! 计入 skipped，绝不让单条坏文件毁掉整个列表。

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// 一个发现的 skill（只读快照）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSummary {
    /// 目录名（skills 目录下的 `<name>/`）。
    pub name: String,
    /// frontmatter 的 description（缺失为空串）。
    pub description: String,
    /// SKILL.md 绝对路径。
    pub path: String,
}

/// 一次扫描的报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillsReport {
    pub skills: Vec<SkillSummary>,
    /// 无法解析的条目（目录名；不崩纪律的观察面）。
    pub skipped: Vec<String>,
}

/// 极简 frontmatter 解析：取文件头部 `---` 围栏内的 `name:` / `description:` 行。
/// 不引入 YAML 依赖——SKILL.md 的 frontmatter 是平铺键值（真机形状），坏行忽略。
fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let mut name = None;
    let mut description = None;
    for line in lines {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("name:") {
            name = Some(value.trim().trim_matches('"').to_string());
        } else if let Some(value) = trimmed.strip_prefix("description:") {
            description = Some(value.trim().trim_matches('"').to_string());
        }
    }
    (name, description)
}

/// 扫描 `<home>/.claude/skills/`。目录不存在 → 空报告（未使用 skills 不是错误）。
pub fn discover(home: &std::path::Path) -> Result<SkillsReport> {
    let root = home.join(".claude").join("skills");
    let mut report = SkillsReport {
        skills: Vec::new(),
        skipped: Vec::new(),
    };
    if !root.is_dir() {
        return Ok(report);
    }
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(dir_name) = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
        else {
            continue;
        };
        let skill_md = path.join("SKILL.md");
        let Ok(content) = std::fs::read_to_string(&skill_md) else {
            report.skipped.push(dir_name);
            continue;
        };
        // 目录名就是 claude 的加载 id；frontmatter 只取 description。
        let (_name, description) = parse_frontmatter(&content);
        report.skills.push(SkillSummary {
            name: dir_name,
            description: description.unwrap_or_default(),
            path: skill_md.to_string_lossy().into_owned(),
        });
    }
    report.skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_home(tag: &str) -> std::path::PathBuf {
        let home = std::env::temp_dir().join(format!("hh-skills-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    /// 正常扫描：frontmatter 的 name/description；name 缺失用目录名兜底。
    #[test]
    fn discovers_skills_and_fills_defaults() {
        let home = temp_home("ok");
        let root = home.join(".claude").join("skills");
        std::fs::create_dir_all(root.join("pdf-tools")).unwrap();
        std::fs::create_dir_all(root.join("commit-helper")).unwrap();
        std::fs::create_dir_all(root.join("no-frontmatter")).unwrap();
        std::fs::write(
            root.join("pdf-tools/SKILL.md"),
            "---\nname: pdf-tools\ndescription: 处理 PDF 的工具\n---\n\n# 用法\n",
        )
        .unwrap();
        std::fs::write(
            root.join("commit-helper/SKILL.md"),
            "---\ndescription: 写提交信息\n---\n\n正文\n",
        )
        .unwrap();
        std::fs::write(root.join("no-frontmatter/SKILL.md"), "# 无 frontmatter\n").unwrap();

        let report = discover(&home).unwrap();
        assert_eq!(report.skipped.len(), 0);
        assert_eq!(
            report
                .skills
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            vec!["commit-helper", "no-frontmatter", "pdf-tools"],
            "按目录名升序"
        );
        let pdf = report
            .skills
            .iter()
            .find(|s| s.name == "pdf-tools")
            .unwrap();
        assert_eq!(pdf.description, "处理 PDF 的工具");
        let commit = report
            .skills
            .iter()
            .find(|s| s.name == "commit-helper")
            .unwrap();
        assert_eq!(commit.description, "写提交信息");
        // name 缺失 → 目录名兜底，不进 skipped。
        let bare = report
            .skills
            .iter()
            .find(|s| s.name == "no-frontmatter")
            .unwrap();
        assert_eq!(bare.description, "");

        let _ = std::fs::remove_dir_all(&home);
    }

    /// SKILL.md 缺失 / skills 目录不存在：容错（skipped / 空报告），不崩。
    #[test]
    fn tolerates_missing_files_and_missing_dir() {
        let home = temp_home("missing");
        let root = home.join(".claude").join("skills");
        std::fs::create_dir_all(root.join("empty-dir")).unwrap();

        let report = discover(&home).unwrap();
        assert_eq!(report.skills.len(), 0);
        assert_eq!(
            report.skipped,
            vec!["empty-dir"],
            "无 SKILL.md 计入 skipped"
        );

        let empty_home = temp_home("nodir");
        let report = discover(&empty_home).unwrap();
        assert_eq!(report.skills.len(), 0);
        assert!(report.skipped.is_empty());

        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&empty_home);
    }

    /// DTO 契约（AGENTS.md）。
    #[test]
    fn skill_dtos_serialize_with_camel_case_keys() {
        let skill = SkillSummary {
            name: "pdf-tools".to_string(),
            description: "d".to_string(),
            path: "C:/t/SKILL.md".to_string(),
        };
        let json = serde_json::to_value(&skill).unwrap();
        assert_eq!(json["name"], "pdf-tools");
        assert_eq!(json["description"], "d");
        assert_eq!(json["path"], "C:/t/SKILL.md");

        let report = SkillsReport {
            skills: vec![skill],
            skipped: vec!["x".to_string()],
        };
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["skipped"], serde_json::json!(["x"]));
    }
}
