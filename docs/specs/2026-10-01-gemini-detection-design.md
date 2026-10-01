# Gemini CLI 检测与清单接入

## 目的与范围

按主规格 §20，Claude 之后接入 Gemini CLI。本次交付仅为可独立验证的
检测阶段：Harnesses 页面展示安装、版本、现存数据目录；生产冷启动同步第三个
Harness。检测成功不等于完整支持，九项 capability 均为 false。

## 设计

- `gemini_cli.rs` 实现现有 `HarnessAdapter`，复用 `HostProbe`，不新增探测抽象。
- 身份 `gemini`、展示名 `Gemini CLI`、PATH 可执行名 `gemini`。
- `installed` 仅取决于 binary；`--version` 失败时版本未知，安装仍然可见。
- 默认只发现 home 下 `.gemini` 目录。组合根读取非空 `GEMINI_CLI_HOME` 并作为
  配置根注入；覆盖根下再拼 `.gemini`，不回退为另一用户目录。
- 不读取配置、认证、Session、日志正文；不发模型请求、不写外部 Harness 数据。
- `build_launch_spec` 返回明确的未实现错误；Terminal 选择器只显示同时具备
  `launch`、`terminal` 的已安装适配器，过滤规则不包含 Harness 名字。
- 注册顺序为 Codex、Claude Code、Gemini CLI。复用现有 reconcile、表结构、IPC
  `HarnessSummary` 及 TS normalize；无需 migration 或新 DTO。

## 验收

1. binary 缺失、版本未知、默认/覆盖目录存在与缺失均有确定性测试。
2. Gemini JSON fixture 同时被 Rust serialization contract 和 TS decoder 测试消费。
3. 真正空库只通过生产引导与注册表同步，得到三条定义与三条安装；重复同步幂等，
   保留原 Codex 首次发现时间；现有 Codex/Claude capability 契约不变。
4. Harnesses 显示 Gemini 的安装信息与全 false 能力；Terminal 不提供该启动选项。
5. 真机只验证 PATH 与 `--version`；未安装明确 SKIP，不能作为真机通过证据。
6. 完成 AGENTS.md 全部门禁并记录真实结果。

## 来源与限制

- [官方配置说明](https://github.com/google-gemini/gemini-cli/blob/main/docs/reference/configuration.md)：
  `GEMINI_CLI_HOME` 是配置根，CLI 在其下使用 `.gemini`。
- [官方 CLI 参考](https://geminicli.com/docs/cli/cli-reference/)：命令入口 `gemini`。
- 2026-10-01 本机 `gemini.cmd --version` 输出 `0.59.0`（退出码 0）。
- 后续 Terminal 生命周期、GUI、Usage 必须另立计划与验收；本次不扩展能力声明。
