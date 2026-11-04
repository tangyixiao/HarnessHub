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

- [x] 添加 status fixture 测试：空结果全零；`MM file\0` 的 staged=1、unstaged=1；`?? dir/a\0?? dir/b\0` 的 untracked=2。
- [x] 添加 rename 双路径只计一、七类冲突只计 conflicted、特殊字节文件名和截断记录报错测试。
- [x] 运行 `cargo test --manifest-path src-tauri/Cargo.toml --lib git::status` 并记录 RED；不能把测试编写后直接实现并声称 RED。
- [x] 实现最小字节解析，重复运行同一命令得到 GREEN。
- [x] 审查未知 XY、安全计数和错误不含路径，再提交 `feat: parse Git worktree status snapshots`。

## Task 2：实现有界只读 Git 执行与快照

**Interfaces:** 生产 `capture(&Path) -> Result<GitObservation, GitCaptureError>`，类型字段按 spec。
内部 runner 接受明确 program/argv/path 与 limits，默认 program 为 git、每条 3 秒、每流 1 MiB。
测试构造器不得通过修改全局环境注入命令。

- [x] 写可控子进程测试覆盖超时、退出、双流大输出和上限。可用测试可执行文件自身的专用 helper，不能依赖 shell sleep。
- [x] 运行定向 runner 测试并记录 RED。
- [x] 实现执行、双流排空、限制、kill/wait 和安全错误；同一测试 GREEN。
- [x] 为 snapshot 补非仓库、空仓库、HEAD 校验和失败分类测试；审查回归保存真实 RED/GREEN，初版流程例外见验收记录。
- [x] 用结构化 argv 实现探测、HEAD、status；禁用可选锁和 fsmonitor，清除影响目标的环境变量。
- [x] 保持 existing GitRootProbe 契约，不引入第二个 root trait；无需跨 IPC 的类型不添加 serde。
- [x] 定向测试 GREEN，提交 `feat: capture bounded read-only Git snapshots`。

## Task 3：真实仓库验收与文档

**Interfaces:** 集成测试调用公开生产 `git::capture`，不替换采集实现。

- [x] 唯一临时目录准备仓库；Git commit 身份用命令参数，不改用户全局配置。
- [x] 写并运行真实路径测试：unborn/clean/dirty/部分暂存/rename/untracked目录/detached/worktree/bare/nonrepo/missing/corrupt HEAD。
- [x] 对尚未满足的边界先记录 RED，再修最小实现并复跑 GREEN。
- [x] 对 HEAD、index 原字节、工作树文件内容和 fsmonitor 哨兵做只读断言。
- [x] 添加生产 capture 对本仓库的只读 smoke，不打印敏感输出。
- [x] 更新 CONTEXT 与 CONTEXT-MAP，仅描述已实现采集能力，明确 Session/UI 未接线。
- [x] 运行所有九项检查，将真实结果写入本计划验收记录；`git diff --check`；审查无 handoff/运行产物/隐私数据进入提交。
- [x] 提交 `test: verify Git snapshot capture on real repositories`，推送分支、创建独立 PR并附到本聊天。

## 验收记录

2026-10-24，在 Windows / Rust 1.98.1 / Git 2.55.0.windows.2 上验收。
最终生产与测试源码为 `8bb6e67`；随后只更新本计划。独立逐任务审查、整分支审查及唯一一波最终修复复审均通过。
Git 采集器未改 SQLite、Session、IPC、UI、依赖或版本号。

| 命令                  | 实际结果                                                                                              |
| --------------------- | ----------------------------------------------------------------------------------------------------- |
| `pnpm format:check`   | `All matched files use Prettier code style!`                                                          |
| `pnpm rust:fmt:check` | exit 0，无格式诊断                                                                                    |
| `pnpm lint`           | `0 errors, 4 warnings`；四条是原有 Fast Refresh 警告                                                  |
| `pnpm typecheck`      | `tsc --noEmit`，exit 0                                                                                |
| `pnpm test`           | `Test Files 9 passed (9)`；`Tests 179 passed (179)`                                                   |
| `pnpm rust:test`      | 库测试 `462 passed; 0 failed; 2 ignored`；所有集成目标和 doc tests 通过；Git 真实仓库矩阵 `10 passed` |
| `pnpm python:test`    | `Ran 12 tests`，`OK`                                                                                  |
| `pnpm build`          | `built in 5.94s`，exit 0                                                                              |
| `pnpm rust:clippy`    | `Finished dev profile`，`--all-targets -- -D warnings`，exit 0                                        |

完整 Rust 重跑的首次并行编译在 mmap 阶段遇到 Windows 页面文件错误 1455；进程内设置 `CARGO_BUILD_JOBS=1` 后重跑上述完整命令成功。
两个 ignored Rust 项均为专用子进程 helper，由父测试显式启动，不是跳过功能验收。
真实 SHA-256 在本机执行并返回 64 位 HEAD，没有走 unsupported SKIP。

真实矩阵覆盖 root/子目录、clean、MM 部分暂存、忽略与逐文件 untracked、rename、merge conflict、detached、linked worktree、unborn、bare、非仓库和损坏 HEAD。
只读断言比较 HEAD 文件、解析后的 commit OID、index 原字节和工作文件；fsmonitor 先用正对照证明可执行，再验证采集不会触发它。
最终单元夹具还通过子进程注入仓库、index、config 环境污染，确认 disposable 外部 sentinel 不变；测试不修改全局环境或真实用户仓库。

流程例外必须保留：初版 Task 2 生产实现早于所要求的新测试；原 Task 2b 第一轮 RED 原始输出没有保存，观察笔记不是 raw log，无法追补为全程严格 TDD。
后续 parser / runner / 分类 / 环境隔离 / 夹具修复有真实 RED/GREEN，最终功能通过结果不消除该历史流程例外。

原始日志保留本地 `test-results/2026-10-23-git-capture/` 和 `test-results/git-snapshot-final-fix-*.log`；逐任务报告和审查记录已归档至前者的 `sdd-evidence/`。
这些运行产物不进入 PR。原开发提交保留本地 `codex/git-snapshot-capture-evidence`，发布分支从相同最终源码整理为独立的设计、解析、采集和验收提交。
现有未跟踪 `docs/handoffs/2026-09-29-task7b-dsh-goal.md` 保留。

交付 PR：[只读 Git 快照 #19](https://github.com/tangyixiao/HarnessHub/pull/19)。Linux CI 以该 PR 当前 HEAD 的 Checks 为准。
互通复用方向另见 [调研与 Proposed 规格 #18](https://github.com/tangyixiao/HarnessHub/pull/18)，未与本采集器混成一个功能交付。
本阶段不满足完整 Git Session Snapshot gate；下一阶段再接 Session 起止快照、持久化和历史页。
