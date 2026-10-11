# CodeBuddy CLI 检测接入

## 目的与范围

按主规格的多 Harness 路线接入第四个 Harness：腾讯 **CodeBuddy Code** CLI。
本次交付仅为可独立验证的**检测阶段**（与 gemini-detection 同构）：Harnesses 页
展示安装、版本、现存数据目录；生产冷启动同步第四个 Harness；身份色进入
Dashboard / Sessions / Harnesses。检测成功不等于完整支持，九项 capability 均为
false，Terminal 不提供启动入口（能力过滤自动生效，前端零改动）。

## 本机与官方事实（2026-10-11 取证）

```text
安装包     @tencent-ai/codebuddy-code@2.161.0（npm 全局，Node.js ≥ 18）
可执行名   codebuddy（PATH shim：.cmd / .ps1 / 无扩展名三件套，另有别名 codebuddy-code、
           codebuddy-lowmem 与短别名 cbc）
--version  2.161.0（退出码 0）
数据目录   %USERPROFILE%\.codebuddy\（本机存在：sessions/ projects/ logs/
           settings.json 等）
启动语义   默认交互 TUI；-p/--print 非交互（官方 --help 实测）
```

## 设计

- `codebuddy.rs` 实现现有 `HarnessAdapter`，复用 `HostProbe`，不新增探测抽象。
- 身份 `codebuddy`、展示名 `CodeBuddy`、PATH 可执行名 `codebuddy`。
- `installed` 仅取决于 binary；`--version` 失败时版本未知，安装仍然可见。
- 数据目录只发现 home 下 `.codebuddy`；未证实官方支持 home 覆盖 env，
  **不发明**（与 Gemini 的 `GEMINI_CLI_HOME` 区别对待）。
- 不读取配置、认证、Session、日志正文；不发模型请求、不写外部 Harness 数据。
- `build_launch_spec` 返回明确的未实现错误；Terminal 选择器按能力过滤自动
  不显示（规则不含 Harness 名字）。
- 注册顺序 Codex、Claude Code、Gemini CLI、CodeBuddy。复用现有 reconcile、
  表结构、IPC `HarnessSummary` 及 TS normalize；无需 migration 或新 DTO。
- 身份色：`harness-colors.ts` 增加 `codebuddy: 190`（青蓝，与 codex 220 /
  zcode 160 错开），测试同步。

## 验收

1. binary 缺失、版本未知、默认目录存在与缺失均有确定性测试。
2. CodeBuddy JSON fixture 同时被 Rust serialization contract 和 TS decoder
   测试消费。
3. 真正空库只通过生产引导与注册表同步，得到四条定义与四条安装；重复同步
   幂等；现有 Codex/Claude/Gemini 契约不变。
4. Harnesses 显示 CodeBuddy 的安装信息与全 false 能力；Terminal 不提供启动入口。
5. 真机只验证 PATH 与 `--version`；未安装明确 SKIP，不能作为真机通过证据。
6. 完成 AGENTS.md 全部门禁并记录真实结果。

## 来源与限制

- [npm 包 @tencent-ai/codebuddy-code](https://www.npmjs.com/package/@tencent-ai/codebuddy-code)；
  官方文档 https://copilot.tencent.com 。
- 2026-10-11 本机 `codebuddy --version` 输出 `2.161.0`；shim 形态与 Codex /
  Claude / Gemini 完全同构。
- 后续 Terminal 生命周期、Usage 必须另立计划与验收；本次不扩展能力声明。
