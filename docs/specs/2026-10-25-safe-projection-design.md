# 安全配置投影（ROADMAP E15）Spec

## Problem

E14 让用户「看见」了配置；切换 Provider / 模型仍要手动编辑文件。规划 §4.15-4.16
定义了投影管线与灾难恢复边界：**写坏原生配置属于最高等级事故**。

## Locked Goal

用户在 Hub 内切换 Claude Code 的 Provider 配置（base URL / model），Hub 按
`read → parse → validate → backup → render → atomic write → verify` 管线
安全投影到 `~/.claude/settings.json`；坏结果绝不落盘（原子写 + verify 兜底），
任何时刻可以恢复到投影前快照。

## Acceptance Criteria

- [ ] `src-tauri/src/profile/projection.rs`：
  - `ProjectionPlan { target: PathBuf, backup: BackupSlot, rendered: String }`
  - `plan_writes(path, new_fields) -> Result<ProjectionPlan>`：read→parse(serde_json
    保留未知键)→merge(只动指定键，其余原样)→render(pretty JSON)→validate(合法 JSON)
  - `apply(plan) -> Result<()>`：backup(拷贝到
    `~/.harness-hub/backups/<harness>/<timestamp>.json`)→temp 文件写入→verify
    (重新 parse 比对)→`rename`（原子替换）。
  - `restore(backup) -> Result<()>`：同管线写回快照内容。
- [ ] merge 语义：new_fields 的键**覆盖**同名键；不删除未知键；嵌套 `env` 块深合并（一层）。
- [ ] 备份保留策略：最近 10 份；超出删除最旧。
- [ ] IPC：`profile_project(harness_id, fields) -> ProjectionReport`（显式写，前端 confirm 文案含「将修改 ~/.claude/settings.json」）；`profile_backups(harness_id) -> Vec<BackupInfo>`；`profile_restore(backup_id)`。
- [ ] 测试：merge 语义（覆盖/保留未知/嵌套合并）、原子性（temp+rename）、
      verify 失败不落盘（注入坏 render）、备份轮换、恢复 round-trip、冷启动真路径。真临时文件树（非真实 ~/.claude）。
- [ ] capability：claude `profiles` 翻 true（投影已实现 + 有测试回归）。

## Non-goals

- 不做 codex/gemini 投影（各自格式差异大，逐个加）；本阶段只 Claude。
- 不做多 Profile 存储比较（后续）；不做 Config Doctor（E17）。
- 不动 OS Secret Store（E16）。

## Decision

管线每步可测试、失败即中止不留半截文件；备份放 Hub 自己的目录
（`~/.harness-hub/backups/`）不混入 harness 数据目录。

## Risks

- 用户手编文件在投影间隙被外部工具改写 → verify 读回时发现差异 → 报错中止
  （不覆盖外部变更，R12 缓解）。

## Verification

全矩阵单测 + 契约 + 全量门。
