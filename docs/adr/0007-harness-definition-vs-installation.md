# ADR-0007 — Harness 定义与安装分离，写路径显式化

- **Status**: Accepted（2026-09-22）
- **Context**:
  最初只有一张 `harnesses` 表，把三类信息混在一起：

  ```text
  codex                       ← Harness 类型
  0.152.1                     ← 某台机器上的版本
  D:\npm-global\codex.cmd     ← 某台机器上的路径
  detected_at                 ← 某台机器上的检测时间
  ```

  但项目的既定方向是 Local / WSL / SSH / Container 多运行时，同一个 Codex
  在每台机器上都会是不同的版本与路径：

  ```text
  Codex
  ├── @ Local      0.152.1  C:\...\codex.cmd
  ├── @ WSL        0.155.0  /usr/bin/codex
  └── @ SSH        0.149.0  ~/.local/bin/codex
  ```

  同时出现了第二个问题：检测结果的落库一度被塞进 `create_session`
  （以及考虑过塞进 `list_harnesses`）。这会让「只是打开页面看一眼」变成写操作：

  ```text
  打开 Harnesses 页面 → detected_at 变了 → 写库 → audit 变化 → watcher 触发
  ```

- **Decision**:

  1. **拆表**（migration 0002）：

     | 表                      | 职责                                                                                                                                           |
     | ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------- |
     | `harnesses`             | Harness 定义 / 注册表身份（`id`、`display_name`、时间戳）                                                                                      |
     | `harness_installations` | 某个 `runtime_target` 上检测到的真实安装（`binary_path`、`version`、`capabilities_json`、`availability`、`first_detected_at`、`last_seen_at`） |

     `installation.id` 使用复合稳定键 `<harness_id>@<runtime_target_id>`：
     upsert 不需要先查后写，`sessions.installation_id` 也可以直接推导。

  2. **Session 关联到安装**：`sessions.installation_id → harness_installations(id)`
     （可空，导入的历史会话可以没有安装）。

  3. **写路径显式化**：

     ```text
     list / get / inspect                = 不修改状态
     refresh / detect / sync / reconcile = 明确允许修改状态
     ```

     `list_harnesses` 是**纯读**，永不写库。落库只能经过
     `harness::inventory::reconcile_harnesses`，调用点只有：
     应用启动、IPC `refresh_harnesses`（用户点 Refresh）、
     以及 `create_session` 的 invariant guard（只为满足外键，不作为主同步路径）。

  4. `reconcile_harnesses` 在 runtime target 缺失时返回**明确错误**，
     而不是把 SQLite 的 FK 报错抛给上层。

- **Alternatives**:
  - 保持单表，加 `runtime_target_id` 列：主键变成复合键，`harnesses.id` 不再是
    稳定的 Harness 身份；能力矩阵会被迫按机器重复存储。
  - 拆表但保留 `create_session` 顺手同步：写副作用仍然藏在读路径附近，难以推理。
  - 用随机 UUID 作为安装 id：需要先查后写，且无法从 (harness, target) 直接推导。
- **Consequences**:
  - 迁移 0002 必须重建两张表（SQLite 改不了 CHECK 约束），因此迁移执行器支持
    「事务外临时关闭外键 + 迁移后 `PRAGMA foreign_key_check`」。
  - 前端需要新增显式的 Refresh（`refresh_harnesses`）入口，UI 才知道何时会写库。
  - 未来接入 WSL / SSH 时，只需为新的 runtime target 各跑一次 reconcile。
- **Evidence**: `src-tauri/src/harness/inventory.rs`（同步路径与其测试）、
  `src-tauri/src/harness/store.rs`（两张表的 upsert 与 `first_detected_at` 保留）、
  迁移 0002 的 v1 → v2 数据搬运测试。
- **Revisit Conditions**: 当同一 Harness 在同一 runtime target 上可能出现多个安装
  （例如多用户环境）时，需要重新审视「复合稳定键 + UNIQUE(harness_id, runtime_target_id)」。
