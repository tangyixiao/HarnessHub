# ADR-0008 — 用结构化 `LaunchSpec` 描述启动，PTY 层不得拼 shell 命令字符串

- **Status**: Accepted（2026-09-22，Task 4 之前冻结接口）
- **Context**:
  下一步（Task 4）要第一次把「Harness installation + 真实 OS 进程 + PTY」接起来。
  最自然的写法通常是这样，而且每一条都会出事：

  ```rust
  // 反例 1：Adapter 自己拼命令字符串
  format!("{} {}", binary, args.join(" "))
  // 反例 2：交给通用 shell
  Command::new("cmd").args(["/C", &format!("codex {args}")])
  ```

  本机的真实情况已经足够说明问题：

  ```text
  D:\npm-global\codex        ← POSIX bash 脚本，Windows 无法 CreateProcess
  D:\npm-global\codex.cmd    ← 可直接执行
  D:\npm-global\codex.ps1    ← 必须 pwsh -File（PATHEXT 不含 .PS1）
  ```

  而且后续还要面对：路径含空格、参数转义规则、shell 注入面、Linux binary、
  WSL、SSH、Container —— 这些差异如果散落在 PTY / Session / UI 层，会变成
  「谁都能拼一条命令」的局面。

- **Decision**:

  1. **Adapter 负责生成结构化启动描述**：

     ```rust
     pub struct LaunchSpec {
         pub program: PathBuf,
         pub args: Vec<String>,
         pub cwd: Option<PathBuf>,
         pub env: Vec<(String, String)>,
         pub runtime_target_id: String,
     }

     trait HarnessAdapter {
         fn build_launch_spec(&self, request: LaunchRequest) -> Result<LaunchSpec>;
     }
     ```

  2. **平台差异只在这一层解决**：`harness::launch::program_for()` 决定
     「检测到的可执行文件 → 真正要执行的 program + 前置参数」：

     | 检测到的文件                         | program    | 前置参数                          |
     | ------------------------------------ | ---------- | --------------------------------- |
     | `codex.cmd` / `codex.exe` / 无扩展名 | 该文件本身 | 无                                |
     | `codex.ps1`                          | `pwsh`     | `-NoLogo -NoProfile -File <脚本>` |

  3. **PTY 层只认识 `LaunchSpec`**：它不知道什么是 Codex，也不允许拼字符串。
     它的职责是「给我一个结构化描述，我负责运行」。

  4. **Adapter 不再拥有 `launch()` / `resume()`**：进程与 PTY 的生命周期归
     Control Plane（PTY 层）。原先 `HarnessAdapter::launch()` 返回
     `ProcessHandle` 的设计被移除 —— 那会让 Adapter 反过来依赖进程管理。

  5. `LaunchSpec::describe()` 只用于日志与排障，**不得**被拿去执行。

- **Alternatives**:
  - Adapter 直接 `spawn` 进程：平台细节与进程管理耦合在适配器里，PTY 层无法统一
    处理读写/尺寸/退出码。
  - 通用 shell 包装（`cmd /C`、`sh -c`）：实现最快，但引入转义与注入问题，
    而且无法表达 WSL / SSH 的启动方式。
  - 只传 `program: String` 不传 args：调用方仍需自己拼参数，等于没有解决问题。
- **Consequences**:
  - Task 4 的接线顺序被固定为：

    ```text
    installation → build_launch_spec → PTY spawn → mark_running
                                              ↘ spawn 失败 → fail
    进程退出 → finish(exit_code)
    ```

  - `capabilities.launch` 在 Task 4 端到端验收通过前保持 `false`：
    `build_launch_spec` 只是启动路径的一半。
  - 未来接入 WSL / SSH 时，只需让对应的 Adapter 产出不同的 `LaunchSpec`
    （例如 `program = wsl`，`args = ["-d", "Ubuntu", "--", ...]`），PTY 层无需改动。
- **Evidence**: `src-tauri/src/harness/launch.rs`（`program_for` 与其测试）、
  `CodexAdapter::build_launch_spec` 的 5 条测试（未安装 / .cmd 直连 / .ps1 走 pwsh /
  空 cwd / runtime target 传递）。
- **Revisit Conditions**: 当需要交互式输入的准备阶段（例如先登录再启动）或
  需要多段命令串联时，评估是否扩展为「spec 列表」而不是退回字符串拼接。
