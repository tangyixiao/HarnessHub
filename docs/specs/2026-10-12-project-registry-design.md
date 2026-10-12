# Project Registry 设计（§20 第 14 步）

## 目的与范围

给 Harness Hub 增加本地 Git 仓库注册表：用户登记一个本地目录，Hub 自动识别 repo root，
之后按项目查看最近打开与项目下的 Session 历史。这是路线图 §20 第 14 步
（`HARNESS_HUB_PROJECT_PLAN.md:3726`），对应 §「Project Registry」需求
（`:427`）：添加本地 Git repository、自动识别 repo root、最近项目、项目级 Harness 设置、
项目级 session 历史。

**本步做**：

- `register_project`：输入一个本地目录路径 → 自动 `git -C <path> rev-parse --show-toplevel`
  识别 repo root → 以 repo_root 为唯一键 upsert（重复登记同一个项目是幂等更新，
  不是报错），并刷新 `last_opened_at`。
- `list_projects`：最近项目，按 `last_opened_at` 倒序（从未打开过的按 `created_at`）。
- `remove_project`：按 id 删除（FK `ON DELETE SET NULL` 保证历史 session 保留、
  只是失去归属；`ON DELETE CASCADE` 清掉项目级设置）。
- `get_project_sessions`：项目下的 Session 历史，复用 sessions 表既有
  `project_id` 外键（migration 0001 已建 `idx_sessions_project`）。
- 前端 `ProjectsPage` 从占位页升级为真实注册表页（登记表单 + 项目卡片 +
  每项目的 session 计数），走 `src/lib/ipc.ts` 唯一入口。

**本步不做**：

- 项目级 Harness 设置（`project_harness_settings` 表已建，UI 与读取命令留到
  Project settings 步骤；本步不读不写该表）。
- 终端启动时挂 project（`TerminalRuntime::start` 目前硬编码 `project_id: None`，
  属 terminal 侧改动，单独一步）。
- Git 活动观察（§20 第 16 步 Git observer；`src-tauri/src/git/` 占位模块不动，
  repo root 检测放在独立的 project 模块，避免把观察层与注册表混在一起）。
- 远程仓库、分支管理、worktree 注册（v0.1 只做本地目录）。

## 本机与官方事实（2026-10-12 实测）

- git 2.55.0.windows.2：`git -C <path> rev-parse --show-toplevel`
  - repo root → `D:/HarnessHub`（**正斜杠**）；子目录 → 同 root；
    独立 worktree → 自己的 root。
  - 非 repo 目录 → exit 128，stderr `fatal: not a git repository (or any of the
parent directories): .git`。
  - 不存在的目录 → exit 128，stderr `fatal: cannot change to 'D:\no-such-dir-xyz':
No such file or directory`。
  - 结论：用 `status.success()` + stdout trim 判定，不改进程 cwd；
    两种失败对用户是同一个语义（「不是 git 仓库」），但 stderr 保留到错误消息里便于区分。
- schema 无需迁移：`src-tauri/src/db/migrations/0001_init.sql:33-40`
  `projects(id TEXT PK, name TEXT NOT NULL, repo_root TEXT NOT NULL UNIQUE,
created_at, updated_at, last_opened_at TEXT)`；`:43-49`
  `project_harness_settings`；`sessions.project_id REFERENCES projects
ON DELETE SET NULL` + `idx_sessions_project`（`:91`）。
  `db/migrations.rs:210-214` EXPECTED_TABLES 已含 `projects`。
- 既有链路已支持 project：`commands.rs:148` `create_session(…, project_id,
…)` 透传给 `SessionService::start_from_installation`
  （`session/service.rs:41`），`session/store.rs:204` 有 `with_project_id`；
  `src/lib/ipc.ts:404` `createSession` 已传 `projectId`。
  本步补的是注册表面（projects 行本身），不是 session 挂接。

## 设计

### Rust：新模块 `src-tauri/src/project/`

- `mod.rs`：模块文档 + 公开面（`ProjectRecord`、`RegisterRequest`、store、service）。
- `store.rs`：`ProjectStore<'conn>`，与 `SessionStore` 同构（`new(conn)` +
  位置参数 `?N` + `params![]`）：
  - `upsert(conn 层由 service 控制)` / `insert`、`get(id)`、`get_by_repo_root(root)`、
    `list_recent(limit)`（`ORDER BY COALESCE(last_opened_at, created_at) DESC,
id ASC`）、`touch(id, now)`（刷新 `last_opened_at`）、`delete(id) -> bool`、
    `count()`。
  - `ProjectRecord { id, name, repo_root, created_at, updated_at, last_opened_at }`
    `#[serde(rename_all = "camelCase")]`，SELECT_COLUMNS 常量 + 行映射函数
    （与 session store 同惯例）。
- `service.rs`：`ProjectService<'conn>`：
  - `register(request) -> ProjectRecord`：
    1. repo root 检测（见下），失败 → `Error::InvalidInput`（消息含原始 stderr）。
    2. `get_by_repo_root` 命中 → 更新 `name`/`updated_at` 并 touch
       `last_opened_at`（幂等：重复登记同一个 repo 不产生第二行）。
    3. 未命中 → 生成 UUID v4 id，insert。
  - `remove(id) -> bool`：`delete`，不存在返回 false（幂等）。
  - `list_recent(limit)`、`sessions(hub 分页 limit)` 转发 store。
- `git_root.rs`：repo root 检测，可测试性优先：
  - `trait GitRootProbe: Send + Sync { fn repo_root(&self, path: &Path) ->
Result<Option<String>>; }`（None = 目录不是 git 仓库；Err = 探测本身失败，
    如 git 不存在）；`SystemGitRootProbe` 用
    `Command::new("git").args(["-C", path, "rev-parse", "--show-toplevel"])`。
  - 服务层不直接 `Command::new`，构造器注入 `Arc<dyn GitRootProbe>`，
    测试用 Fake 返回固定 root（与其他模块 HostProbe 注入同一模式）。
- 模块边界：`project → db/error/clock`，不依赖 harness/session；
  `commands.rs → project`（依赖方向与 AGENTS.md 分层一致）。

### IPC 命令（`commands.rs` 新增 4 个）

`register_project(path: String) -> ProjectRecord`、`list_projects(limit: Option<u32>) ->
Vec<ProjectRecord>`、`remove_project(project_id: String) -> bool`、
`get_project_sessions(project_id: String, limit: Option<u32>) -> Vec<SessionRecord>`。
分页 limit 沿用 `DEFAULT_SESSION_LIMIT=50` / `MAX_SESSION_LIMIT=500` 语义。
所有 DTO camelCase 序列化 + 契约测试（跨 IPC 契约纪律）。
`lib.rs` 的 `invoke_handler` 追加 4 个命令（14 → 18）。

### 前端（`src/lib/ipc.ts` + `ProjectsPage`）

- ipc.ts 新增 `ProjectRecord` 类型 + `normalizeProjectRecord`（主键缺失返 null、
  其余补安全默认值，同 `normalizeSessionRecord` 惯例）+
  `registerProject(path)` / `listProjects(limit?)` / `removeProject(id)` /
  `getProjectSessions(id, limit?)`，全部走 `invokeCommand`。
- `ProjectsPage.tsx`：PageState 四态（loading / ipc-unavailable / error / ready），
  登记表单（路径输入 + 登记按钮）+ 项目卡片（name、repo_root、最近打开时间、
  该项目 session 数）+ 删除按钮。复用 `Card`/`Button`/`PageHeader`。
- 路由 `#/projects` 已存在（`src/app/routes.tsx:43`），无路由改动。

## 验收

1. `ProjectStore`/`ProjectService` 单测覆盖：register 幂等（同一 repo 二次登记
   不产生第二行、last_opened_at 刷新）、repo root 检测失败（git 失败 → InvalidInput）、
   list_recent 排序与 limit、delete 幂等返回 false、FK 级联（删项目后
   session.project_id 置 NULL、settings 行被删）。
2. 真机测试：在 `D:\HarnessHub-E2E\project-registry` 建真实 git repo，
   `register_project` 返回 root；对非 repo 目录用真实 `SystemGitRootProbe`
   断言失败（不 mock）。冷启动纪律：至少一条用例从 `Database::open_in_memory()`
   只走生产代码填充（ensure_local_target → register）。
3. IPC 契约：`ProjectRecord` JSON serialization contract test 锁 6 个 camelCase
   键；`normalizeProjectRecord` fixture test 锁降级行为。
4. 前端测试：ProjectsPage 四态 + 登记 → 卡片出现 + 删除 → 卡片消失；
   ipc.ts listProjects/registerProject/removeProject 的 invoke 断言。
5. 门禁全绿：format / rust:fmt / lint / typecheck / test / rust:test / clippy /
   python:test / build / `git diff --check`。
6. GUI 验收：真实 dev app 里登记 `D:\HarnessHub`，卡片出现 repo root 与
   `D:/HarnessHub` 归一化值；删除后消失；SQLite 只读快照确认 projects 行。

## 来源与限制

- `HARNESS_HUB_PROJECT_PLAN.md:3726-3750`（§20 顺序）、`:427-435`（Project
  Registry 需求）、`:3754`（§21 Release Gate 要求多项目）。
- `docs/adr/0007-harness-definition-vs-installation.md`（定义/安装分离，
  projects 不引入第三种「定义」）、`docs/adr/0003-local-first.md`（不读远程）、
  `docs/adr/0012-dashboard-reads-only-sqlite.md`（列表命令只读 SQLite）。
- 本机 git 探测实测（2026-10-12）；schema 事实以
  `src-tauri/src/db/migrations/0001_init.sql` 为准。
- 限制：不解析 `.gitmodules`、不探测 remote、不做目录浏览对话框（v0.1 手动
  输入路径）；`git` 不在 PATH 上的机器登记会失败并提示（readiness，不是缺陷）。
