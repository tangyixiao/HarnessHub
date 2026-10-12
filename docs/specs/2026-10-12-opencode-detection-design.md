# OpenCode CLI 检测接入

## 目的与范围

按主规格 §20 第 13 步接入第五个 Harness：**OpenCode**（sst/opencode，本机为
Scoop 安装的原生 `opencode.exe`）。本次交付仅为可独立验证的**检测阶段**
（与 codebuddy-detection 同构）：Harnesses 页展示安装、版本、现存数据目录；
生产冷启动同步第五个 Harness。九项 capability 均为 false，Terminal 不提供
启动入口（能力过滤自动生效，前端零改动）。

launch / terminal / usage 需要各自的真机证据（PTY spike、生命周期、Usage 对账），
另立计划；本次不扩展能力声明（ADR-0005 零宣称）。

## 本机与官方事实（2026-10-12 取证）

```text
安装形态   D:\Scoop\shims\opencode.exe（136192 B 原生 exe + opencode.shim）
           —— 与 codex/claude/gemini/codebuddy 的 npm shim 三件套**不同**：
           本机没有 opencode.cmd / opencode.ps1
--version  1.18.23（stdout 纯数字串，退出码 0），现有 parse_version 可直接解析
opencode debug paths（默认，退出码 0）：
  home    C:\Users\tangy
  data    C:\Users\tangy\.local\share\opencode
  bin     C:\Users\tangy\.cache\opencode\bin
  log     <data>\log
  repos   <data>\repos
  cache   C:\Users\tangy\.cache\opencode
  config  C:\Users\tangy\.config\opencode
  state   C:\Users\tangy\.local\state\opencode
  tmp     C:\Users\tangy\AppData\Local\Temp\opencode
```

**XDG 根覆盖实测有效**（本机设 `XDG_DATA_HOME` / `XDG_CONFIG_HOME` /
`XDG_STATE_HOME` / `XDG_CACHE_HOME` 后重跑 `debug paths`，退出码 0）：

```text
data    $XDG_DATA_HOME\opencode      cache  $XDG_CACHE_HOME\opencode
bin     $XDG_CACHE_HOME\opencode\bin config $XDG_CONFIG_HOME\opencode
log     <data>\log                   state  $XDG_STATE_HOME\opencode
repos   <data>\repos
```

清空变量后 `debug paths` 恢复默认值，说明这是真实的运行时解析，不是一次性缓存。
四个变量**互相独立**（只设其中一个只影响对应的根）。

本机四个目录均存在：`~\.local\share\opencode`（内含 `opencode.db` 3.9 MB 与
`log/` `repos/` `snapshot/` `tool-output/`）、`~\.config\opencode`
（`opencode.json` 等）、`~\.local\state\opencode`、`~\.cache\opencode`。
本机当前**未设置**任何 `XDG*` 变量。

## 设计

- `opencode.rs` 实现现有 `HarnessAdapter`，复用 `HostProbe`，不新增探测抽象。
- 身份 `opencode`、展示名 `OpenCode`、PATH 可执行名 `opencode`。
- `installed` 仅取决于 binary；`--version` 失败时版本未知，安装仍然可见。
- **数据目录按 OpenCode 自己的规则解析四个 XDG 根**，只列出确实存在的目录，
  顺序固定为 data、config、state、cache：

  | 根     | 默认                           | 覆盖变量          |
  | ------ | ------------------------------ | ----------------- |
  | data   | `<home>/.local/share/opencode` | `XDG_DATA_HOME`   |
  | config | `<home>/.config/opencode`      | `XDG_CONFIG_HOME` |
  | state  | `<home>/.local/state/opencode` | `XDG_STATE_HOME`  |
  | cache  | `<home>/.cache/opencode`       | `XDG_CACHE_HOME`  |

  Windows 上同样是这套 Unix 风格默认值 —— 这不是猜测，是本机
  `opencode debug paths` 的实测输出。

- **不 shell out 到 `opencode debug paths`**：`HostProbe` 只承诺
  `--version`，多跑一个子命令等于新增探测抽象；而 XDG 解析规则已实测可复现，
  可以在测试里离线锁定。
- 环境变量只在组合根 `lib.rs` 读取一次，适配器只接受已解析的
  `OpenCodeXdgRoots`（与 Gemini 的 `GEMINI_CLI_HOME` 注入同构），
  测试因此不依赖真实环境。
- 不读取配置、认证、Session、日志正文；不发模型请求、不写外部 Harness 数据。
- `build_launch_spec` 返回明确的未实现错误；Terminal 选择器按能力过滤自动
  不显示（规则不含 Harness 名字）。
- 注册顺序 Codex、Claude Code、Gemini CLI、CodeBuddy、OpenCode。复用现有
  reconcile、表结构、IPC `HarnessSummary` 及 TS normalize；无需 migration 或新 DTO。
- 身份色：`harness-colors.ts` 已含 `opencode: 130`（草绿，与既有六色互异），
  **无需改动**。

## 验收

1. binary 缺失、版本未知、四个根各自存在与缺失、home 缺失、XDG 覆盖存在时
   **不回退默认**，均有确定性测试。
2. OpenCode fixture（`version.txt` + `summary.json`）同时被 Rust serialization
   contract 和 TS decoder 测试消费。
3. 真正空库只通过生产引导与注册表同步，得到五条定义与五条安装；重复同步
   幂等；现有 Codex/Claude/Gemini/CodeBuddy 契约不变。
4. Harnesses 显示 OpenCode 的安装信息（1.18.23 与现存数据目录）与全 false
   能力矩阵；Terminal 不提供启动入口。
5. 真机只验证 PATH 与 `--version`，并断言报告的每个数据目录**真实存在**；
   未安装明确 SKIP，不能作为真机通过证据。
6. 完成 AGENTS.md 全部门禁并记录真实结果。

## 来源与限制

- `opencode --version` / `opencode debug paths` / `opencode --help` 均为
  2026-10-12 本机实测（1.18.23）。
- OpenCode 遵循 XDG Base Directory 规范；本机 Windows 上的默认根即
  `~/.local/share` 等 Unix 风格路径。
- 后续 Terminal 生命周期与 Usage 必须另立计划与验收；本次不扩展能力声明。
