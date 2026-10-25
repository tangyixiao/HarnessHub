# Deep Link 到 Harness 原生 Session（ROADMAP B9）Spec

## Problem

B6-B8 让用户能在 Harness Hub 里回放历史会话；但继续这个会话仍要自己开终端、
自己敲 `claude --resume <id>`。B9 把「回放 → 续聊」接起来：从回放页一键
在内置终端里以 `--resume <source_session_id>` 启动该 Harness。

## Locked Goal

回放页会话详情有「在终端中恢复」按钮：capability `resume` 为 true 的 harness
按其适配器生成的 resume LaunchSpec 启动内置终端会话（hub_session_id 新建、
source_session_id 关联原会话）。capability 不支持 → 按钮不渲染。

## Acceptance Criteria

- [ ] `HarnessAdapter` 增加 `build_resume_spec(ResumeRequest) -> Result<LaunchSpec>`，
      默认实现 = 不支持（错误）；claude 覆写为 `claude --resume <id>`（Windows 走
      既有 program_for 地基）；codex/gemini/opencode 保持默认（capability resume
      仍 false，UI 不渲染按钮——不伪造）。
- [ ] `terminal::TerminalRuntime::start_resume(installation_id, source_session_id, cwd, cols, rows, emitter)`：
      与 start 同一生命周期（created → running → 终态），
      但 SessionRecord 带 `source_session_id = <原会话 id>`、`launch_mode = resume`。
- [ ] IPC `resume_terminal`；capability resume=false 的 harness 调用报明确错误。
- [ ] claude capability `resume` 翻 true（实现即能力；spec 注明证据 = fixture +
      真机 launch 确认参数拼接，真机完整续聊验收待用户交互场景）。
- [ ] Session 去重纪律：同一 (claude, source_session_id) 重复 resume →
      数据库唯一索引拒绝 → 返回明确错误（用户可从既有会话继续）。
      与 B6 只读回放不冲突（回放不写 sessions 表）。
- [ ] 前端：回放详情页按钮（capability 门控）+ 点击后跳转 Terminal 页；
      契约/normalize/页面测试。
- [ ] 冷启动：从空库走生产路径 resume 一条 source 会话（fake PTY）。

## Non-goals

- 不做 codex/gemini/opencode 的 resume（各自参数语义未取证，capability false）。
- 不做跨机器 resume（RuntimeTarget 仍是 local）。
- 不解析 resume 后的原生历史 diff（那是外部 harness 的事）。

## Decision

resume 复用既有 TerminalRuntime 全链路（PTY/containment/状态机/快照），
只是 LaunchSpec 参数与 SessionRecord 字段不同——绝不 fork 一条新启动路径。
`--resume` 参数由适配器拼接（launch-spec 边界，ADR-0008），IPC 层不拼命令。

## Risks

- claude 版本差异（--resume 参数兼容性）→ fixture 锁定拼接；真机 E2E 补充。
- resume 的会话可能已被删除 → claude 会在 TUI 里报错，属于上游行为，如实透传。

## Verification

单测（spec 拼接 / 状态机 / 去重）+ fake PTY 冷启动 + 契约 + `pnpm test` + 全量门。
