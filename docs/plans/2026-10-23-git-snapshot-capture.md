# Git 快照采集 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox syntax for tracking.

**Goal:** 实现独立、只读、有界的 Git 工作树快照采集器。

**Architecture:** Rust `git` 模块负责领域结果、porcelain 字节解析和有界 Git 命令。
不接 SQLite/Session/IPC/UI，复用系统 Git，不增加依赖。主代理负责规格、审查和验收，Luna 实现。

**Tech Stack:** Rust std::process/std::thread、现有 thiserror、系统 Git、Cargo tests。

**Spec:** `docs/specs/2026-10-23-git-snapshot-capture-design.md`（用户已确认第一阶段范围）。

## Global Constraints

遵守 AGENTS.md，先 RED 再 GREEN。只在临时目录准备测试仓库。
不修改冻结接口、migration、真实 app data、未跟踪 handoff；不打印原始 Git 输出或文件名。
本阶段不满足完整 Git Session Snapshot gate，不提高版本号。

## Review Focus

- rename 的 NUL 续项与非 UTF-8 文件名必须由解析测试锁住。
- 空仓库和损坏 HEAD 要由真实仓库测试区分，不以所有非零退出代表 unborn。
- 双流排空、输出限制和 kill/wait 用可控子进程测试，避免死锁或僵尸。
- linked worktree 与继承仓库环境不能使采集指向错误仓库。
- 只读测试比较 index 原字节和 fsmonitor 哨兵，不能仅凭命令名称宣称只读。

## 文件责任

- `src-tauri/src/git/mod.rs`：公开领域类型、错误、capture 入口和范围说明。
- `src-tauri/src/git/status.rs`：内部 porcelain v1 -z 字节解析及 fixture 单测。
- `src-tauri/src/git/runner.rs`：仅用于 Git 的有界命令、双流读取和确定性子进程测试。
- `src-tauri/src/git/snapshot.rs`：命令编排、root/HEAD/status 校验。
- `src-tauri/tests/git_snapshot.rs`：唯一临时目录真实 Git 路径和仓库只读 smoke。
- `docs/CONTEXT.md`、`docs/CONTEXT-MAP.md`：完成后记录真实能力和入口。

允许为符合现有惯例合并小文件，不拆出泛化框架；若复用现有 project probe 执行路径，说明变化并补回归测试。

## Task 1：锁住状态统计与协议解析

**Interfaces:** 内部 `parse_status(&[u8]) -> Result<StatusCounts, GitCaptureError>`。
`StatusCounts` 四个 u64 字段与 spec 的 GitSnapshot 计数字段一一对应。

- [ ] 添加 status fixture 测试：空结果全零；`MM file\0` 的 staged=1、unstaged=1；`?? dir/a\0?? dir/b\0` 的 untracked=2。
- [ ] 添加 rename 双路径只计一、七类冲突只计 conflicted、特殊字节文件名和截断记录报错测试。
- [ ] 运行 `cargo test --manifest-path src-tauri/Cargo.toml --lib git::status` 并记录 RED；不能把测试编写后直接实现并声称 RED。
- [ ] 实现最小字节解析，重复运行同一命令得到 GREEN。
- [ ] 审查未知 XY、安全计数和错误不含路径，再提交 `feat: parse Git worktree status snapshots`。

## Task 2：实现有界只读 Git 执行与快照

**Interfaces:** 生产 `capture(&Path) -> Result<GitObservation, GitCaptureError>`，类型字段按 spec。
内部 runner 接受明确 program/argv/path 与 limits，默认 program 为 git、每条 3 秒、每流 1 MiB。
测试构造器不得通过修改全局环境注入命令。

- [ ] 写可控子进程测试覆盖超时、退出、双流大输出和上限。可用测试可执行文件自身的专用 helper，不能依赖 shell sleep。
- [ ] 运行定向 runner 测试并记录 RED。
- [ ] 实现执行、双流排空、限制、kill/wait 和安全错误；同一测试 GREEN。
- [ ] 为 snapshot 写非仓库、空仓库、HEAD 校验和失败分类测试，记录 RED。
- [ ] 用结构化 argv 实现探测、HEAD、status；禁用可选锁和 fsmonitor，清除影响目标的环境变量。
- [ ] 保持 existing GitRootProbe 契约，不引入第二个 root trait；无需跨 IPC 的类型不添加 serde。
- [ ] 定向测试 GREEN，提交 `feat: capture bounded read-only Git snapshots`。

## Task 3：真实仓库验收与文档

**Interfaces:** 集成测试调用公开生产 `git::capture`，不替换采集实现。

- [ ] 唯一临时目录准备仓库；Git commit 身份用命令参数，不改用户全局配置。
- [ ] 写并运行真实路径测试：unborn/clean/dirty/部分暂存/rename/untracked目录/detached/worktree/bare/nonrepo/missing/corrupt HEAD。
- [ ] 对尚未满足的边界先记录 RED，再修最小实现并复跑 GREEN。
- [ ] 对 HEAD、index 原字节、工作树文件内容和 fsmonitor 哨兵做只读断言。
- [ ] 添加生产 capture 对本仓库的只读 smoke，不打印敏感输出。
- [ ] 更新 CONTEXT 与 CONTEXT-MAP，仅描述已实现采集能力，明确 Session/UI 未接线。
- [ ] 运行所有九项检查，将真实结果写入本计划验收记录；`git diff --check`；审查无 handoff/运行产物/隐私数据进入提交。
- [ ] 提交 `test: verify Git snapshot capture on real repositories`，推送分支、创建独立 PR并附到本聊天。

## 验收记录

尚未运行；实现后以实际命令输出补充。
