# fixtures

外部 Harness 的**脱敏**日志 / 输出样本，用于 adapter 的 golden fixture 回归。

```text
ccusage/       ccusage JSON 报告（Usage 导入的 golden 样本）
codex/         Codex CLI
claude/        Claude Code
gemini/        Gemini CLI
codebuddy/     CodeBuddy CLI
opencode/      OpenCode CLI
```

规则：

1. 只放脱敏样本，禁止提交真实 Prompt、代码、路径中的用户名、任何 Token / Key。
2. 新增 Harness Adapter 必须同时提交 fixture 与 contract test（ADR-0022）。
3. fixture 变更视为 adapter 行为变更，必须在 PR 中说明差异来源（例如上游日志格式升级，风险 R1）。

各 Harness 目录当前存放的是**检测阶段**夹具：`version.txt` 为真实 `--version`
输出，`summary.json` 为 Rust 与 TS 共用的 IPC 序列化契约（键名、camelCase、
能力默认值）。Usage 与生命周期夹具在其能力真正验收时再补。
