# Activity Timeline 执行计划（§20 第 15 步）

- 日期：2026-10-14
- 规格：`docs/specs/2026-10-14-activity-timeline-design.md`
- 分支约定：与上一步相同 —— 一个 `docs(spec): …` 提交 + 一个 `feat(…): …` 提交，走 PR。
- 前置确认：`git status` 干净（仅既有未跟踪的 `.survey-b*.md` 与 `docs/handoffs/`，
  本步不提交它们）；无 `harness-hub.exe` 占用（否则 `pnpm rust:test` 会 os error 5）。

## Task 1 — RED：先写失败的测试

1. `src-tauri/src/activity/mod.rs`：只放 DTO + `keys()` + `#[cfg(test)] mod tests;`，
   `query.rs` 先留 `unimplemented!()` 或最小签名，让编译通过但断言失败。
2. 单测（`activity/tests.rs`）覆盖规格「验收 1」的每一条：
   - 冷启动：`Database::open_in_memory()` → `ensure_local_target` +
     `reconcile_harnesses`（`test_support::cold_start_db()`）→ 生产 `UsageImporter` 灌 usage →
     生产 `SessionService` 灌 session → 查询。**不写裸 INSERT 造 FK 前置。**
   - 本地日边界（UTC+8，`NOW = 2026-09-22T23:30:00Z`，`UTC_PLUS_8 = 480`）。
   - `duration_ms` 的 NULL / 非 NULL 两态。
   - `project_id` 只在 `hub_session_id` 可 join 时非空。
   - `has_more` / `hours.len() == 24` / `days` 升序。
   - 范围过滤 `?N` 绑定（覆盖修饰符 + 范围参数同时存在）。
3. 契约测试：`ActivityTimeline::keys()` 全量 + `events[0]` / `days[0]` / `hours[0]` 键名，
   `assert!(json.get("snake_case_twin").is_none())`。
4. 前端 RED：`src/features/history/ActivityPage.test.tsx`（stub 未预期命令即抛错）
   - `src/lib/ipc.test.ts` 的 `normalizeActivityTimeline` fixture test。
5. 命令：`cargo test --manifest-path src-tauri/Cargo.toml --lib activity` 与
   `pnpm test` 必须**失败**，并把真实失败输出记入本文件。

## Task 2 — GREEN：最小实现

1. `src-tauri/src/activity/query.rs` 写两条 SQL + 聚合（见规格「设计」），
   `lib.rs` 加 `pub mod activity;`。
2. `cargo fmt`，然后 `cargo test --manifest-path src-tauri/Cargo.toml --lib activity`
   与 `cargo test --manifest-path src-tauri/Cargo.toml --lib`（全量 lib）转绿。
3. 重构：确认没有重复造 `session::store` / `project::store` 已有的读取 API。

## Task 3 — IPC 命令 + 契约测试

1. `src-tauri/src/commands.rs` 新增 `activity_timeline`（`DEFAULT_ACTIVITY_LIMIT = 200` /
   `MAX_ACTIVITY_LIMIT = 1000` 夹取）。
2. `src-tauri/src/lib.rs` 的 `invoke_handler` 18 → 19（保持字母序）。
3. 命令层契约测试：`activity_timeline_serializes_with_camel_case_keys`
   （`ActivityTimeline` 顶层键 + `range` 嵌套 + `events[0]` 键）。

## Task 4 — 前端 ipc.ts + ActivityPage + 测试

1. `src/lib/ipc.ts`：类型 + `normalizeActivityEvent` / `normalizeActivityTimeline` +
   `getActivityTimeline({range, timezoneOffsetMinutes, limit})`。
2. `src/features/history/ActivityPage.tsx`：范围切换 / KPI / Activity Calendar /
   Session Timeline / 空态 / 约束 7 提示横幅。**不 import 别的 feature 的文件**
   （Dashboard 的 `formatCost` / `formatTokens` 不跨 feature 复用，本地定义）。
3. `pnpm test` 全绿；`pnpm lint` / `pnpm typecheck` 干净。

## Task 5 — 真机测试 + GUI 证据

1. `src-tauri/tests/real_activity_timeline.rs`：
   - `CARGO_MANIFEST_DIR` 定位仓库根；裸 `Connection` + `apply_pending(&mut conn)` 建真库；
   - 生产写入路径灌数据 → `activity::query::timeline` → 与**独立手写 SQL** 逐项对账；
   - close/reopen 后数字不变；
   - 断言查询路径不起进程（签名只拿 `&Connection`，编译期保证 + 文档说明）。
   - 证据：`cargo test --manifest-path src-tauri/Cargo.toml --test real_activity_timeline -- --nocapture`。
2. GUI：`pnpm tauri dev`（`--config` 覆盖 devUrl 3300，CDP 9222），实际打开 `/activity`：
   空态、有数据态、范围切换、点击某天过滤，截图存 `D:\HarnessHub-E2E\`，
   并在 `tests/e2e/README.md` 增加一节。

## Task 6 — 文档 + Gates + 提交

1. `docs/CONTEXT.md` §6：把 Activity Timeline 从「尚未实现」移到「已完成」，写明模块路径、
   1 个 IPC 命令、Activity 页面。
2. `docs/CONTEXT-MAP.md`：History / Activity Timeline 行补上 `src-tauri/src/activity/`。
3. Gates 表（真实输出）：

   | 门禁      | 命令                  | 结果                                                                                  |
   | --------- | --------------------- | ------------------------------------------------------------------------------------- |
   | format    | `pnpm format:check`   | 通过（2026-10-16）                                                                    |
   | rust fmt  | `pnpm rust:fmt:check` | 通过（2026-10-16）                                                                    |
   | lint      | `pnpm lint`           | 通过（0 errors；4 个 react-refresh/only-export-components warnings 系既有）           |
   | typecheck | `pnpm typecheck`      | 通过                                                                                  |
   | unit      | `pnpm test`           | 通过（167 passed / 9 files）                                                          |
   | rust unit | `pnpm rust:test`      | 通过（lib 356 passed + 各 harness 测试套件；opencode_lifecycle S2 修复后 4 条链全绿） |
   | python    | `pnpm python:test`    | 通过（12 tests）                                                                      |
   | build     | `pnpm build`          | 通过（rolldown 7s；tailwind hook 3.5s 属正常 timing 报告）                            |
   | clippy    | `pnpm rust:clippy`    | 通过（`-D warnings` 干净）                                                            |

4. 两次提交：`docs(spec): Activity timeline design and plan` →
   `feat(activity): unified activity timeline projection`。
   推送优先 SSH，失败回退 HTTPS（上一步的既有做法）。

## 已知风险（来自上一步的实测）

1. **dev app 占用 `harness-hub.exe`** → `pnpm rust:test` 报 `os error 5`。
   跑 rust 测试前先确认没有 `harness-hub` 进程。
2. **vite watch EBUSY**：dev server 运行时改 docs 可能触发崩溃；写文档时留意。
3. **SQLite `mode=ro` 不读未 checkpoint 的 WAL**：真机对账用普通连接 +
   `PRAGMA query_only=ON`，不要用只读 URI。
4. **Tauri invoke spy 收到 3 个参数**：前端断言 `?.limit` 而不是参数个数
   （上一步踩过：省略 limit 时 `invoke` 仍收到 `{}`）。
5. **`?N` 绑定顺序**：activity 的聚合 SQL 同样把时区修饰符放在 `?1`，
   范围参数必须显式从 `?2` 起（summary.rs:294-312 记录的坑）。
