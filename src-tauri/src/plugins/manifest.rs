//! 插件清单（`manifest.toml`）解析与校验——**测试先行**：先锁行为，再写实现。

pub const API_VERSION: &str = "1";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Capabilities {
    pub detect: bool,
    pub launch: bool,
    pub resume: bool,
    pub usage: bool,
    pub trace: bool,
    pub profiles: bool,
    pub mcp: bool,
    pub skills: bool,
}

impl Capabilities {
    /// 把方法名映射到它需要的 capability；未知方法返回 false。
    ///
    /// 这是授权的**唯一**来源：握手里插件的 `supported_methods` 只是自报，
    /// 必须再与本表求交集才算数（ADR-0018 决策 2）。
    /// 因此这里对未知方法/空串一律返回 false——拼错方法名等于没声明。
    pub fn allows(&self, method: &str) -> bool {
        match method {
            "harness/detect" => self.detect,
            "harness/launch" => self.launch,
            "harness/resume" => self.resume,
            "usage/import" => self.usage,
            "trace/query" => self.trace,
            "profile/list" => self.profiles,
            "mcp/servers" => self.mcp,
            "skill/list" | "skill/get" => self.skills,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub command: Vec<String>,
    pub capabilities: Capabilities,
}

/// 反序列化的中间形态：全部字段可选，这样才能自己给出**点名**的错误信息。
///
/// 直接 `toml::from_str::<Manifest>` 会让 `serde` 报「missing field `name`」，
/// 那样的信息既带不上文件路径，也没法在同一个地方统一 trim。
#[derive(serde::Deserialize)]
struct RawManifest {
    id: Option<String>,
    name: Option<String>,
    version: Option<String>,
    api_version: Option<String>,
    adapter: Option<RawAdapter>,
    capabilities: Option<toml::Value>,
}

#[derive(serde::Deserialize)]
struct RawAdapter {
    command: Option<Vec<String>>,
}

/// 合法的 capability 键全集。用显式列表而不是 `#[serde(deny_unknown_fields)]`，
/// 是为了在报错里**点名**那个键（「未知键 usages」比「unknown field」有用得多）。
const KNOWN_CAPABILITIES: [&str; 8] = [
    "detect", "launch", "resume", "usage", "trace", "profiles", "mcp", "skills",
];

fn manifest_error(detail: impl Into<String>) -> crate::error::Error {
    crate::error::Error::PluginManifest(detail.into())
}

/// 校验并规范化 `id`：`^[a-z][a-z0-9-]*$`。
///
/// id 同时是插件目录名、数据库主键和 IPC 里的句柄，所以不允许大小写变体
/// 与下划线——Windows 文件系统大小写不敏感，放过 `Foo` 迟早撞上 `foo`。
fn validate_id(raw: &str) -> crate::error::Result<String> {
    let id = raw.trim();
    if id.is_empty() {
        return Err(manifest_error("id 不能为空"));
    }

    let mut chars = id.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    let rest_ok = chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');

    if !first_ok || !rest_ok {
        return Err(manifest_error(format!(
            "id {id:?} 不合法：必须匹配 ^[a-z][a-z0-9-]*$"
        )));
    }

    Ok(id.to_string())
}

/// `name` / `version` 只有一条规则：trim 后非空。
fn require_text(field: &str, raw: Option<String>) -> crate::error::Result<String> {
    let value = raw.unwrap_or_default();
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(manifest_error(format!("{field} 不能为空")));
    }
    Ok(trimmed.to_string())
}

/// `[adapter].command` 是一个 argv 数组，不经过 shell。
///
/// 空数组、空串元素都要拒绝：前者 spawn 会 panic，后者会得到一个空参数，
/// 插件那边看到的 argv 与作者预期不同，错误还会推迟到运行时才暴露。
fn validate_command(raw: Option<Vec<String>>) -> crate::error::Result<Vec<String>> {
    let command = raw.ok_or_else(|| manifest_error("[adapter].command 不能缺失"))?;

    if command.is_empty() {
        return Err(manifest_error("[adapter].command 不能是空数组"));
    }

    command
        .into_iter()
        .enumerate()
        .map(|(index, element)| {
            let trimmed = element.trim();
            if trimmed.is_empty() {
                return Err(manifest_error(format!(
                    "[adapter].command 第 {index} 个元素 trim 后为空"
                )));
            }
            Ok(trimmed.to_string())
        })
        .collect()
}

/// 解析 `[capabilities]`：缺省全 false；未知键或非布尔值一律拒绝。
///
/// 为什么不静默忽略未知键：`usages = true` 这种拼写错误如果被忽略，
/// 插件会以「全部能力都没声明」的身份启动，然后在第一个 RPC 上莫名其妙被拒。
/// 声明即契约，写错就当场报错。
fn parse_capabilities(raw: Option<toml::Value>) -> crate::error::Result<Capabilities> {
    let Some(value) = raw else {
        return Ok(Capabilities::default());
    };

    let table = value
        .as_table()
        .ok_or_else(|| manifest_error("[capabilities] 必须是一个表"))?;

    let mut capabilities = Capabilities::default();

    for (key, value) in table {
        let flag = value.as_bool().ok_or_else(|| {
            manifest_error(format!("[capabilities].{key} 必须是布尔值，实际是 {value}"))
        })?;

        match key.as_str() {
            "detect" => capabilities.detect = flag,
            "launch" => capabilities.launch = flag,
            "resume" => capabilities.resume = flag,
            "usage" => capabilities.usage = flag,
            "trace" => capabilities.trace = flag,
            "profiles" => capabilities.profiles = flag,
            "mcp" => capabilities.mcp = flag,
            "skills" => capabilities.skills = flag,
            unknown => {
                return Err(manifest_error(format!(
                    "[capabilities] 含未知键 {unknown:?}（已知：{}）",
                    KNOWN_CAPABILITIES.join(", ")
                )))
            }
        }
    }

    Ok(capabilities)
}

pub fn parse(toml_text: &str) -> crate::error::Result<Manifest> {
    let raw: RawManifest = toml::from_str(toml_text)
        .map_err(|error| manifest_error(format!("TOML 解析失败：{error}")))?;

    let id = validate_id(&raw.id.unwrap_or_default())?;
    let name = require_text("name", raw.name)?;
    let version = require_text("version", raw.version)?;

    // api_version 缺失与「值不对」分开报：前者是漏写，后者是版本不兼容。
    let api_version = require_text("api_version", raw.api_version)?;
    if api_version != API_VERSION {
        return Err(manifest_error(format!(
            "api_version 不兼容：插件声明 {api_version:?}，宿主只支持 {API_VERSION:?}"
        )));
    }

    let command = validate_command(raw.adapter.and_then(|adapter| adapter.command))?;

    Ok(Manifest {
        id,
        name,
        version,
        api_version,
        command,
        capabilities: parse_capabilities(raw.capabilities)?,
    })
}

/// 从插件目录读取 `manifest.toml`。
///
/// 读失败直接透传 [`crate::error::Error::Io`]：扫描目录时大量子目录根本不是插件，
/// 调用方要靠 `Io` 与 `PluginManifest` 区分「跳过」和「报告损坏」。
pub fn load(plugin_dir: &std::path::Path) -> crate::error::Result<Manifest> {
    let text = std::fs::read_to_string(plugin_dir.join("manifest.toml"))?;
    parse(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use std::path::PathBuf;

    /// 一份各字段都合法的清单；每个测试只改它关心的那一处。
    fn valid() -> &'static str {
        r#"
id = "usage-connect"
name = "Usage Connect"
version = "0.1.0"
api_version = "1"

[adapter]
command = ["python", "adapter/usage.py"]

[capabilities]
usage = true
"#
    }

    /// 把清单写进一个按 tag 隔离的临时目录，返回目录路径。
    ///
    /// 不用 `tempfile`：仓库没有这个 dev-dependency，而这里只需要一个
    /// 「进程内唯一 + 可清理」的目录。tag 让并行测试互不干扰。
    fn temp_plugin_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hh-manifest-{}-{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("临时目录");
        dir
    }

    // ---------- 正常路径 ----------

    #[test]
    fn parses_every_field() {
        let manifest = parse(valid()).expect("合法清单必须解析成功");

        assert_eq!(manifest.id, "usage-connect");
        assert_eq!(manifest.name, "Usage Connect");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.api_version, API_VERSION);
        assert_eq!(
            manifest.command,
            vec!["python".to_string(), "adapter/usage.py".to_string()]
        );
        assert!(manifest.capabilities.usage);
        // 未声明的能力必须是 false，不能因为「没写」就当成 true。
        assert!(!manifest.capabilities.detect);
        assert!(!manifest.capabilities.skills);
    }

    #[test]
    fn missing_capabilities_section_is_all_false() {
        let text = r#"
id = "hello-skill"
name = "Hello Skill"
version = "0.1.0"
api_version = "1"

[adapter]
command = ["python", "adapter/skill.py"]
"#;
        let manifest = parse(text).expect("缺省 [capabilities] 是合法的");

        assert_eq!(manifest.capabilities, Capabilities::default());
        assert!(!manifest.capabilities.allows("usage/import"));
        assert!(!manifest.capabilities.allows("skill/list"));
    }

    /// capability 名字与协议方法的映射是**唯一**的授权来源：
    /// supervisor 的握手交集只信这里（插件自报 supported_methods 不算数）。
    #[test]
    fn capabilities_map_methods_to_their_owner() {
        let all = Capabilities {
            detect: true,
            launch: true,
            resume: true,
            usage: true,
            trace: true,
            profiles: true,
            mcp: true,
            skills: true,
        };

        for method in [
            "harness/detect",
            "harness/launch",
            "harness/resume",
            "usage/import",
            "trace/query",
            "profile/list",
            "mcp/servers",
            "skill/list",
            "skill/get",
        ] {
            assert!(all.allows(method), "{method} 应被允许");
        }

        // 只声明 usage 时，别的方法一律不放行。
        let only_usage = Capabilities {
            usage: true,
            ..Capabilities::default()
        };
        assert!(only_usage.allows("usage/import"));
        assert!(!only_usage.allows("skill/list"));
        assert!(!only_usage.allows("harness/detect"));

        // 未知方法 = 没声明：拼错不能静默放行。
        assert!(!all.allows("usage/importt"));
        assert!(!all.allows(""));
        assert!(!all.allows("Usage/Import"));
    }

    #[test]
    fn unknown_top_level_key_is_tolerated() {
        // 向前兼容只允许「新增可选字段」：旧宿主遇到新字段必须还能读。
        // 注意必须写在任何 `[table]` 之前——追加到文件末尾会被算进
        // `[capabilities]` 表，那是「未知 capability 键」，本来就该拒绝。
        let text = format!(
            "icon = \"icon.svg\"\nhomepage = \"https://example.invalid\"\n{}",
            valid()
        );
        let manifest = parse(&text).expect("未知顶层键必须被忽略，不是报错");

        assert_eq!(manifest.id, "usage-connect");
    }

    #[test]
    fn surrounding_whitespace_is_trimmed() {
        let text = valid()
            .replace("\"Usage Connect\"", "\"  Usage Connect  \"")
            .replace("\"0.1.0\"", "\" 0.1.0 \"")
            .replace("\"python\"", "\" python \"");
        let manifest = parse(&text).expect("可 trim 的空白不该让清单失效");

        assert_eq!(manifest.name, "Usage Connect");
        assert_eq!(manifest.version, "0.1.0");
        assert_eq!(manifest.command[0], "python", "argv[0] 带空格会 spawn 失败");
    }

    // ---------- id 规范 ----------

    #[test]
    fn invalid_ids_are_rejected() {
        for id in [
            "",              // 空串
            "Usage_Connect", // 大写 + 下划线
            "usage_connect", // 下划线
            "-leading",      // 以连字符开头
            "9lives",        // 不以字母开头
            "with space",
            "usage/import", // 斜杠会被当成路径分隔符
        ] {
            let text = valid().replace("usage-connect", id);
            let error = parse(&text).expect_err(&format!("id {id:?} 必须被拒绝"));

            assert!(
                matches!(error, Error::PluginManifest(_)),
                "id {id:?} 必须报 PluginManifest，实际：{error:?}"
            );
        }
    }

    /// `^[a-z][a-z0-9-]*$` 只管首字符是字母，**不禁止**结尾连字符。
    ///
    /// 这里刻意把这条边界写成正向用例：把 `trailing-` 当非法会让
    /// 实现悄悄偏离规格，将来与插件作者的校验器对不上。
    #[test]
    fn trailing_hyphen_is_allowed_by_the_literal_regex() {
        let text = valid().replace("usage-connect", "trailing-");
        let manifest = parse(&text).expect("规格的正则并不禁止结尾连字符");

        assert_eq!(manifest.id, "trailing-");
    }

    #[test]
    fn valid_ids_are_accepted() {
        for id in ["a", "usage-connect", "a1", "plugin-2-x"] {
            let text = valid().replace("usage-connect", id);
            let manifest =
                parse(&text).unwrap_or_else(|error| panic!("id {id:?} 应合法：{error:?}"));
            assert_eq!(manifest.id, id);
        }
    }

    // ---------- api_version ----------

    #[test]
    fn api_version_must_be_one() {
        let text = valid().replace("api_version = \"1\"", "api_version = \"2\"");
        let error = parse(&text).expect_err("宿主只接受 api_version = \"1\"");

        assert!(matches!(error, Error::PluginManifest(_)), "{error:?}");
        let message = error.to_string();
        assert!(
            message.contains('2'),
            "错误信息必须带上插件给的值：{message}"
        );
    }

    #[test]
    fn missing_api_version_is_rejected() {
        let text = valid().replace("api_version = \"1\"\n", "");
        let error = parse(&text).expect_err("api_version 是必填");

        assert!(matches!(error, Error::PluginManifest(_)), "{error:?}");
        assert!(error.to_string().contains("api_version"), "{error}");
    }

    // ---------- name / version ----------

    #[test]
    fn blank_name_or_version_is_rejected() {
        for (field, broken) in [
            ("name", valid().replace("\"Usage Connect\"", "\"   \"")),
            ("name", valid().replace("name = \"Usage Connect\"\n", "")),
            ("version", valid().replace("\"0.1.0\"", "\" \t \"")),
            ("version", valid().replace("version = \"0.1.0\"\n", "")),
        ] {
            let error = parse(&broken).expect_err(&format!("{field} 空白/缺失必须被拒绝"));
            assert!(matches!(error, Error::PluginManifest(_)), "{error:?}");
            assert!(
                error.to_string().contains(field),
                "错误信息应点名 {field}：{error}"
            );
        }
    }

    // ---------- [adapter].command ----------

    #[test]
    fn missing_adapter_command_is_rejected() {
        let text = valid().replace("command = [\"python\", \"adapter/usage.py\"]\n", "");
        let error = parse(&text).expect_err("[adapter].command 是必需字段");

        assert!(matches!(error, Error::PluginManifest(_)), "{error:?}");
        assert!(error.to_string().contains("command"), "{error}");
    }

    #[test]
    fn empty_command_array_is_rejected() {
        let text = valid().replace("[\"python\", \"adapter/usage.py\"]", "[]");
        let error = parse(&text).expect_err("空数组没有可执行的程序");

        assert!(matches!(error, Error::PluginManifest(_)), "{error:?}");
    }

    #[test]
    fn blank_command_element_is_rejected() {
        for broken in [
            valid().replace("\"adapter/usage.py\"", "\"\""),
            valid().replace("\"adapter/usage.py\"", "\"   \""),
            valid().replace("\"python\"", "\"\""),
            // 只有程序、没有参数：合法（等于无参启动），不该被拒绝。
        ] {
            let error = parse(&broken).expect_err("command 元素 trim 后不能为空");
            assert!(matches!(error, Error::PluginManifest(_)), "{error:?}");
        }

        let program_only = valid().replace("[\"python\", \"adapter/usage.py\"]", "[\"python\"]");
        assert_eq!(
            parse(&program_only).expect("单元素 command 合法").command,
            vec!["python".to_string()]
        );
    }

    // ---------- [capabilities] 严格性 ----------

    #[test]
    fn unknown_capability_key_is_rejected() {
        for broken in [
            valid().replace("usage = true", "usages = true"), // 拼错
            valid().replace("usage = true", "usage = \"true\""), // 非布尔
            valid().replace("usage = true", "usage = 1"),     // 非布尔
        ] {
            let error = parse(&broken).expect_err("未知 capability 键或非布尔值都必须拒绝");
            assert!(matches!(error, Error::PluginManifest(_)), "{error:?}");
        }

        // 拼错的那个键必须被点出来，否则用户只能干瞪眼。
        let typo = valid().replace("usage = true", "usages = true");
        assert!(
            parse(&typo).unwrap_err().to_string().contains("usages"),
            "错误信息应点名未知键"
        );
    }

    // ---------- 结构错误 ----------

    #[test]
    fn malformed_toml_is_a_manifest_error() {
        let error = parse("id = ").expect_err("坏 TOML 不能 panic");

        assert!(matches!(error, Error::PluginManifest(_)), "{error:?}");
    }

    // ---------- load() ----------

    #[test]
    fn load_reads_manifest_toml_from_the_plugin_dir() {
        let dir = temp_plugin_dir("ok");
        std::fs::write(dir.join("manifest.toml"), valid()).expect("写清单");

        let manifest = load(&dir).expect("从目录读取清单");
        assert_eq!(manifest.id, "usage-connect");
    }

    #[test]
    fn load_reports_a_broken_manifest_with_plugin_manifest_error() {
        let dir = temp_plugin_dir("broken");
        std::fs::write(dir.join("manifest.toml"), "id = ").expect("写清单");

        let error = load(&dir).expect_err("坏清单不能静默");
        assert!(matches!(error, Error::PluginManifest(_)), "{error:?}");
    }

    /// 缺 `manifest.toml` 是**IO**错误而不是清单错误：
    /// 「目录里没有清单」与「清单写错了」对调用方意味着完全不同的处置
    /// （前者是「这不是插件」，后者是「这个插件坏了」）。
    #[test]
    fn load_on_a_dir_without_manifest_is_an_io_error() {
        let dir = temp_plugin_dir("missing");

        let error = load(&dir).expect_err("没有清单必须报错");
        assert!(matches!(error, Error::Io(_)), "{error:?}");
    }
}
