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
  全 ok，0 失败）/ python（OK）/ build（4.06s）/ clippy / `git diff --check`。
  before/after 七页截图：`D:\HarnessHub-E2E\ui-before-*.png` /
  `ui-after-*.png`（仓库外）。
