# ADR-0001 — 桌面壳使用 Tauri 2 而不是 Electron

- **Status**: Accepted（2026-04）
- **Context**: Harness Hub 需要 PTY、进程管理、文件监听、Git 与本机权限控制，同时要长期驻留
  在开发者机器上（含托盘）。候选方案为 Electron、Tauri 2、以及"纯 CLI + 本地 Web"。
- **Decision**: 桌面壳使用 **Tauri 2**。Rust 作为 Control Plane（进程 / PTY / Git / 文件监听 /
  SQLite / 本机权限 / Sidecar 生命周期），前端使用 React + TypeScript + Vite + Tailwind CSS。
- **Alternatives**:
  - Electron：生态最成熟、Node 侧能力最全；但发行包体积与内存占用显著更大，且大量本机能力
    需要在 Node 与 renderer 之间重新搭桥。
  - 纯 CLI + 本地 Web：开发最快；但托盘、原生窗口、PTY 体验和"桌面入口"这个产品定位不匹配。
- **Consequences**:
  - 正向：体积小、启动快、Rust 侧可直接使用 `portable-pty` / `notify` / `rusqlite` 等系统能力。
  - 负向：团队需同时维护 Rust 与 TypeScript；Windows 上需要 MSVC 工具链。
  - Rust 与 Python 之间使用 stdio JSON-RPC，**不为桌面端常驻开放 localhost HTTP 端口**。
- **Evidence**: `src-tauri/` 骨架可编译可测试；`pnpm tauri dev` 可启动窗口。
- **Revisit Conditions**: 若 Rust 侧开发成本导致交付速度长期不可接受，或某关键能力只有 Node 生态具备。
