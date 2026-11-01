# Skills / Prompts Registry + System Tray（ROADMAP E19）Spec

## Problem

跨 harness 的项目级记忆文件（`CLAUDE.md` / `AGENTS.md` / `GEMINI.md`）只能手动维护：
同一段指令要在每个仓库 × 每个 harness 各写一份，手工同步必漂移。规划 §4.21 要求
Canonical Content → Harness-specific Renderer，**禁止暴力复制**。托盘（§4.25）是
v0.4.0 收官的第二个交付。写盘基建（E15 的 backup → temp → verify → rename 管线，
`src-tauri/src/profile/projection.rs`）已就绪，本阶段复用与泛化。

## Locked Goal

用户在 Hub 内登记 Prompt（canonical markdown，可绑定项目），按项目 × harness
预览并显式应用投影：每个 harness 一个独立投影文件，Hub 内容用标记块包住，
块外用户内容逐字节保留，重复渲染幂等。Skills 只做只读发现。托盘提供
Open Dashboard / Active Sessions 只读计数 / Quit 三个入口。

## Acceptance Criteria

### Prompts Registry

- [ ] migration 0014 `prompt_artifacts`，注册进 `db::migrations::MIGRATIONS`：

  ```sql
  CREATE TABLE prompt_artifacts (
      id         TEXT PRIMARY KEY,
      name       TEXT NOT NULL UNIQUE,
      content    TEXT NOT NULL, -- canonical markdown
      project_id TEXT REFERENCES projects (id) ON DELETE CASCADE,
      enabled    INTEGER NOT NULL CHECK (enabled IN (0, 1)),
      created_at TEXT NOT NULL,
      updated_at TEXT NOT NULL
  );
  ```

  不做 role 字段（YAGNI）；project_id NULL = 全局（后续按需加 harness 绑定表）。

- [ ] 迁移测试（AGENTS.md 冷启动纪律）：裸 `Connection` + `apply_until(13)` →
      v13 无此表 → `apply_pending` 升到 14；UNIQUE / CHECK / FK 级联各有真实插入被拒用例。
- [ ] `src-tauri/src/prompts/` 模块：CRUD + 渲染器。渲染器核心是纯函数
      （existing markdown + canonical 块 → 合成结果），不依赖文件系统，可独立单测。
- [ ] 标记块语义，Hub 内容用 `<!-- harness-hub:begin -->` 与
      `<!-- harness-hub:end -->` 包住（标记行独占一行）：
  - 目标文件不存在 → 创建，内容 = 标记块（首渲）。
  - 存在且无标记块（纯用户文件）→ 头部插入标记块，原文逐字节保留。
  - 已有标记块 → 旧块整体替换（begin 到 end 含标记行），块外逐字节保留。
  - 只有 begin 没有 end（用户手删）→ 明确报错，绝不落盘。
  - 幂等：二次渲染输出与首次逐字节相同（round-trip 测试）。
- [ ] 多条 prompt：同 scope（project + harness）的 enabled prompt 按 name 升序
      拼接进同一标记块；顺序单测锁定。
- [ ] 写盘走 E15 管线：`profile::projection` 泛化出文本通用的 `apply_text`
      （backup → temp 写入 → verify 逐字节比对 → 原子 rename）；既有 JSON
      `plan` / `apply` 行为不变（回归测试锁定）。备份轮换与列表的扩展名过滤同步泛化。
      注意 `plan` 的「目标文件必须已存在」约束只适用 JSON 投影，文本路径允许创建。
- [ ] IPC 五命令：`prompt_list(project_id?)` / `prompt_upsert` /
      `prompt_remove` / `prompt_render_preview(project_id, harness)`（纯读，返回
      目标路径 + 渲染全文 + 是否新建）/ `prompt_render_apply`（显式写，与 preview
      同参；前端 confirm 文案列出将修改的文件路径）。
- [ ] DTO 契约测试（serde 实际键名 = camelCase，锁定嵌套形状）+ TS normalize +
      fixture test（`src/lib/ipc.test.ts` 同一纪律）。
- [ ] Harness 能力位：`HarnessCapabilities` 新增 `prompts`。claude / codex /
      gemini = true（三文件名映射即实现）；opencode / codebuddy / copilot = false。
      冻结接口的 ADR 修订记录见 Decision。
- [ ] 前端：Projects 页项目卡内新增「Prompts」展开区（列表 + 编辑 + 预览 +
      应用）；全局 prompt（project_id NULL）在卡外「全局」分组展示。

### Skills Registry（缩范围：只读发现）

- [ ] `src-tauri/src/skills.rs`：扫描 `~/.claude/skills/<name>/SKILL.md`，
      只读解析 frontmatter 的 name / description；文件缺失 / frontmatter 坏 →
      计入 skipped，不崩（容错纪律）。
- [ ] IPC `skills_list`（纯读）；Settings 页「Skills（只读）」区展示。

### System Tray（Tauri 2 tray-icon）

- [ ] Cargo：`tauri` features 增加 `tray-icon`（`src-tauri/Cargo.toml`）。
- [ ] `lib.rs` setup 构建 `TrayIcon`；菜单模型（项定义 / 文案 / 禁用态 /
      动作枚举）放不依赖 GUI 类型的纯模块，可单测。
- [ ] 菜单三项：Open Dashboard（显示 + 聚焦主窗口）、
      Active Sessions（「N 个活动会话」，禁用态、纯读）、Quit。
- [ ] 计数数据源：`SELECT count(*) FROM sessions WHERE status = 'running'`；
      菜单构建时重建、构建即现查；重建由应用内既有事件触发，不做后台轮询。
- [ ] 窗口关闭策略不变（不做 close-to-tray；tray 只做入口）。
- [ ] 单测边界：菜单模型层可单测（计数 → 文案 / 禁用态 / 动作映射）；
      **真机验收**（headless CI 无法验证，标注）：图标显示、菜单弹出、
      Open Dashboard 聚焦、Quit 退出。

## Non-goals

- Skills 不做复制 / 投影：claude 自己就是 skills 的加载方
  （`~/.claude/skills` 是它的原生目录），Hub 复制一份没有消费方；
  复制语义与真机验收推迟。扫描范围也只有 `~/.claude/skills`，
  不做其它 harness 的 skills 目录。
- Tray 不做 Profile 切换：`ProfileAdapter.activate` 尚不存在；规划 §4.25
  的切换必须走同一 ProfileAdapter + audit log，那是 activate 实现之后的事，
  Tray 绝不允许绕过安全层。
- 不做 close-to-tray、后台轮询、全局快捷键；tray 只做 v0.4 入口。
- 不做 harness 级 prompt 绑定表（project_id NULL = 全局，后续按需加）。
- 不做记忆文件 → Registry 的双向导入。
- 不做 opencode 的 AGENTS.md 投影扩展（原生也读 AGENTS.md，但需真机证据后
  单独立项）。

## Decision

- 渲染器：canonical → 每 harness 独立投影文件（不是一份文件喂所有 harness），
  Hub 内容永远在标记块内、用户内容永远在块外。v0.4 的 per-harness renderer
  就是「canonical 原文入块」；renderer 接口先立骨架，§4.21 的语义差异后续在
  renderer 层演进，不破坏幂等契约。
- **UI 位置：Prompts 主入口放 Projects 页项目卡内展开区**（Worktrees 区先例）。
  理由：prompt 的作用域主轴是项目（project_id 外键、投影目标 = 仓库根下文件），
  用户心智是「这个仓库的记忆文件」；项目卡内直接展开，无需先选项目。
  Settings 汇总管理不做第二入口（避免双入口漂移）。Skills 是全局资源，
  放 Settings 页（与 Provider 配置现状、配置体检同页）。
- 泛化而非分叉：文本投影共享 E15 的 backup / temp / rename 机制，verify 从
  「JSON parse 比对」泛化为「逐字节比对」；JSON 路径回归测试锁定原行为。
- ADR 修订记录：`harness::HarnessCapabilities` 是冻结接口（CONTEXT.md §5，
  改动需要 ADR）。本次修订：新增 `prompts` 能力位，语义遵守 ADR-0005
  （capability = 实现事实，不随 readiness 变化；三 harness 全 true 的依据 =
  三文件名映射已实现且有测试回归）。修订记录存于本 spec，不另开 ADR 文件；
  后续再变更能力位语义时须补 ADR。
- Tray 菜单模型与 Tauri 类型分离，是 ADR-0012「结构上不可越界」同款纪律：
  可测逻辑放在没有 GUI 依赖的模块里，headless CI 验证到菜单模型为止。

## Risks

- 用户手编记忆文件与投影竞争 → E15 同款缓解：先备份、verify 后 rename、
  失败不落盘；标记块未闭合检测防止吞掉用户内容。
- name 冲突 / 多条 prompt 顺序 → UNIQUE 约束 + 渲染纯函数单测锁定。

## Verification

- 全量门（AGENTS.md 清单）+ 迁移升级用例 + 标记块幂等 round-trip +
  契约 / normalize fixture + tray 菜单模型单测。
- 真机验收（标注）：tray 图标与菜单交互；渲染产物被 claude / codex /
  gemini 实际加载。

## Release 审计（`.github/workflows/release.yml`，只读）

v0.4.0 发版前需同步抬高的 smoke 断言（`smoke` job 的隔离冷启动步骤）：

| 行号 | 当前内容                                                                         | 当前值  | v0.4.0 建议值                                   |
| ---- | -------------------------------------------------------------------------------- | ------- | ----------------------------------------------- |
| 137  | `assert version >= 10, f"schema 版本异常：{version}"`                            | `>= 10` | `>= 14`（0013 注册 + 0014 落地后最新版本 = 14） |
| 138  | `assert harnesses == 5, f"harness 定义行异常：{harnesses}（应为 5 个内置定义）"` | `== 5`  | `== 6`                                          |

补充事实：

- `harnesses == 5` **现在已经过期**：`lib.rs::build_harness_registry` 已注册
  6 个 adapter（codex / claude / gemini / codebuddy / opencode / copilot），
  启动 reconcile 会写入 6 行定义；下一次 tag run 该断言即失败，与 E19 无关也要修。
- schema 断言上方的注释（136 行）同样过期：声称「当前最新 0010_git_snapshots」，
  实际 `MIGRATIONS` 已注册到 0012，0013 的 SQL 文件已存在（E18 落地时注册）。
- 其余 smoke 断言（`sessions == 0` / `plugins == 0` / `PRAGMA foreign_key_check`）
  不受 E19 影响；61-63 行的版本一致性校验与本次改动无关。
