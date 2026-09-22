//! Windows PTY 行为实测（Task 4 的设计依据，保留为回归测试）。
//!
//! 为什么必须实测而不能推断：`std::process::Command` 在 Windows 上会自己通过
//! `cmd.exe` 处理 `.cmd` / `.bat`，但 **portable-pty 是另一条代码路径**
//! （ConPTY + CreateProcess），两者行为不能互相推断。
//!
//! 2026-09-22 在本机的实测结论（portable-pty 0.9.0 / Windows）：
//!
//! ```text
//! A. CommandBuilder("D:\npm-global\codex.cmd") 直接 spawn
//!    → spawned = true, exit_code = 0, 输出含 "codex-cli 0.152.1"      ✅ 可用
//!
//! B. cmd.exe /D /S /C "\"D:\npm-global\codex.cmd\" --version"
//!    → spawned = true, exit_code = 1
//!      输出：'\"D:\npm-global\codex.cmd\"' 不是内部或外部命令           ❌ 不可用
//! ```
//!
//! 由此得出两条设计结论：
//!
//! 1. `.cmd` **不需要**包命令解释器 —— `CodexAdapter::build_launch_spec()` 直接给
//!    可执行文件本身即可（与 `harness::launch::program_for` 一致）。
//!    `/S` 的引号规则会把朴素拼接的引号变成字面量，这也再一次说明：
//!    命令字符串绝不能靠手拼。
//! 2. **PTY 宿主必须应答 DSR（`ESC[6n` 光标位置查询）**，否则交互式 CLI 会永久等待。
//!    第一次跑这个 spike 时两条路径都超时，输出只有 `ESC[6n`；补上应答后才拿到 exit 0。
//!    真实 UI 里这一步由 xterm.js 之类的终端模拟器完成。
//!
//! 条件式：非 Windows 或没装 Codex 的机器直接跳过（CI 上不会失败）。

#![cfg(windows)]

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use harness_hub_lib::harness::adapter::HarnessAdapter;
use harness_hub_lib::harness::adapters::codex::CodexAdapter;
use harness_hub_lib::harness::probe::SystemHostProbe;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

/// 一次 spawn 尝试的观察结果。
struct Attempt {
    spawned: bool,
    exit_code: Option<u32>,
    timed_out: bool,
    output: String,
    error: Option<String>,
}

/// 在真实 PTY 里跑一次，最多等 `timeout`。
///
/// **关键**：真终端必须回答子进程的 DSR（`ESC[6n` 光标位置查询），
/// 否则交互式 CLI 会一直等下去。这里手动应答 `ESC[1;1R`，
/// 真实 UI 里这一步由 xterm.js 之类的终端模拟器完成。
fn attempt(program: &str, args: &[String], timeout: Duration) -> Attempt {
    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(PtySize {
        rows: 24,
        cols: 120,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pair) => pair,
        Err(error) => {
            return Attempt {
                spawned: false,
                exit_code: None,
                timed_out: false,
                output: String::new(),
                error: Some(format!("openpty 失败：{error}")),
            };
        }
    };

    let mut builder = CommandBuilder::new(program);
    for arg in args {
        builder.arg(arg);
    }

    let mut child = match pair.slave.spawn_command(builder) {
        Ok(child) => child,
        Err(error) => {
            return Attempt {
                spawned: false,
                exit_code: None,
                timed_out: false,
                output: String::new(),
                error: Some(format!("spawn 失败：{error}")),
            };
        }
    };
    // 父进程必须丢掉 slave，否则子进程拿不到 EOF。
    drop(pair.slave);

    let mut reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(error) => {
            return Attempt {
                spawned: true,
                exit_code: None,
                timed_out: false,
                output: String::new(),
                error: Some(format!("取读端失败：{error}")),
            };
        }
    };
    let mut writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(error) => {
            return Attempt {
                spawned: true,
                exit_code: None,
                timed_out: false,
                output: String::new(),
                error: Some(format!("取写端失败：{error}")),
            };
        }
    };

    let buffer = Arc::new(Mutex::new(String::new()));
    let buffer_for_reader = Arc::clone(&buffer);
    let reader_thread = thread::spawn(move || {
        let mut chunk = [0u8; 1024];
        while let Ok(read) = reader.read(&mut chunk) {
            if read == 0 {
                break;
            }
            buffer_for_reader
                .lock()
                .expect("buffer")
                .push_str(&String::from_utf8_lossy(&chunk[..read]));
        }
    });

    let deadline = Instant::now() + timeout;
    let mut exit_code = None;
    let mut timed_out = false;
    let mut answered_dsr = 0usize;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = Some(status.exit_code());
                break;
            }
            Ok(None) => {}
            Err(error) => {
                let captured = buffer.lock().expect("buffer").clone();
                return Attempt {
                    spawned: true,
                    exit_code: None,
                    timed_out: false,
                    output: captured,
                    error: Some(format!("try_wait 失败：{error}")),
                };
            }
        }

        // 子进程每问一次光标位置，就答一次 —— 否则它会一直等。
        let pending = buffer.lock().expect("buffer").matches("\u{1b}[6n").count();
        while answered_dsr < pending {
            let _ = writer.write_all(b"\x1b[1;1R");
            let _ = writer.flush();
            answered_dsr += 1;
        }

        if Instant::now() >= deadline {
            timed_out = true;
            let _ = child.kill();
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    // 给读线程一点时间收尾，然后杀掉/等待
    thread::sleep(Duration::from_millis(300));
    let _ = child.kill();
    drop(reader_thread);

    // 先把内容取出来，避免临时 MutexGuard 活到函数末尾。
    let captured = buffer.lock().expect("buffer").clone();

    Attempt {
        spawned: true,
        exit_code,
        timed_out,
        output: captured,
        error: None,
    }
}

fn codex_binary() -> Option<String> {
    let adapter = CodexAdapter::new(Arc::new(SystemHostProbe::new()));
    let detected = adapter.detect();

    if detected.installed {
        detected.binary_path
    } else {
        None
    }
}

#[test]
fn windows_cmd_shim_is_spawned_directly_and_needs_dsr_answers() {
    let Some(binary) = codex_binary() else {
        eprintln!("跳过：本机没有检测到 Codex");
        return;
    };
    eprintln!("检测到的 binary = {binary}");

    // 设计所依赖的那条路径：直接把 .cmd 交给 portable-pty
    let direct = attempt(&binary, &["--version".to_string()], Duration::from_secs(20));

    // 对照路径：朴素拼引号的 cmd.exe 包装（记录它为什么不能用）
    let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
    let naive_wrapped = attempt(
        &comspec,
        &[
            "/D".to_string(),
            "/S".to_string(),
            "/C".to_string(),
            format!("\"{binary}\" --version"),
        ],
        Duration::from_secs(20),
    );

    for (label, result) in [
        ("A 直接 spawn .cmd", &direct),
        ("B cmd.exe /D /S /C（朴素引号）", &naive_wrapped),
    ] {
        eprintln!("--- {label} ---");
        eprintln!("  spawned   = {}", result.spawned);
        eprintln!("  exit_code = {:?}", result.exit_code);
        eprintln!("  timed_out = {}", result.timed_out);
        eprintln!("  error     = {:?}", result.error);
        eprintln!("  output    = {:?}", result.output.trim());
    }

    // 1) 直接 spawn 必须可用：这是 build_launch_spec 直连 .cmd 的依据
    assert!(
        direct.spawned,
        "portable-pty 无法 spawn {}：{:?}",
        binary, direct.error
    );
    assert!(
        !direct.timed_out,
        "直接 spawn 超时 —— 说明 PTY 宿主没有应答 DSR（ESC[6n）"
    );
    assert_eq!(
        direct.exit_code,
        Some(0),
        "codex --version 应以 0 退出，实际输出：{}",
        direct.output
    );
    assert!(
        direct.output.contains("codex-cli"),
        "输出里应含版本号，实际：{:?}",
        direct.output
    );

    // 2) 朴素引号的 cmd 包装不可用 —— 记录这个坑，避免以后有人「顺手包一层 shell」
    assert_ne!(
        naive_wrapped.exit_code,
        Some(0),
        "如果这条开始成功，说明引号规则变了，需要重新评估 build_launch_spec"
    );
}

/// **环境继承回归**：`portable-pty` 在 Windows 上曾有「吞掉自定义 PATH」的报告
/// （wezterm#4205）。Codex 启动后会去调用 `node` / `git`，
/// PATH 丢了就会以非常难排查的方式失败，所以这里显式锁住。
#[test]
fn pty_child_inherits_path_and_can_locate_node_and_git() {
    let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
    let run_cmd = |command: String| {
        attempt(
            &comspec,
            &["/D".to_string(), "/C".to_string(), command],
            Duration::from_secs(20),
        )
    };

    for tool in ["node", "git"] {
        let result = run_cmd(format!("where {tool}"));

        assert_eq!(
            result.exit_code,
            Some(0),
            "PTY 子进程在 PATH 里找不到 {tool}，输出：{:?}",
            result.output
        );
        assert!(
            result.output.to_lowercase().contains(tool),
            "where {tool} 的输出应包含路径：{:?}",
            result.output
        );
    }

    // 不只是「看得到」，还要「跑得起来」
    let node = run_cmd("node --version".to_string());
    assert_eq!(
        node.exit_code,
        Some(0),
        "node 无法执行，输出：{:?}",
        node.output
    );
    assert!(
        node.output.contains('v'),
        "node --version 应输出版本号：{:?}",
        node.output
    );
}
