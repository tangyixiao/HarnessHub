# Harness Hub

Harness Hub 是一个 **local-first 的 AI Coding Harness 桌面控制中心**，用于登记项目、发现本机 Harness、管理终端会话，并查看活动与使用量记录。它不会替代 Codex、Claude Code、Gemini CLI 或 OpenCode。

- 项目规格：[HARNESS_HUB_PROJECT_PLAN.md](./HARNESS_HUB_PROJECT_PLAN.md)
- 稳定事实：[docs/CONTEXT.md](./docs/CONTEXT.md)
- 代码地图：[docs/CONTEXT-MAP.md](./docs/CONTEXT-MAP.md)
- 架构决策：[docs/adr/](./docs/adr/)
- 实施计划：[docs/plans/](./docs/plans/)

## 当前版本

正在准备 **v0.1.0-alpha.4**。Windows x64 unsigned NSIS 安装包与 SHA-256 校验文件尚未构建；Release 尚未发布。准备完成后将在 [GitHub Releases](https://github.com/tangyixiao/HarnessHub/releases) 提供下载，目标条目为 [v0.1.0-alpha.4](https://github.com/tangyixiao/HarnessHub/releases/tag/v0.1.0-alpha.4)。

当前实现包含：

- 登记 Git 项目，并按仓库根目录去重；
- 检测 Codex、Claude Code、Gemini CLI、OpenCode 与 CodeBuddy，并按各自实现情况提供能力；
- 通过内置终端启动并管理支持的 Harness 会话；
- 汇总使用量与会话活动，查看 Activity Timeline；
- 插件宿主 v0.1 后端与 IPC：仅提供 `plugin_list` / `plugin_refresh`。Rust service 负责调用插件的 `usage/import`；skill 在 refresh 时拉取并缓存，随后随插件列表返回。暂未提供插件管理 UI。

能力矩阵表示适配器实现的功能，不保证每台机器上的 Harness 当前都可用。认证、安装和 GUI 环境会影响实际 readiness。详见 [E2E 与验收记录](./tests/e2e/README.md)。

## 下载与运行

发布后，从 [GitHub Releases](https://github.com/tangyixiao/HarnessHub/releases) 下载 Windows x64 NSIS 安装包。该 alpha 安装包计划为**未签名**版本；Windows 可能显示发布者未知或 SmartScreen 提示。安装后可从开始菜单启动 Harness Hub。

运行桌面应用需要 Windows x64 与 WebView2 Runtime（Windows 11 通常已包含）。要使用某个 Harness，需先在本机单独安装并配置它；Harness Hub 不附带 Harness 程序或凭据。内置 usage 导入会依次使用 PATH 上的 `ccusage`、用户配置的命令，或通过 `npx --yes ccusage@20.0.24` 运行固定版本；最后一种方式需要 Node.js/npm，并可能在本机尚无缓存时下载 ccusage。源码仓库中的 usage-connect 示例插件只需 Python 标准库和示例 fixture，由 Rust 宿主解析结果，不需要另装 ccusage；该示例不会预装在发布安装包中。Python sidecar 使用时另需 Python 3.10+ 与 uv 0.5+。

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

- Gemini CLI 的认证与 GUI 终端渲染仍受本机环境限制；已有 headless lifecycle 证据不能替代认证恢复后的 chat 级 GUI 验收。
- OpenCode 的 headless lifecycle 已取证；当前记录中的 GUI TUI 渲染受环境阻塞，OpenCode 认证 readiness 也需由本机配置决定。
- CodeBuddy 当前为 detection-only，尚不支持启动、终端或 usage 导入。
- 插件 v0.1 仅暴露后端 IPC，没有 UI；权限决策为 observe-only 记录，不构成安全沙箱或访问隔离。
- 插件 IPC 仅有 `plugin_list` 与 `plugin_refresh`。`usage/import` 由 Rust service 调用；skill 在 refresh 时拉取、缓存，再随插件列表返回。源码仓库中的 usage-connect 示例只依赖 Python 标准库与 fixture，发布安装包不预装示例插件。内置 ccusage runner 会依次使用 PATH 上的 `ccusage`、显式配置命令，或 Node.js/npm 提供的固定版本 npx fallback。其他插件需自行提供声明的 adapter 命令。
- 当前仍是 alpha 软件；发布安装包的实际验证情况会在本版本 Release 说明中逐项记录。

## License

MIT
