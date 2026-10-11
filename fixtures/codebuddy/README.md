# CodeBuddy CLI detection fixture

2026-10-11 本机执行 `codebuddy --version`，stdout 为 `version.txt`（`2.161.0`），
退出码 0。这是 CLI 版本取证，不是模型或 TUI 调用。

`summary.json` 为手工构造的 IPC 契约 fixture：版本取自上述真实输出，binary 使用
无用户信息的路径，dataPaths 为目录不存在时的空数组（数据目录发现行为由
`codebuddy.rs` 的 `FakeHostProbe` 单测覆盖）。Rust 与 TS 共同读取它，
锁定 camelCase 键名与全 false 能力。
未包含任何用户配置、认证、Prompt、Response 或 Session 内容。
