# 多工具互通：先复用已有底座

日期：2026-10-23。研究对象包括“打通操作本身”，不限定 DeepSeek、ZCode、WorkBuddy 三种客户端。
这是公开源码与官方文档的可行性研究；未安装上游、未运行模型任务、未读取用户凭证或会话。
“存在实现”与“在 HarnessHub 已通过接入验收”分开记录，本文不宣称后者。

## 结论

根计划 §4.12–4.16 已要求参考/复用 CC Switch、保留配置 ownership、备份与恢复。
应落实这一选择：先组合现成配置工具和通用 Agent 协议客户端，HarnessHub 负责已有项目、启动、Session/Usage 索引及少量适配。
配置同步、Agent 执行、历史读取、桌面现存会话挂接是不同方向，不以一种“万能 Connector”覆盖。

优先路径：**CC Switch / CC Switch CLI 管理配置，acpx / 官方 ACP SDK 连接 Agent，官方客户端自身管理 Provider/MCP/插件。**
有公开接口就委托，缺稳定接口先贡献上游；复制代码排在后面，另造一套框架排在最后。

## 现成底座与代码证据

所有版本固定到下列提交，维护状态是本次查询快照，未来实现前重核版本、许可和发行产物。
许可证列是仓库许可文件的识别结果；实际复制前仍要核对具体文件、依赖与 NOTICE。

| 项目 / 固定提交                                                                                               | 许可、维护快照                           | 已有实现 / 优先复用方式                                                                          | 局限与取舍                                                                                                  |
| ------------------------------------------------------------------------------------------------------------- | ---------------------------------------- | ------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------- |
| [CC Switch](https://github.com/farion1231/cc-switch/tree/d8983bb10b82a1ccc15ae5f44ab070902623e990)            | MIT，未归档，10-23 有提交                | Provider/MCP/Skills/Prompt、原生配置投影、导入确认；先用外部应用深链、贡献上游格式支持           | GUI Tauri command 不是公共管理 API；深链发送成功不等于激活成功                                              |
| [CC Switch CLI](https://github.com/SaladDay/cc-switch-cli/tree/a285cd7d02fe893a72e9b47340f3893908bcabb6)      | MIT，未归档，10-22 更新，README 5.11.3   | 现成 Rust CLI fork，provider switch/start、MCP/Skills、session 与 backup；作为程序化配置联动候选 | provider list/current 不是稳定脱敏 JSON 契约；部分启动路径会迁移状态，不能默认只读；Windows daemon 能力有限 |
| [acpx](https://github.com/openclaw/acpx/tree/59ba80066f8932c8237aef0c46e13bbc4ae98fd2)                        | MIT，未归档，0.20.0，Node >=22.13        | ACP CLI、NDJSON、Agent registry、持久会话/取消、runtime 和共享会话 owner；优先验证外部 CLI 或库  | pre-1.0；固定版本，禁自动 npx 安装；不等于接管任意桌面已有会话                                              |
| [ACP Rust SDK](https://github.com/agentclientprotocol/rust-sdk/tree/305aefda6fbe88be6ae8b9088003e383f909b135) | Apache-2.0，未归档                       | 官方 client/agent runtime、schema、测试工具；acpx 不合适时的备选                                 | 当前 Rust 1.88 / edition 2024，与本仓声明 1.77 不同；升级需显式决策，不能静默加依赖                         |
| [conforme](https://github.com/maxgfr/conforme/tree/9d82431b1ad1468f87e892b2ba37837ae3c594b1)                  | MIT，未归档，10-20 更新，小型项目        | 已有多格式 MCP/Skills 同步，status/check/diff、sync --dry-run；可先调用既有 CLI                  | 不负责 Provider 或会话；默认清理/迁移有写入语义，只在隔离副本验证；不优先另建同步引擎                       |
| [AgentSwitch](https://github.com/RoyCoding8/AgentSwitch/tree/c6d8964b2f7f875259aa4e44d62497e9732579df)        | Apache-2.0，未归档，10-21 更新，小型项目 | Rust GUI 统一配置/聊天历史，源码有 config_store 和原子写入                                       | 未验证稳定 SDK/CLI，作为比较项，不优于 CC Switch 首选                                                       |
| [ccusage](https://github.com/ryoppippi/ccusage/tree/e42e1b44737fb1eedc200d1dd187426f0ee3013d)                 | 根许可 MIT，未归档                       | 已有 JSON 用量导入，本仓已经复用；继续走 UsageSourceAdapter                                      | 不因支持模型供应商就推断支持 ZCode/WorkBuddy 历史格式                                                       |
| [AgentAPI](https://github.com/coder/agentapi/tree/7c468d5b25ec9d3d76c41313cf4e08904ff5968d)                   | MIT，已归档，README 明确弃用             | 现成终端 Agent HTTP/SSE 封装可作历史比较                                                         | 不作为新主干依赖，不重新实现 TUI 文本猜消息方案                                                             |

## CC Switch：先核对操作的真实含义

[AppType](https://github.com/farion1231/cc-switch/blob/d8983bb10b82a1ccc15ae5f44ab070902623e990/src-tauri/src/app_config.rs)、
[deeplink parser](https://github.com/farion1231/cc-switch/blob/d8983bb10b82a1ccc15ae5f44ab070902623e990/src-tauri/src/deeplink/mod.rs)、
[provider 导入](https://github.com/farion1231/cc-switch/blob/d8983bb10b82a1ccc15ae5f44ab070902623e990/src-tauri/src/deeplink/provider.rs)、
[导入测试](https://github.com/farion1231/cc-switch/blob/d8983bb10b82a1ccc15ae5f44ab070902623e990/src-tauri/tests/deeplink_import.rs)
证明配置互通不是仅 README 声明。当前 AppType 没有 ZCode、WorkBuddy、CodeBuddy；支持 GLM Provider 不等于支持 ZCode 客户端配置。

`ccswitch://` 打开导入确认。切换式客户端默认仅添加候选；确认启用才 switch；累加式客户端写入配置也不等于切换当前 Provider。
Tauri 内部返回资源 ID，不是给 HarnessHub 的外部回执。本次没有核实到主仓稳定的外部 status/activate/readback API。
首个薄入口最多宣称“已请求打开确认”，不能显示“Profile 已激活”。不把 secret 放进 URI。

CLI fork 补足了可脚本化操作，但还要核对执行边界：

- [main.rs](https://github.com/SaladDay/cc-switch-cli/blob/a285cd7d02fe893a72e9b47340f3893908bcabb6/src-tauri/src/main.rs#L90) 的启动路径会按命令初始化/迁移；provider list 并非天然无写入。
- [provider.rs](https://github.com/SaladDay/cc-switch-cli/blob/a285cd7d02fe893a72e9b47340f3893908bcabb6/src-tauri/src/cli/commands/provider.rs#L675) 有 list/current/switch；
  [provider_inspect.rs](https://github.com/SaladDay/cc-switch-cli/blob/a285cd7d02fe893a72e9b47340f3893908bcabb6/src-tauri/src/cli/commands/provider_inspect.rs#L74) 的 list 是面向人的表格，current 会打印配置/密钥字段，不适合作为 Hub 自动采集输入。
- README 明确 `CC_SWITCH_CONFIG_DIR` 可隔离配置；`start` 的临时单次 Provider 与全局 switch 不同，`--shared-sessions` 及 daemon 有 Unix 限制。Windows 必须单独验收。

因此先在隔离目录验证现有 CLI，优先向上游增加脱敏机器可读状态/操作回执，不抓人类表格、不轮询或直写其 SQLite。
GUI 与 CLI fork 可能使用相同默认数据目录；未验证 schema/并发兼容前，不让二者同时管理同一份数据。

## Agent 互通：直接复用协议实现

acpx 的 [package exports](https://github.com/openclaw/acpx/blob/59ba80066f8932c8237aef0c46e13bbc4ae98fd2/package.json)、
[runtime](https://github.com/openclaw/acpx/blob/59ba80066f8932c8237aef0c46e13bbc4ae98fd2/src/runtime.ts)、
[shared runtime](https://github.com/openclaw/acpx/blob/59ba80066f8932c8237aef0c46e13bbc4ae98fd2/src/runtime/shared.ts)
已有查找/创建、事件、控制、取消和共享 owner。
[覆盖文档](https://github.com/openclaw/acpx/blob/59ba80066f8932c8237aef0c46e13bbc4ae98fd2/docs/2026-02-19-acp-coverage-roadmap.md) 说明按能力选择 session/resume 或 session/load。
普通 CLI 的重连失败可能创建新会话；共享 runtime 拒绝无损恢复失败，两种模式不能混用成功语义。

先用固定本地 executable 和假 ACP Agent 做 initialize/new/prompt/cancel/close/resume、坏包、超时和双客户端 owner 测试。
不默认执行 registry 中的 npx 下载。ACP schema/SDK 版本与 wire protocolVersion 分别核验，按 initialize 的能力协商。
共享 owner 与本仓 PTY reaper 的进程所有权必须明确，不能双方 kill 同一 Session；第三方 session store 保持只读。
现有插件宿主的 JSON-RPC handshake 不是 ACP，不能因为同为 stdio 就直接兼容。

## 用产品验证底座覆盖

| 对象                                                                                                                     | 当轮核实的接点                                                                                                                                                                                                                                                                                                                  | 首选路线与能力边界                                                                                                                 |
| ------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| [官方 DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness/tree/687f282e8625c6b8d11ff58211a90524be2e4317)   | MIT；[ACP server](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/acp/acp/src/index.ts)、[ACP 子代理 client](https://github.com/deepseek-ai/deepseek-harness/blob/687f282e8625c6b8d11ff58211a90524be2e4317/packages/subagent/subagent-acp/src/run.ts)、MCP client 已存在 | 用现成 ACP client 连官方 `dsh --profile acp`；新 runtime 不代表接管 Desktop 已开 Session；子代理 client 单次执行不代表完整共享会话 |
| [Dataelem DSH Desktop](https://github.com/dataelement/dsh-desktop/tree/67df6719fb1895139cd1ac595b2367b5642be67c)         | 独立社区 Electron，MIT；[runtime 启动/鉴权](https://github.com/dataelement/dsh-desktop/blob/67df6719fb1895139cd1ac595b2367b5642be67c/src/main/runtime/harness-runtime.ts)                                                                                                                                                       | 保留原管理界面，已有 runtime 的 Gateway/SessionControl 单独验证授权与 baseline/seq；`dsh-desktop://` 静态资源协议不是调度深链      |
| [Tauri DSH Desktop](https://github.com/dsh-tauri/deepseek-harness-desktop/tree/6169d3e81fd6fb51243c9f7d5dc7fddf230b248d) | 社区客户端；[LICENSE.details](https://github.com/dsh-tauri/deepseek-harness-desktop/blob/6169d3e81fd6fb51243c9f7d5dc7fddf230b248d/LICENSE.details) 含非商用等附加条件，不能按普通 MIT 看待                                                                                                                                      | 优先官方协议/外部应用；许可澄清前不复制其代码，不搬整个桌面壳                                                                      |
| [Z.ai ZCode](https://github.com/zai-org/ZCode/tree/3c5b227649371507ddc4eb46ac5133afe6db1cab)                             | 官方 Apache-2.0 源码，CLI [automation 文档](https://github.com/zai-org/ZCode/blob/3c5b227649371507ddc4eb46ac5133afe6db1cab/apps/zcode-cli/docs/usage/13-automation.md) 有 JSON/stream-json/resume 和隔离目录                                                                                                                    | 沿用官方 CLI 和既有 LaunchSpec；配置格式支持优先贡献 CC Switch，上游未支持就明确缺口，不复制 ZCode runtime                         |
| [腾讯 WorkBuddy](https://open.workbuddy.cn/docs/openapi)                                                                 | 官方开放平台含云任务、ACP、产物、本地助理消息；[连接器](https://open.workbuddy.cn/docs/connector) 支持 MCP/CLI+Skill                                                                                                                                                                                                            | 区分 WorkBuddy 调工具与外部发任务；API 经云端，不默认调用；桌面现存会话与 CodeBuddy CLI 是独立产品/能力                            |

第三方 [workbuddy-mcp](https://github.com/LinHaiJ/workbuddy-mcp/blob/9c8f3ef6d1b5cdb9575dbfef7f9ff51b672b3682/server.js) 实际 spawn CodeBuddy CLI，且默认跳权限、shell:true，不能按名称当作桌面控制器直接接入。
[workbuddy-switch](https://github.com/changexbc/workbuddy-switch/tree/22b318c1f44b335c2222e444abcc2d0f74e160f7) 涉及认证写入与 Session 同步，不符合外部 Session 只读边界，不采用这些通路。

## 研究限制与下一步

源码证据足以筛掉重复架构，但还不能决定一个依赖已可发布。
后续先做隔离的配置与 ACP 互操作试验，再为选定路线写执行 spec/plan/ADR；见
`docs/specs/2026-10-23-ecosystem-interoperability-direction.md`。
不将本研究当作改变云传输、Secret 存储或外部 Session 只读规则的授权。
