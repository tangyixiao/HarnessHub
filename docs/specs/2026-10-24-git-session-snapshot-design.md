# Git Session Snapshot 接线（ROADMAP 阶段 A1）Spec

## Problem

`git::capture(&Path)`（migration 前的内部能力）已能对任意目录做有界只读快照，
但没有接进 Session 生命周期：不落库、无 IPC、无 UI。v0.1 Release Gate 的
「Git Session Snapshot」未满足（docs/CONTEXT.md §6 明确记录）。

## Locked Goal

Harness Hub 管理的终端会话在「启动成功」与「进入终态」两个时点各落一条
有界 Git 快照；Sessions 页能展示会话窗口内的 Git 变化（**Changes during
session**），用词不得声称 AI 贡献（CONTEXT.md 核心约束 7 / 规划 R5）。

## Acceptance Criteria

- [ ] migration 0010 新增 `git_snapshots`，(hub_session_id, phase) 主键，phase ∈ {start, end}，
      四类文件计数非负，FK 到 sessions ON DELETE CASCADE；老库 v9→v10 升级测试
      （裸 Connection + apply_until）通过。
- [ ] 启动成功（mark_running 之后）写 start 快照；进程终态写入成功后写 end 快照。
- [ ] 快照 best-effort：cwd 缺失 / 非 Git 仓库 / git 命令失败 → 不落行、**不影响**
      会话状态机与启动/退出路径（观测失败绝不吞掉真实错误）。
- [ ] 快照失败幂等：重复写同一 (session, phase) 返回 false 不报错。
- [ ] `git capture` 的子进程（每条 3s 上限）**不得在数据库锁内执行**（terminal.rs
      短事务纪律）。
- [ ] IPC `session_git_snapshots` 纯读返回快照列表，Rust DTO camelCase 契约测试。
- [ ] TS normalize fixture 测试 + Sessions 页展示；文案使用「会话期间的变更」，
      出现 head 短 sha 与四类计数，绝不写「AI 写了 N 行」。
- [ ] 冷启动真路径用例：空库 → 生产代码填充前置 → 会话 → 快照行。

## Non-goals

- 不做 `+lines/-lines`、commit 数（阶段 A3 用 `git log start..end` 有界查询）。
- 不做 worktree 元数据（阶段 C10）。
- 不改 `git_events` 表（那是 Canonical Event 平面（G24）的容器）。
- 不在 reconcile_orphans（宿主重启收敛）时补拍快照——进程早已结束，
  此刻的仓库状态不代表会话结束时刻。

## Evidence / Existing Solutions

- 现有实现：`src-tauri/src/git/{mod,snapshot,runner,status}.rs`（capture），
  真实仓库测试 `src-tauri/tests/git_snapshot.rs`。
- 会话状态机：`src-tauri/src/session/store.rs`（ADR-0006）。
- 终端编排：`src-tauri/src/terminal.rs`（with_db 短事务纪律）。

## Grill Report（摘要）

- **Goals**：见 Locked Goal。
- **Acceptance**：见上；失败模式（非 repo、git 缺失、超时）必须显式处理。
- **Boundaries**：只读外部仓库；不起长事务；不写 git_events。
- **Alternatives**：复用 `git_events`（kind=head_start/head_end）——被否决：
  其形状是事件流（无四类计数、无 (session,phase) 唯一约束），且属于未来
  Trace 平面；新表职责单一。
- **Assumptions**：`git::capture` 的 3s/命令限流对启动路径可接受
  （worst case ~6 条命令；启动是低频用户动作）。

## Decision

新表 `git_snapshots`（migration 0010）+ `git::store` / `git::service` 两个子模块；
terminal.rs 在 mark_running 成功后与 on_exit 的 finish 成功后各触发一次
「先短锁读 cwd → 无锁跑 capture → 短锁写行」的 best-effort 流程；
commands.rs 增加 `finish_session` 终态更新成功后的同款触发与
`session_git_snapshots` 只读命令。

## Rejected Alternatives

- 在 create_session（created 态）拍 start：此时进程未启动，「会话开始」语义
  应锚定进程真正跑起来（mark_running）。
- 在 DB 锁内同步 capture：违反短事务纪律，git 卡住会堵死所有 IPC。

## Risks

- 快照与终态写入并发：PK 去重，重试安全。
- capture 耗时拉长启动感知：观测失败只记 false；后续阶段可移到后台任务。

## Verification

`pnpm rust:test`（迁移/store/service/terminal/契约/冷启动）+ `pnpm test`
（TS normalize + 页面）+ 全套质量门。
