# OpenCode CLI detection fixture

2026-10-12 本机执行 `opencode --version`，stdout 为 `version.txt`（`1.18.23`），
退出码 0。这是 CLI 版本取证，不是模型或 TUI 调用。

`summary.json` 为手工构造的 IPC 契约 fixture：版本取自上述真实输出，binary 使用
无用户信息的路径，dataPaths 为四个数据目录都不存在时的空数组（数据目录发现与
XDG 覆盖行为由 `opencode.rs` 的 `FakeHostProbe` 单测覆盖）。Rust 与 TS 共同读取
它，锁定 camelCase 键名与全 false 能力。

`opencode debug paths` 的默认数据根（2026-10-12 实测）：

```text
data    ~/.local/share/opencode
config  ~/.config/opencode
state   ~/.local/state/opencode
cache   ~/.cache/opencode
```

四个根分别可被 `XDG_DATA_HOME` / `XDG_CONFIG_HOME` / `XDG_STATE_HOME` /
`XDG_CACHE_HOME` 覆盖（实测有效）。未包含任何用户配置、认证、Prompt、Response
或 Session 内容。
