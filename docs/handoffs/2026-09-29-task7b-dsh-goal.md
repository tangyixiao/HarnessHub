# DSH Goal 交接：Task 7B Claude GUI 验收收口

> 用法：在 DSH 中以 Goal 模式运行时，将下方「Goal 初始指令」作为目标，并把本文件作为任务上下文。Goal 应持续工作到达成验收条件或遇到明确阻塞，不要把整份文档当成一次性总结任务。

## Goal 初始指令

请在 `D:\HarnessHub` 的现有分支上，独立审阅并收口 Task 7B（Claude Code Terminal）的当前源码 GUI 验收证据。以仓库代码、计划、ADR 和已记录的原始验收结果为准；先核对事实，再决定是否需要修改。只修复能够复现或由仓库内容直接证明的缺陷/文档矛盾。最终给出逐项证据、实际执行的验证命令与输出、改动文件和提交状态，并停在可供用户审阅的本地分支上。

成功条件：

1. 确认当前 Task 7B 的启动、真实 TUI、自然退出、孤儿恢复、能力矩阵及跨 Harness 隔离证据彼此一致；保留旧预编译程序空白终端的历史记录，不把它误判为当前源码结果。
2. 对发现的每个具体问题完成最小修复；若未发现问题，不为制造代码改动而改代码。
3. 按 `AGENTS.md` 执行完成门列出的全部命令；任何失败都如实报告为未完成，并保留完整失败输出。
4. 交付可审阅的本地分支、清晰的改动/验证摘要和剩余限制。不要推送、创建/更新 PR、合并、改动 `main` 或移动 tag；等待用户审阅和后续指示。

## 仓库与起点

- 仓库：`D:\HarnessHub`
- 起始分支：`task-7b/claude-terminal-acceptance`
- 交接时 HEAD：`1637fa21efb166b747f9784e6b18f60f807f3ca2`（`docs: record current-source Claude GUI exit`）
- 对比基线：`origin/main` = `7b09caea9fbdd437ff6f2700196a26fd77918cc1`
- 创建本交接文档前，代码工作区干净；当前唯一新增的未跟踪文件是本文件。远端没有该分支的开放 PR。
- 分支已有 6 个提交；相对 `origin/main` 修改涉及：Task 7 Claude 计划、Claude adapter 注释、`two_harness_concurrency` 测试、`tests/e2e/README.md`。
- 必须保留隔离诊断 worktree `D:\HarnessHub-E2E\wt-main`，不要进入、清理、覆盖或搬运其中内容。

开始前必须依次阅读：

1. `AGENTS.md`
2. `docs/CONTEXT.md` 与 `docs/CONTEXT-MAP.md`
3. `docs/plans/2026-09-23-task7-second-harness-claude-code.md`
4. `tests/e2e/README.md` 中 Task 7B、7B.1、7D 和 2026-09-27 至 2026-09-29 的记录
5. `docs/adr/0003-local-first.md`、`docs/adr/0005-capability-vs-readiness.md`、`docs/adr/0006-session-state-machine.md`、`docs/adr/0010-reader-vs-reaper.md`、`docs/adr/0013-windows-process-containment-establishment.md`
6. 相关实现：`src-tauri/src/harness/adapters/claude_code.rs`、`src-tauri/tests/claude_lifecycle.rs`、`src-tauri/tests/two_harness_concurrency.rs`

## 已记录的当前证据

以下是交接时仓库已经记录的结果。请检查证据与当前 HEAD 是否一致；这些记录本身不是你独立重跑的结果。

- 当前源码构建的真实 Tauri GUI 启动 Claude v2.1.126，xterm 显示欢迎界面与工作目录；进程树为 `harness-hub.exe → cmd.exe → claude.exe`。
- 在真实 GUI xterm 提交 `/exit` 后，同一 Session 从 `running` 收敛到 `exited / natural_exit / exit_code=0`；全库 `running=0`，Claude 与 shim 进程退出。原始观测、session ID 和数据库状态见 `tests/e2e/README.md` 的「2026-09-29 当前源码 GUI `/exit`：通过」。
- Claude 单独会话的 GUI 强杀重启已有 `unknown / lost` 收敛记录；7D 同时有真实 GUI 双 Harness 会话和定向终止隔离记录。
- `launch`、`terminal`、`usage` 为 true；`resume`、`replay`、`tool_calls`、`subagents`、`live_state`、`worktree` 仍为 false。不要扩大能力声明。
- 2026-09-27/28 的旧预编译程序空白 xterm 观察必须保留为历史证据。它不覆盖当前源码重新构建的 2026-09-29 GUI 结果。
- GUI 自动化脚本和只读 SQLite/WAL 快照脚本仍位于仓库外 `D:\HarnessHub-E2E`。仓库目前不能独立复现这段 GUI 驱动；本 Goal 不负责把这些脚本迁入仓库或扩大为新功能。
- `tests/e2e/README.md` 记录了当前源码本机 Gate 全部通过的输出。Goal 执行者仍须按完成门重新执行并报告自己的实际结果，不得把已有记录冒充为本次执行。

## 工作顺序

1. 确认分支、HEAD、工作区状态和远端基线；保留本交接文件。若除此文件外的状态不符合上面的起点，先记录实际状态，不要 reset、checkout 覆盖或清理用户内容。
2. 按上面的阅读清单检查代码、能力矩阵、计划和 GUI 证据。把每条验收条件映射到具体测试或真实 GUI 记录，指出证据边界。
3. 搜索是否存在文档漂移、断言缺口或实现问题。只有找到具体问题才修改；行为修复遵守 TDD，先新增/调整失败测试并确认失败，再做最小实现。
4. 复核 diff，运行 `git diff --check`，确认没有超出 Task 7B 的改动。
5. 执行仓库完成门中的全部命令。不要在 `pnpm tauri dev` 正运行时运行 Prettier；如需 GUI 真机复验，使用已有窄范围 E2E 工作目录与流程，不要信任用户主目录或更改 `~/.claude.json`。
6. 有实际改动时按“一次提交一件事”制作本地提交，使用仓库要求的提交前缀；没有改动时不要造空提交。
7. 整理最终报告并停下。推送、开 PR、合并和 tag 操作不属于本 Goal。

## 硬边界

- 只在 `D:\HarnessHub` 当前分支工作；禁止切到或修改 `main`，禁止推送分支、创建/更新 PR、合并或移动 tag。
- 不修改或清理 `D:\HarnessHub-E2E\wt-main`。仓库外 GUI 辅助目录只按完成验收所需读取，不复制其用户数据或会话原文进仓库。
- Claude/Codex 等外部 Harness 的日志、Session、配置内容只读。不得向它们发送会话原文到网络；不得把真实密钥写入仓库、SQLite 或日志。
- 不新增 Claude 专用 Terminal runtime、导入器或 Core 特判；若代码显示确需扩大范围，记录证据并停止该扩展，报告阻塞原因。
- 不把“测试跳过”“旧预编译程序结果”或“实现者先前的本机/CI 输出”写成自己的当前源码 GUI 复验。
- 如果需要访问当前不可用的 Claude 安装、GUI 桌面或 Windows Rust 工具链，不得猜测结果；先完成独立可做的审阅与验证，再把剩余阻塞及其精确证据写入报告。

## 完成门与最终报告

必须遵循 `AGENTS.md` 的完成门，并贴出真实命令输出或简洁、完整的结果摘要：

```text
pnpm format:check && pnpm rust:fmt:check
pnpm lint
pnpm typecheck
pnpm test && pnpm rust:test && pnpm python:test
pnpm build
pnpm rust:clippy
```

最终报告按以下结构：

```text
状态：完成 / 未完成（阻塞）
分支与 HEAD：
独立检查的验收项：逐项列结论和证据位置
本次改动与提交：无改动则明确写无
本次实际执行：每条命令及真实结果；未执行项和原因
仍有限制：尤其说明仓库外 GUI 驱动不可复现这一边界
用户后续动作：只在确有需要时列出
```
