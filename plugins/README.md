# plugins

Harness Hub 插件 SDK 与示例插件。v0.1 只冻结最小 Plugin API：两条通路
（用量导入、技能透传）+ 一个真实可用的 Host API（`host/log`）。

```text
plugins/
├── README.md                        本文件
├── sdk/
│   ├── protocol.md                  协议契约：传输、握手、方法、错误码、红线
│   └── manifest.schema.json         manifest 校验规则的 JSON Schema
└── examples/
    ├── usage-connect/               connect 型：把外部用量报告导入 Hub
    │   ├── manifest.toml
    │   ├── adapter/usage.py         stdlib-only，自带 stdio JSON-RPC 循环
    │   ├── fixtures/sample.json     ccusage unified session 形状的样例数据
    │   └── icon.svg
    └── hello-skill/                 skill 型：向会话提供 SKILL.md
        ├── manifest.toml
        ├── adapter/skill.py         stdlib-only，skill/list + skill/get
        ├── skills/demo/SKILL.md     一个真实可用的小技能（IPC 契约测试）
        └── icon.svg
```

先读 `sdk/protocol.md`，再挑一个示例改。两个示例都是**故意自包含**的：只用 Python 3
标准库，自带读循环，不 import `harness_hub_runtime`（插件进程不该依赖宿主内部包）。

## 两个示例各覆盖什么

|            | `usage-connect`                           | `hello-skill`                  |
| ---------- | ----------------------------------------- | ------------------------------ |
| 类型       | connect（connect 型 data source）         | skill（技能提供者）            |
| capability | `usage = true`                            | `skills = true`                |
| 实现的方法 | `usage/import`                            | `skill/list`、`skill/get`      |
| 返回值来源 | `fixtures/sample.json` 原样返回           | 从 `skills/*/SKILL.md` 读原文  |
| 演示范式   | 把外部工具的用量报告塞进 Hub 的现有解析器 | 把一份 Markdown 技能透传给会话 |

`usage-connect` 的 fixture 覆盖了三种真实形态：Codex 的 rollout 路径式 `period`、
Claude 的裸 UUID、ZCode 的 `sess_…`；还带了一个 `missingPricing: true` 的未定价模型行
（宿主会把它落成 `NULL` 金额，而不是 0 元——「没定价」和「免费」是两件事）。

`hello-skill` 的 SKILL.md 是自包含的：正文会被宿主原样透传给会话，会话不需要再读别的文件
就能按它干活。

## 手动跑一个插件

插件就是「读 stdin 一行、写 stdout 一行」的进程，不需要宿主也能手工验证：

```powershell
cd D:\HarnessHub

# 1) 喂一行 initialize，应当看到 result + initialized 通知两行
'{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"host_version":"0.1.0","api_version":"1","manifest_capabilities":{"usage":true}}}' |
  python plugins/examples/usage-connect/adapter/usage.py
```

预期输出（stdout 两行，stderr 一行启动日志）：

```json
{"jsonrpc": "2.0", "id": 1, "result": {"plugin_id": "usage-connect", "plugin_version": "0.1.0", "supported_methods": ["usage/import"]}}
{"jsonrpc": "2.0", "method": "initialized", "params": {}}
```

```powershell
# 2) 一次喂多行：握手 + 调方法。把请求写进临时文件（别提交进仓库）
@'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"host_version":"0.1.0","api_version":"1","manifest_capabilities":{"skills":true}}}
{"jsonrpc":"2.0","id":2,"method":"skill/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"skill/get","params":{"skill_id":"demo"}}
'@ | Set-Content -Encoding utf8 D:\tmp\dsh-temp\plugin-probe.jsonl

Get-Content D:\tmp\dsh-temp\plugin-probe.jsonl | python plugins/examples/hello-skill/adapter/skill.py
```

```powershell
# 3) 直接看 fixture 能不能通过宿主的算术校验（四类 token 与明细一致）
python -c "import json;d=json.load(open(r'plugins/examples/usage-connect/fixtures/sample.json',encoding='utf-8'));print([(r['agent'],r['totalTokens']) for r in d['session']])"
```

注意：`python` 必须在 PATH 上（manifest 里写的就是 `python`，不走 shell、不做路径展开）。
插件的工作目录由宿主固定为插件目录，所以手工跑时请先 `cd` 到仓库根或插件目录。

## 环境变量

| 变量                      | 作用                                                                        |
| ------------------------- | --------------------------------------------------------------------------- |
| `HARNESS_HUB_PLUGINS_DIR` | 覆盖插件扫描根。缺省是 `<app_data_dir>/plugins`；设了就用它，不再看默认目录 |

扫描规则：接受含 `manifest.toml` 的一级或二级子目录；**一个插件坏掉不影响其他插件**
（它会被单独标记为 `degraded`，主程序照常运行）。

## 边界（ADR-0018）

插件的隔离是**故障隔离**，不是安全沙箱。两条红线：

1. **插件默认 out-of-process，通过行分隔 JSON-RPC 2.0 over stdio 与宿主通信。**
   目的不是防恶意插件，而是让插件的 panic / 死锁 / 栈溢出无法拖垮主程序
   （R14「插件故障不得拖垮主程序」）。插件挂掉是一个可观测事实：标记 `degraded`，
   需要时再懒重启，绝不 panic 宿主。

2. **Host API 是唯一的跨边界能力面。**
   插件进程**拿不到数据库句柄，也拿不到 Secret Store 句柄**；Host API 的 7 个方法就是
   全部能力面。v0.1 只实现了 `host/log`，其余 6 个返回 `-32003 not_implemented`。

由此推出的具体禁止项：

- ❌ 插件**不得**直连主 SQLite（也不得自己去打开 `%APPDATA%/harness-hub/*.sqlite`）。
- ❌ 插件**不得**读取 Secret Store（钥匙串、凭据文件都算）。
- ❌ **不得把 secret 原文放进 Host API 参数**。`secret.resolve_ref` 的语义是「传引用，
  宿主自己解析」；审计表会记录参数（超过 512 字符截断），写进去就等于落盘。
- ❌ 不得绕过 Permission Engine、不得执行 manifest 未声明的外部程序。

还有一条容易被误读的：v0.1 的权限是**只观察不拦截**——每次 Host API 调用都会写一行
审计记录，但 `decision` 恒为 `observed`，没有真正拒绝。所以「现在能跑通」不等于
「被允许」，请不要把观察模式当成安全许可。

详见 `docs/adr/0018-plugin-isolation-boundary.md`。
