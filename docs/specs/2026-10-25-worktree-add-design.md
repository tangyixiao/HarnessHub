# Worktree 创建与状态（ROADMAP C10）Spec

## Problem

多 Agent 并行开发需要每个会话有独立工作树；`sessions.worktree_path` 列
（migration 0001）闲置至今，Hub 看不到仓库有哪些 worktree、也不能一键为
新会话创建一个。CCManager 已验证此工作流（借鉴架构，不抄代码）。

## Locked Goal

给定一个已登记项目（repo_root）：列出其全部 worktree（含主树，只读
`git worktree list --porcelain`）；用户显式创建新 worktree（路径 + 新分支名，
`git worktree add`）；创建的会话可绑定 worktree_path。Git 观察纪律不变
（限时限量、禁用可选锁）；「外部 Harness 日志只读」不适用于用户显式请求的
仓库操作——但**创建/删除必须都是显式用户动作**，无自动后台写。

## Acceptance Criteria

- [ ] `src-tauri/src/git/worktree.rs`：- `list(repo_root) -> Vec<WorktreeEntry>`（porcelain v1 解析：worktree/
      HEAD/branch/detached/bare；解析失败如实报错不猜）；- `add(repo_root, path, branch) -> WorktreeEntry`（`git worktree add -b
      <branch> <path>`；已存在的分支不带 -b；路径已占用/分支冲突 → 明确错误）；- 复用 `git::runner` 限时限量；add 允许更长超时（克隆检出耗时，10s）。
- [ ] IPC：`worktree_list(repo_root)`（纯读）、`worktree_add(repo_root, path,
    branch)`（显式写，前端二次确认文案）。repo_root 必须命中已登记项目
      （防任意路径写）。
- [ ] `create_session` / `start_terminal` 已有 cwd 关联；新增：Terminal 页启动
      时可选 worktree（选中后 cwd=worktree 路径并写 worktree_path 列）——
      `SessionService::start_from_installation` 增加 worktree 参数（None 不变，
      现有调用全兼容）。
- [ ] 状态展示：Projects 页（或 Terminal 启动器）展示 worktree 列表（路径、
      分支、HEAD 短 sha、是否主树、是否 detached）。
- [ ] fixture/测试：真临时仓库（隔离 env）——list 主树/detached/linked、
      add 成功/重复分支/重复路径、porcelain 畸形输出容错；
      service 绑定回写测试；冷启动真路径。
- [ ] DTO camelCase 契约测试 + TS normalize + UI 测试。
- [ ] harness capability `worktree` **保持 false**（那是「该 harness 能否在
      worktree 中跑」的声明，C11 验证后再翻）。

## Non-goals

- 不做 merge / delete（C11）；不做并行编排视图（C11）。
- 不自动创建 worktree（始终显式）。
- 不改 `git_events` 表。

## Decision

`worktree add` 走 `git::runner`（与快照同一条有界执行纪律），路径与分支由
前端显式传入并经已登记项目校验；删除类操作全部留给 C11 单独设计（涉及
未提交变更的安全检查）。

## Risks

- `worktree add` 在大仓库上耗时（检出）→ 10s 上限 + 明确错误；
- 路径写安全：只允许在已登记 repo_root 下的兄弟目录（禁止 repo_root 内嵌套）。

## Verification

真临时仓库单测 + 契约 + `pnpm test` + 全量门 + 本机真实仓库只读 smoke。
