# ADR-0009 — PTY 层是 raw transport，终端协议归终端模拟器

- **Status**: Accepted（2026-09-22，Task 4）
- **Context**:
  第一次用 `portable-pty` 跑真实 Codex 时，两条启动路径（直接 spawn `.cmd`、包一层
  `cmd.exe`）**都超时 20 秒**，输出只有 `ESC[6n` —— 那是终端的 DSR（Device Status
  Report）光标位置查询，子进程在等终端回答 `ESC[<row>;<col>R`。
  补上应答后立刻拿到 `exit 0` 与 `codex-cli 0.152.1`（见 `tests/windows_pty_spike.rs`）。

  于是出现一个诱人的做法：既然 `ESC[6n` 会阻塞，那就在 PTY 后端里顺手识别并回
  `ESC[1;1R`。**这条路必须堵死**：一旦 PTY 层开始解析终端协议，它就会一步步变成
  半吊子 terminal emulator —— 接下来是 OSC（窗口标题、超链接）、鼠标模式、
  bracketed paste、颜色查询、光标形状……

- **Decision**:

  1. **生产 PTY 层只搬字节**，不做任何终端协议解析或应答：

     ```text
     负责：spawn / read bytes / write bytes / resize / kill / wait / pid
     不负责：terminal escape parsing、DSR/CSI/OSC 应答、Codex 语义、
            命令字符串拼装、Session 落库策略
     ```

     禁止在 `PortablePtyBackend` 中出现 `ESC[6n → ESC[1;1R` 之类的硬编码。

  2. **DSR 应答属于终端模拟器**：

     ```text
     GUI 路径：PTY bytes → Tauri Channel → xterm.js
                              ↑ input IPC（xterm.js 依当前光标位置回应）
     无头测试：PTY bytes → test-only HeadlessTerminalResponder
     ```

  3. GUI 侧必须 **ready-before-spawn**：先建好 xterm、挂上 channel 回调、
     挂上 `onData` 写回路径与 resize，**最后**才 `invoke(start_terminal)`。
     否则 Codex 首屏的 DSR 可能早于 responder 就绪，界面直接卡住。

  4. 输出保持**原始 bytes**：不经 `String::from_utf8_lossy`。
     xterm.js `write(Uint8Array)` 自带跨 chunk 的有状态 UTF-8 解码，
     正好适配 PTY 的任意切块；Rust 侧也绝不按字符边界重新切分。

- **Alternatives**:
  - 在 PTY 后端应答 DSR：实现最快，但把传输层变成终端模拟器，且无法覆盖
    OSC/鼠标/粘贴等后续需求。
  - 启动时给子进程 `TERM=dumb` 规避查询：会改变 Harness 自身行为，
    是「为了迁就宿主而篡改被观察对象」，且不保证所有 CLI 都遵守。
  - 无头测试改用管道而不是 PTY：那样就测不到真实 PTY 行为（本 ADR 的起因）。
- **Consequences**:
  - 无头 E2E 必须自带 responder（测试专用，不进生产路径）。
  - 终端渲染能力（颜色、光标、鼠标）完全取决于前端终端模拟器，
    Rust 侧不承担这部分责任。
  - Channel 的 `Output` 事件携带字节数组；未来若吞吐成为瓶颈，
    可切换到 Tauri 的 raw body 通道，而不改变分层。
- **Evidence**: `tests/windows_pty_spike.rs` 的两条实测记录；
  `src-tauri/src/pty/backend.rs` 的负责/不负责清单；
  `pty::manager` 的「原始字节与跨 chunk UTF-8 完整」测试。
- **Revisit Conditions**: 当需要服务端渲染或录制终端画面（session replay）时，
  评估引入一个**独立的**终端模拟器组件，而不是把解析塞回 PTY 层。
