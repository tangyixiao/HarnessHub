# fixtures

外部 Harness 的**脱敏**日志 / 输出样本，用于 adapter 的 golden fixture 回归。

```text
codex/         Codex CLI
claude/        Claude Code
gemini/        Gemini CLI
```

规则：

1. 只放脱敏样本，禁止提交真实 Prompt、代码、路径中的用户名、任何 Token / Key。
2. 新增 Harness Adapter 必须同时提交 fixture 与 contract test（ADR-0022）。
3. fixture 变更视为 adapter 行为变更，必须在 PR 中说明差异来源（例如上游日志格式升级，风险 R1）。

当前目录为空：Phase 1 的 ccusage JSON 导入完成后开始收集 Codex 样本。
