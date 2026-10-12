//! OpenCode Terminal 生命周期 spike：真实 `opencode.exe` 进真实 PTY，**只测量不推断**。
//!
//! 与 Gemini / Claude 的 spike 同构，但 OpenCode 是本仓库第一个**原生 exe** Harness
//! （Scoop shim，没有 `.cmd` → node 的中间层），因此要单独测它会不会像 shim 那样
//! 停在「横幅输出后无输出、不退出」。
//!
//! **PTY 层不解析任何序列**：本文件只统计字节模式，终端协议仍归 xterm.js
//! （ADR-0009）。这里只做 test-only 的观察与应答。
//!
//! 非 Windows 或没装 OpenCode 的机器直接跳过。

#![cfg(windows)]

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use harness_hub_lib::harness::adapter::HarnessAdapter;
use harness_hub_lib::harness::adapters::opencode::{OpenCodeAdapter, OpenCodeXdgRoots};
use harness_hub_lib::harness::probe::SystemHostProbe;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

const DSR_REQUEST: &[u8] = b"\x1b[6n";
const DSR_REPLY: &[u8] = b"\x1b[1;1R";
const E2E_WORKSPACE: &str = r"D:\HarnessHub-E2E\opencode-terminal";

fn installed_opencode() -> Option<String> {
    let adapter = OpenCodeAdapter::new(
        Arc::new(SystemHostProbe::new()),
        OpenCodeXdgRoots::from_env(),
    );
    let detect = adapter.detect();
    detect
        .installed
        .then(|| detect.binary_path.unwrap_or_default())
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || haystack.len() < needle.len() {
        return 0;
    }
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

/// 一次「起进程 → 读一段时间」的观察。
struct Observation {
    output: Vec<u8>,
    exited: Option<u32>,
}

/// 起真实 OpenCode，读 `deadline`，期间可选地应答 DSR、可选地在某个时刻写入字节。
///
/// spawn 偶发一次瞬态失败时重试一次（与 Gemini spike 的既有处理一致）。
fn observe(
    program: &str,
    args: &[&str],
    answer_dsr: bool,
    deadline: Duration,
    extra_input: Option<(Duration, Vec<u8>)>,
) -> Observation {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 30,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let mut command = CommandBuilder::new(program);
    for argument in args {
        command.arg(argument);
    }
    command.cwd(E2E_WORKSPACE);

    let mut child = match pair.slave.spawn_command(command) {
        Ok(child) => child,
        Err(first) => {
            eprintln!("spawn 第一次失败（瞬态，重试一次）：{first}");
            let mut command = CommandBuilder::new(program);
            for argument in args {
                command.arg(argument);
            }
            command.cwd(E2E_WORKSPACE);
            pair.slave
                .spawn_command(command)
                .expect("spawn opencode（重试后仍失败）")
        }
    };
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let mut writer = pair.master.take_writer().expect("writer");
    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink = Arc::clone(&output);
    let reader_thread = thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 {
                break;
            }
            sink.lock()
                .expect("sink")
                .extend_from_slice(&buffer[..read]);
        }
    });

    let start = Instant::now();
    let mut answered = 0usize;
    let mut sent_extra = false;
    let mut exited = None;

    while start.elapsed() < deadline {
        thread::sleep(Duration::from_millis(120));

        if answer_dsr {
            let requested = count(&output.lock().expect("sink"), DSR_REQUEST);
            while answered < requested {
                let _ = writer.write_all(DSR_REPLY);
                let _ = writer.flush();
                answered += 1;
            }
        }

        if let Some((after, bytes)) = &extra_input {
            if !sent_extra && start.elapsed() > *after {
                let _ = writer.write_all(bytes);
                let _ = writer.flush();
                sent_extra = true;
            }
        }

        if let Ok(Some(status)) = child.try_wait() {
            exited = Some(status.exit_code());
            break;
        }
    }

    let pid = child.process_id();
    if exited.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    // **不 join 读取线程**：与 Claude / Gemini spike 相同的理由 —— 进程被杀后
    // 子进程可能仍持有 PTY。让线程随测试进程退出，并用**定向**树杀清理。
    drop(reader_thread);
    if let Some(pid) = pid {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
    }

    let output = output.lock().expect("sink").clone();
    Observation { output, exited }
}

fn printable(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .collect()
}

fn describe(label: &str, observation: &Observation) -> String {
    let bytes = &observation.output;
    let text = printable(bytes);
    format!(
        "{label}: bytes={} dsr={} alt_screen={} bracketed_paste={} mouse={} hide_cursor={} \
         clear={} exited={:?} printable_head={:?} printable_tail={:?}",
        bytes.len(),
        count(bytes, DSR_REQUEST),
        count(bytes, b"\x1b[?1049h"),
        count(bytes, b"\x1b[?2004h"),
        count(bytes, b"\x1b[?1000h") + count(bytes, b"\x1b[?1002h") + count(bytes, b"\x1b[?1006h"),
        count(bytes, b"\x1b[?25l"),
        count(bytes, b"\x1b[2J") + count(bytes, b"\x1b[H"),
        observation.exited,
        text.chars().take(220).collect::<String>(),
        text.chars()
            .rev()
            .take(400)
            .collect::<String>()
            .chars()
            .rev()
            .collect::<String>(),
    )
}

#[test]
fn real_opencode_in_a_real_pty_reports_what_it_needs() {
    let Some(binary) = installed_opencode() else {
        eprintln!("跳过：本机没有 opencode");
        return;
    };
    assert!(Path::new(E2E_WORKSPACE).is_dir(), "E2E 工作区必须存在");
    eprintln!("opencode binary = {binary}");

    // [1] 不应答任何查询：看它自己会说什么、会不会卡住。
    let silent = observe(&binary, &[], false, Duration::from_secs(8), None);
    eprintln!("[1] 不应答  {}", describe("silent", &silent));

    // [2] 应答 DSR + 一段时间后写一个字符。
    let answered = observe(
        &binary,
        &[],
        true,
        Duration::from_secs(14),
        Some((Duration::from_secs(9), b"x".to_vec())),
    );
    eprintln!("[2] 应答+输入  {}", describe("answered+input", &answered));

    // [3] Ctrl+C×2：TUI 的自然退出路径。
    let double = observe(
        &binary,
        &[],
        true,
        Duration::from_secs(30),
        Some((Duration::from_secs(10), b"\x03\x03".to_vec())),
    );
    eprintln!("[3] Ctrl+C×2  {}", describe("ctrl-c-twice", &double));

    // 实测结论断言：把**已观测到的协议层事实**锁住（ADR-0022：能稳定回归才算支持）。
    assert!(
        binary.ends_with("opencode.exe"),
        "本机实测应选中 Scoop 原生 exe：{binary}"
    );

    // [1] 它会先问终端能力（DSR）并**等**应答：不应答就只有查询序列本身，
    // 不会画出界面。这是「PTY 层必须原样透传、由终端应答」的契约。
    assert!(
        count(&silent.output, DSR_REQUEST) >= 1,
        "OpenCode 会发 DSR 查询"
    );
    assert!(
        silent.output.len() < 64,
        "不应答 DSR 时不得画出界面，实测只有查询序列本身：{} bytes",
        silent.output.len()
    );

    // [2] 应答后画的是**真 TUI**：alternate screen + bracketed paste 同时出现。
    assert!(
        answered.output.len() > 1000,
        "应答 DSR 后必须画出 TUI，实测 {} bytes",
        answered.output.len()
    );
    assert!(
        count(&answered.output, b"\x1b[?1049h") >= 1,
        "真 TUI 应开启 alternate screen"
    );
    assert!(
        count(&answered.output, b"\x1b[?2004h") >= 1,
        "真 TUI 应开启 bracketed paste"
    );

    // [3] Ctrl+C×2 是自然退出路径：干净退出、退出码 0、并依次关闭
    // bracketed paste 与 alternate screen。
    assert_eq!(
        double.exited,
        Some(0),
        "Ctrl+C×2 必须让 TUI 自然退出（实测退出码 0）"
    );
    assert!(
        count(&double.output, b"\x1b[?2004l") >= 1 && count(&double.output, b"\x1b[?1049l") >= 1,
        "退出时必须成对关闭 bracketed paste 与 alternate screen"
    );
}
