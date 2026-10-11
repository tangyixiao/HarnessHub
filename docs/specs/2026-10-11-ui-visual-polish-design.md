# UI 视觉打磨（行为零变更）

## 目的与范围

现有界面是骨架期风格：全灰单色、标签与数值无层级、数据可视化弱、组件平。本次
交付一轮**纯视觉**升级：设计令牌、应用壳、基础组件（Card / Button / PageHeader）、
Dashboard、Sessions、Harnesses、Terminal 容器与占位页。

**做**：颜色层级、状态色系统、数字排版（tabular-nums）、图标点缀、表格与时间线
质感、分段控件、滚动条。
**不做**：任何 IPC / 数据 / 交互语义变更；不改任何文案、aria-label、testid、
路由；不新增依赖。

## 研究依据（2026-10-11 第二轮，指导修订）

- **[Linear 设计系统 token](https://open-design.ai/plugins/design-system-linear-app)**：
  近黑画布按亮度分层（#08090a → #0f1011 → #191a1b）；文字永不纯白、分四级
  （#f7f8f8 / #d0d6e0 / #8a8f98 / #62666d）；描边用白透明（5–8%）而非实灰；
  深色界面**不用投影**，立体感 = 亮度分层 + 1px 描边；单一 accent 只给
  CTA / 激活态 / 链接，禁止装饰性使用；字重 400/510/590；圆角 6/8/12/9999px。
- **[Dashboard UX 模式（Pencil & Paper）](https://www.pencilandpaper.io/articles/ux-pattern-analysis-data-dashboards)**：
  大数字克制使用；渐进披露（次要信息进 hover/tooltip）；颜色编码先用文字约定；
  避免「彩虹沙拉」；每个图表必须挣得自己的位置；空态与加载反馈缺一即「像坏了」。
- 对第一轮的修正：accent 从「四处着色」（KPI 章、头像）收敛回**交互态**；
  卡片从「阴影 + 高光」改为「描边 + 亮度分层」；灰阶降饱和；圆角整体降一档
  （按钮 6px / 卡片 8px / 面板 12px）。

## 设计决策（修订版）

- 令牌：`surface-base/raised/overlay` 更暗、降饱和；新增 `content-secondary`
  文字中间层；accent 微移至 indigo 区（Linear #5e6ad2 的同族）；去掉 body 辉光
  （应用内求平，不求炫）。
- 主按钮：accent 实底 + 白字 + hover 提亮（Linear 模式）；ghost/secondary 用
  白透明底（2–5%）；按压反馈保留。
- 卡片：`rounded-lg` + 半透明底 + hairline 描边，无投影。
- 导航：13px，激活 = accent 淡底 + accent 图标 + 指示条，文字回到主层级。
- KPI 图标章、Harness 头像 → 中性白透明章（accent 不再装饰性使用）。
- 状态徽章（ok/warn/error/running）保留语义色 + 状态点 —— 这是颜色编码的
  合法用途，且有文字标签并行（不单靠颜色）。

## 验收

1. 全部既有前端测试原样通过（文案/aria/testid 未动）。
2. 七个页面 before/after 截图对比，视觉层级明显提升；无行为变化。
3. AGENTS.md 完成门全绿；`pnpm build` 通过。
