# UI 视觉打磨（行为零变更）

## 目的与范围

现有界面是骨架期风格：全灰单色、标签与数值无层级、数据可视化弱、组件平。本次
交付一轮**纯视觉**升级：设计令牌、应用壳、基础组件（Card / Button / PageHeader）、
Dashboard、Sessions、Harnesses、Terminal 容器与占位页。

**做**：颜色层级、状态色系统、数字排版（tabular-nums）、图标点缀、表格与时间线
质感、分段控件、滚动条。
**不做**：任何 IPC / 数据 / 交互语义变更；不改任何文案、aria-label、testid、
路由；不新增依赖。

## 设计决策

- 令牌：保持小集合传统，只增 `--color-accent-soft` 并微调表面分层对比；body 加
  极淡的顶部 accent 辉光与自定义滚动条。
- 主色系不动（accent 蓝 250°），状态色（ok/warn/error）从「描边+10% 底」提到
  12–15% 可见度并统一圆角。
- 数字全部 `tabular-nums`；时间/ID/路径保持 mono。
- Dashboard：范围选择器改为分段控件（仍是 Button + aria-pressed）；KPI 卡加
  accent 图标章；时间线改渐变粗条；分布表加行分隔与 hover。
- Sessions / Harnesses：状态徽章提亮；Harness 卡加首字母头像；能力矩阵 ✓ 上色。
- Terminal：启动控件统一焦态；xterm 容器圆角加深色底；状态行改徽章。
- 占位页（Projects / Activity / Settings）：虚线卡片 + 图标，不再像内容卡。

## 验收

1. 全部既有前端测试原样通过（文案/aria/testid 未动）。
2. 七个页面 before/after 截图对比，视觉层级明显提升；无行为变化。
3. AGENTS.md 完成门全绿；`pnpm build` 通过。
