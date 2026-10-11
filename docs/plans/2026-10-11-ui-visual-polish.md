# UI 视觉打磨实施计划

规格：[ui-visual-polish-design](../specs/2026-10-11-ui-visual-polish-design.md)。
基线：`cb5883a`（PR #5 合并后），分支 `ui/visual-polish`。before 截图：
`D:\HarnessHub-E2E\ui-before-*.png`（仓库外工具 `ui-shot-all.mjs` 经 CDP 截取）。

## Task 1：令牌与基础组件

- `globals.css`：表面分层微调、`--color-accent-soft`、body 辉光、滚动条。
- `card.tsx` / `button.tsx` / `page-header.tsx`：圆角、内高光、主按钮质感、标题字号。
- `AppShell.tsx`：品牌块加渐变 logo 章；导航激活态 accent 化。
- Expected：`pnpm test -- src/app/AppShell.test.tsx` 原样通过。

## Task 2：Dashboard

- 分段控件化范围选择；KPI 卡加图标章与 tabular-nums 大数字；
  时间线渐变粗条；分布表行分隔 + hover + 表头小写字距；控制平面指标瓦片化。
- Expected：DashboardPage.test.tsx 全绿（文案与 testid 未动）。

## Task 3：Sessions / Harnesses / Terminal / 占位页

- Sessions：状态徽章提亮 + 状态点；UUID/路径 mono 排版微调。
- Harnesses：加载态加旋转图标；Harness 卡首字母头像；能力 ✓ 上色；安装徽章提亮。
- Terminal：启动控件统一类；xterm 容器圆角深底；运行/结束状态行改徽章。
- Projects / Activity / Settings 占位卡：虚线边 + 图标章（文案不动）。
- Expected：相关页面测试全绿。

## Task 4：验证与提交

- `pnpm test` / `lint` / `typecheck` / `build` / format；Rust 未动但按完成门跑全量。
- after 截图对比；`git diff` 审阅确认无行为改动；提交 + PR + 合并。

## 执行记录

- Task 1-3：完成。改动文件：`globals.css`（令牌微调 + 辉光 + 滚动条）、
  `card.tsx` / `button.tsx` / `page-header.tsx`、新增 `placeholder-card.tsx`、
  `AppShell.tsx`（logo 章 + 激活指示条）、`DashboardPage.tsx`（分段控件、KPI
  图标章、渐变时间线、表格分隔与 hover、指标瓦片）、`SessionsPage.tsx`
  （首字母头像 + 状态点）、`HarnessesPage.tsx`（加载旋转 + 头像 + ✓ 上色）、
  `TerminalPage.tsx`（控件统一焦态 + 状态徽章 + 容器内阴影）、三个占位页。
  文案 / aria-label / testid / 路由 / IPC 调用零变更。
- Task 4：门禁全绿（2026-10-11 实测）：format / rust fmt / lint（0 errors /
  4 既有 warnings）/ typecheck / `pnpm test`（130）/ `rust:test`（20 个 binary
  全 ok，0 失败；期间 S2 暴露一次偶发超时 —— Ctrl+C 早于 Ink 初始化被吞，已按
  「等画面静默 + 补发一对」修固并复测通过）/ python（OK）/ build / clippy /
  `git diff --check`。before/after 七页截图：`D:\HarnessHub-E2E\ui-before-*.png`
  / `ui-after-*.png`（仓库外）。PR #6 已合并（f8dcd55）。

## 第二轮：依据设计规范的修订（ui/linear-refinement）

按用户要求先研究再改。研究依据与修订决策见 spec 的「研究依据」与「设计决策
（修订版）」两节；要点：

- accent 从装饰性使用（KPI 章 / 头像全蓝）收敛回交互态；KPI 图标章与 Harness /
  Session 头像改为中性白透明章。
- 卡片从「投影 + 内高光」改为 Linear 式「hairline 描边 + 亮度分层」；表面
  令牌更暗、降饱和；新增 `content-secondary` 文字层级。
- accent 从蓝 250° 微移至 indigo 268°（Linear #5e6ad2 同族）；主按钮改
  accent 实底 + 白字 + hover 提亮；ghost/secondary 白透明底。
- 圆角整体降档：按钮/输入 6px、卡片 8px、药丸 full；时间线改纯色 accent 条。
- 门禁复跑全绿（`pnpm test` 130；首轮 `rust:test` 出现一次
  `a_job_contains_the_child_and_its_descendants` 偶发失败 —— job 活跃进程计数
  3≠2 的环境瞬态，单测与全量复跑均通过）；v2 截图 `ui-v2-*.png`。
- 提交 + PR + 合并。
