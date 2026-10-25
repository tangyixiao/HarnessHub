# Harness Hub

Harness Hub 是一个 **local-first 的 AI Coding Harness 桌面控制中心**，用于登记项目、发现本机 Harness、管理终端会话，并查看活动与使用量记录。它不会替代 Codex、Claude Code、Gemini CLI 或 OpenCode。

- 项目规格：[HARNESS_HUB_PROJECT_PLAN.md](./HARNESS_HUB_PROJECT_PLAN.md)
- 稳定事实：[docs/CONTEXT.md](./docs/CONTEXT.md)
- 代码地图：[docs/CONTEXT-MAP.md](./docs/CONTEXT-MAP.md)
- 架构决策：[docs/adr/](./docs/adr/)
- 实施计划：[docs/plans/](./docs/plans/)

## 当前版本

**v0.1.0 — 首个正式版本（Windows x64 + Linux x64）**。相对 alpha 系列，补齐了
Git Session Snapshot（会话 start/end 快照 + 窗口摘要）、Gemini usage 解析回归、
Linux deb/AppImage 自动打包与发布流水线。下载入口：
[v0.1.0 Release](https://github.com/tangyixiao/HarnessHub/releases/tag/v0.1.0)；
验证范围见[发布说明](./docs/releases/v0.1.0.md)。

当前实现包含：

- 登记 Git 项目，并按仓库根目录去重；
- 检测 Codex、Claude Code、Gemini CLI、OpenCode 与 CodeBuddy，并按各自实现情况提供能力；
- 通过内置终端启动并管理支持的 Harness 会话；
- 会话生命周期的 Git 快照与窗口摘要（「会话期间的变更」，不做 AI 归因）；
- 汇总使用量与会话活动，查看 Activity Timeline（Codex / Claude Code / OpenCode /
  Gemini 的 usage 经 ccusage 统一导入）；
- 插件宿主 v0.1 后端与 IPC：仅提供 `plugin_list` / `plugin_refresh`。Rust service 负责调用插件的 `usage/import`；skill 在 refresh 时拉取并缓存，随后随插件列表返回。暂未提供插件管理 UI。

能力矩阵表示适配器实现的功能，不保证每台机器上的 Harness 当前都可用。认证、安装和 GUI 环境会影响实际 readiness。详见 [E2E 与验收记录](./tests/e2e/README.md)。

## 下载与运行

从 [v0.1.0 Release 条目](https://github.com/tangyixiao/HarnessHub/releases/tag/v0.1.0) 下载对应平台安装包与 `SHA256SUMS`（校验后再安装）：

- **Windows x64**：`Harness-Hub_0.1.0_x64-setup.exe`（未签名 NSIS；SmartScreen 可能提示发布者未知）。需要 WebView2 Runtime（Windows 11 通常已包含）。
- **Linux x64**：`Harness-Hub_0.1.0_amd64.deb`（`sudo apt install ./Harness-Hub_0.1.0_amd64.deb`，依赖由包声明）或 `Harness-Hub_0.1.0_amd64.AppImage`（免安装，宿主仍需 WebKitGTK 4.1 运行库）。Linux 打包基线为 ubuntu-22.04（glibc 2.35）。

### 10 分钟上手

1. 安装并启动 Harness Hub（见上）。
2. 准备至少一个 Harness（如 Codex CLI：`npm i -g @openai/codex` 后 `codex` 完成登录）。Harness Hub 不附带 Harness 程序或凭据。
3. 打开 **Harnesses** 页确认检测到的安装与能力矩阵；未安装的会如实标 Missing。
4. 在 **Projects** 页登记一个本地 Git 仓库目录（自动识别 repo root）。
5. 从内置终端启动一次会话：选择 Harness → 启动 → 在终端里对话；结束后 **Sessions** 页会出现该会话及其「Git · 会话期间的变更」。
6. 在 **Dashboard** 点导入 Usage（需要 Node.js/npm：runner 依次尝试 PATH 上的 `ccusage`、用户配置命令、或 `npx --yes ccusage@20.0.24` 固定版本 fallback），随后查看 Token / 成本下界 / Harness 分布与 **Activity** 时间线。

Python sidecar 功能另需 Python 3.10+ 与 uv 0.5+；不装也不影响上述主流程。

## 从源码运行

开发环境需要 Node.js 22 LTS（22.12+）或 Node.js 24 LTS、pnpm 11、Rust stable（含 rustfmt 和 clippy）、Windows MSVC Build Tools 与 WebView2 Runtime。Python sidecar 使用时另需 Python 3.10+ 和 uv 0.5+。

```powershell
git clone https://github.com/tangyixiao/HarnessHub.git
cd HarnessHub
pnpm install
pnpm tauri dev
```

常用验证命令见仓库 [AGENTS.md](./AGENTS.md)。

## 已知限制

- Gemini CLI 的认证与 GUI 终端渲染仍受本机环境限制；已有 headless lifecycle 证据不能替代认证恢复后的 chat 级 GUI 验收。Gemini usage 解析已有回归覆盖，但本机尚无真实 Gemini 使用数据可导入。
- OpenCode 的 headless lifecycle 已取证；当前记录中的 GUI TUI 渲染受环境阻塞，OpenCode 认证 readiness 也需由本机配置决定。
- CodeBuddy 当前为 detection-only，尚不支持启动、终端或 usage 导入。
- Git 快照与窗口摘要只描述「会话期间的变更」，不能证明代码由 AI 编写；没有快照也不代表没有变化（best-effort 观测）。
- 插件 v0.1 仅暴露后端 IPC，没有 UI；权限决策为 observe-only 记录，不构成安全沙箱或访问隔离。Linux 上插件宿主需要 `python` 命令（Debian/Ubuntu 默认只有 `python3`，装 `python-is-python3` 可解）。
- 安装包未签名；本版本的构建、质量门与冷启动验证证据见 Release 说明。实际 Harness readiness 仍依赖本机安装、认证和 GUI 环境。

## License

MIT
