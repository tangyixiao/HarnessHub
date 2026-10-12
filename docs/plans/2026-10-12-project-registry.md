# Project Registry 执行计划（§20 第 14 步）

基线：`95e1e9c`（PR #11 合并后的 main），分支 `feat/project-registry`。
spec：`docs/specs/2026-10-12-project-registry-design.md`。

## Task 1 — RED：store + service 契约先行

新建 `src-tauri/src/project/{mod.rs,store.rs,service.rs,git_root.rs}` 的测试骨架
（模块先只写签名与 `todo!()` 不行 —— 本仓库惯例是测试与实现同 PR 但分两次提交，
先让测试失败）：

- `store`：`register_is_idempotent_per_repo_root`、`touch_refreshes_last_opened_at`、
  `list_recent_orders_by_last_opened_then_created`、`delete_returns_false_when_missing`、
  `deleting_a_project_detaches_sessions_and_cascades_settings`
  （FK 级联用裸 SQL 造 session + settings 行）。
- `service`：`register_resolves_the_real_repo_root`（Fake probe 返回 `D:/demo`）、
  `register_rejects_a_path_outside_any_repository`（Fake probe 返回 None →
  `Error::InvalidInput`）、`re_registering_the_same_repo_updates_name_and_touch`。
- 冷启动纪律：`registers_from_a_truly_cold_database` —— `Database::open_in_memory()`
  只跑生产代码（`ensure_local_target` → `ProjectService::register`），不造夹具行。

确认 RED：`cargo test --manifest-path src-tauri/Cargo.toml --lib project` 失败。

## Task 2 — GREEN：最小实现

- `git_root.rs`：`trait GitRootProbe` + `SystemGitRootProbe`
  （`git -C <path> rev-parse --show-toplevel`，stdout trim，失败退 None）。
- `store.rs`：`ProjectStore`（SELECT_COLUMNS / map_project_row / upsert+touch /
  get / get_by_repo_root / list_recent / delete / count）。
- `service.rs`：`ProjectService`（register 幂等 + remove 幂等 + list_recent）。
- `mod.rs`：`ProjectRecord`（6 字段 camelCase）+ `RegisterRequest`。
- 跑 `cargo fmt`；GREEN：同命令通过。

## Task 3 — IPC 命令 + 契约测试

- `commands.rs`：`register_project` / `list_projects` / `remove_project` /
  `get_project_sessions`（session 查询复用 `SessionService::list_recent` 先按
  project 过滤：新增 `SessionStore::list_by_project(project_id, limit)` ——
  顺带让 `ListSessionsFilter` 的预留字段有了真实用途，不改 schema）。
- `lib.rs`：`invoke_handler` 14 → 18。
- 契约测试（跨 IPC 纪律）：`project_record_serializes_with_camel_case_keys`
  锁 6 键；`list_by_project` 单测（两项目各有 session，只返回指定项目的）。
- RED→GREEN 顺序：先写 `list_by_project` 测试确认失败，再实现。

## Task 4 — 前端 ipc.ts + ProjectsPage

- `ipc.ts`：`ProjectRecord` + `normalizeProjectRecord` + `registerProject` /
  `listProjects` / `removeProject` / `getProjectSessions`。
- `ipc.test.ts`：normalize 降级（缺 id → null、缺字段补默认）+ 四个函数的
  invoke 参数断言。
- `ProjectsPage.tsx`：真实页面（四态、登记表单、卡片、删除）；
  `ProjectsPage.test.tsx`：stub `window.__TAURI_INTERNALS__` 模式，覆盖
  空 / 登记 / 删除 / error 四条。
- RED→GREEN：先写测试再实现。

## Task 5 — 真机 + GUI

- `src-tauri/tests/project_registry.rs`：真实 git repo
  （`D:\HarnessHub-E2E\project-registry`，`git init` 产物只读使用）→
  `register_project` 返回 root；非 repo 目录 → 错误含 `not a git repository`；
  未装 git 的探测失败路径用 Fake。目录不存在时 `eprintln!` + SKIP（对齐
  `codebuddy_detection.rs` 惯例）。
- GUI：dev app（override 3300 端口 + CDP 9222）登记 `D:\HarnessHub` → 卡片出现；
  删除 → 消失；只读 SQLite 快照核对 `projects` 行；截图存
  `D:\HarnessHub-E2E\proj-gui-*.png`。
- `tests/e2e/README.md` 新增 Project registry 章节（登记 → 卡片 → 删除 → DB 证据）。

## Task 6 — 文档 + 门禁 + 提交

- `docs/CONTEXT.md` §6：Project Registry 从「尚未实现」移入已完成段；
  `docs/CONTEXT-MAP.md`：Project registry 入口（`src-tauri/src/project/`、
  fixture/测试文件、前端页面）。
- 门禁：`pnpm format:check && pnpm rust:fmt:check`、`pnpm lint`、
  `pnpm typecheck`、`pnpm test`、`pnpm rust:test`（先确认无 harness-hub.exe
  占用）、`pnpm rust:clippy`、`pnpm python:test`、`pnpm build`、
  `git diff --check`。
- 提交惯例（对齐 PR #10/#11 两笔）：`docs(spec): Project registry design and plan`
  → `feat(projects): register local git repositories as projects`。
- 推送（HTTPS fallback：SSH fetch 曾失败）、开 PR、等 CI、合并、同步 main。

## 已知风险

1. repo root 归一化：git 返回正斜杠（`D:/HarnessHub`），用户输入反斜杠 ——
   upsert 以 git 返回值为准，UI 原样展示，不做额外改写（避免发明归一化规则）。
2. `git` 不在 PATH / 非 repo 目录 / 目录不存在三种失败对用户统一为
   「不是有效的本地 Git 仓库」，stderr 附在消息里便于诊断。
3. dev app 占用 exe 会阻塞 `pnpm rust:test`（os error 5）—— 门禁前先停 dev app。
4. vite 全仓库 watch 会因编辑 docs 时的临时文件崩溃（EBUSY）—— dev app 与
   写文档不同时进行。

## 执行记录

### Task 1 — RED（实测）

`cargo test --lib project` 失败：模块不存在（测试先行的骨架即编译失败，属 RED）。

### Task 2 — GREEN（实测）

- `cargo test --lib project` → 16 passed / 0 failed（store 8 + service 8，含
  `registers_from_a_truly_cold_database_with_only_production_code` 冷启动纪律）。
- `cargo test --lib` 全量 → 338 passed（原 320）。

### Task 3 — IPC 契约（实测）

- `commands.rs` 4 命令（register/list/remove/get_project_sessions）；
  `lib.rs` `invoke_handler` 14 → 18；`AppState` 新增 `git_probe`（组合根注入
  `SystemGitRootProbe`）。
- 契约测试 `project_record_serializes_with_camel_case_keys`（6 键逐一断言）+
  `register_request_deserializes_with_camel_case_keys` → `cargo test --lib commands::` 3 passed。
- `SessionStore::list_by_project(project_id, limit)` 新增（`ListSessionsFilter` 的
  预留字段由此获得真实用途，不改 schema）。

### Task 4 — 前端（实测）

- `src/lib/ipc.ts`：`ProjectRecord` + `normalizeProjectRecord` + 4 个函数；
  `ipc.test.ts` 新增 9 条（74 passed，原 65）。踩坑：Tauri invoke 的 spy 收 3 参；
  不传 limit 时 args 被补成 `{}`，断言按 `?.limit === undefined`。
- `ProjectsPage.tsx` 全量重写（四态 + 登记表单 + ProjectCard/ProjectCardBody +
  session 计数 effect）；踩坑：`ProjectCard` 曾被插入两次且首个不完整 → TS1005，
  修复为单一 `ProjectCard`（effect）+ `ProjectCardBody`（展示）。
- `ProjectsPage.test.tsx` 7 条（倒序渲染/登记/失败/删除/空态/IPC 错/浏览器态）；
  踩坑：`lastOpenedAt=null` 与 `sessionCount=null` 都渲染「—」→
  `within(card).getAllByText('—')` 惯例。
- 门禁：`pnpm test` 151 passed（8 files）；typecheck/lint/build 全过。

### Task 5 — 真机 + GUI（实测，2026-10-12）

- `cargo test --test project_registry -- --nocapture` → 1 passed / 0.10s：
  `resolved repo root: D:/HarnessHub`、`registered: name=HarnessHub root=D:/HarnessHub`；
  子目录登记与 root 登记幂等复用同一行；删除后列表空。
  （编译踩坑：`apply_pending(&mut conn)` 需要 mutable；`Arc<T>` 上调 trait 方法需
  `as_ref()` 或 trait 在作用域。）
- GUI（override 3300 + CDP 9222）：登记 → 卡片 `HarnessHub / D:/HarnessHub`；
  重复登记 → 最近打开刷新（14:59:49→14:59:51）、登记时间不变（幂等 touch 实证）；
  只读快照 `projects` 一行 `id=90c65dd4-… repo_root=D:/HarnessHub created=updated=
last_opened=2026-10-12T15:06:06Z`；删除 → 卡片消失、DB 行数 0。截图
  `D:\HarnessHub-E2E\pr-gui-final.png`（空态）、`pr-gui-empty.png`。
  （SQLite 只读踩坑：`mode=ro` URI 不读 WAL 未 checkpoint 数据，app 存活期用普通
  连接 + `PRAGMA query_only=ON` 才能看到刚写入的行。）
- `tests/e2e/README.md` 新增「2026-10-12 — Project Registry」章节（真机输出 + GUI
  证据 + DB 快照）。

### Task 6 — 门禁（实测）

| 门禁                  | 结果                                                    |
| --------------------- | ------------------------------------------------------- |
| `pnpm format:check`   | 通过（prettier）                                        |
| `pnpm rust:fmt:check` | 通过（`cargo fmt` 后）                                  |
| `pnpm lint`           | 0 errors / 4 既有 warnings（exit 0）                    |
| `pnpm typecheck`      | exit 0                                                  |
| `pnpm test`           | 151 passed（8 files）                                   |
| `pnpm rust:test`      | 全绿（各 integration target ok；先停 dev app 释放 exe） |
| `pnpm rust:clippy`    | `-D warnings` exit 0                                    |
| `pnpm python:test`    | 12 tests OK                                             |
| `pnpm build`          | exit 0（dist 388.18 kB / gzip 122.63 kB）               |
| `git diff --check`    | exit 0                                                  |

### 提交

- `docs(spec): Project registry design and plan`（spec + plan 两文件）。
- `feat(projects): register local git repositories as projects`
  （Rust project 模块 + commands/lib.rs + 前端 ipc/页面/测试 + e2e README + 文档）。
