//! Task 7B spike：真实 `claude.cmd` 进真实 PTY，**只测量不推断**。
//!
//! 要回答的问题（全部用实测输出回答，不用「Codex 也这样」推断）：
//!
//! 1. `.cmd` 能否直接作为 `program`（不包命令解释器）；
//! 2. 是否要求真实 TTY；
//! 3. 是否发送 DSR（`ESC[6n`）等设备查询；不发应答会怎样；
//! 4. 是否进入 alternate screen（`?1049h`）；
//! 5. 是否使用 bracketed paste（`?2004h`）/ mouse mode（`?1000/1002/1003/1006h`）；
//! 6. 应答 DSR 后是否继续推进（TUI 真的画出来）；
//! 7. 从 GUI 侧写入字节，是否产生新的可观察输出。
//!
//! **PTY 层不解析任何序列**：本文件只统计字节模式，终端协议仍归 xterm.js
//! （ADR-0009）。这里只做 test-only 的观察与应答。
//!
//! 非 Windows 或没装 Claude 的机器直接跳过。

#![cfg(windows)]

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use harness_hub_lib::harness::adapter::HarnessAdapter;
use harness_hub_lib::harness::adapters::claude_code::ClaudeCodeAdapter;
use harness_hub_lib::harness::probe::SystemHostProbe;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

const DSR_REQUEST: &[u8] = b"\x1b[6n";
const DSR_REPLY: &[u8] = b"\x1b[1;1R";

fn installed_claude() -> Option<String> {
    let adapter = ClaudeCodeAdapter::new(Arc::new(SystemHostProbe::new()));
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
    errors: Vec<String>,
}

/// 起真实 Claude，读 `deadline`，期间可选地应答 DSR、可选地在某个时刻写入字节。
fn observe(
    program: &str,
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
    command.cwd(std::env::temp_dir().to_string_lossy().to_string());

    let mut child = pair.slave.spawn_command(command).expect("spawn claude");
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
    // **不 join 读取线程**：实测 `.cmd` shim 被杀后，它启动的 node 子进程可能仍持有
    // PTY，读取会一直阻塞。这里让线程随测试进程退出，并用**定向**树杀清理
    // （`/T` 杀整棵树、按 PID 指定，绝不安杀所有 node）。
    drop(reader_thread);
    if let Some(pid) = pid {
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .output();
    }

    let output = output.lock().expect("sink").clone();
    Observation {
        output,
        exited,
        errors: Vec::new(),
    }
}

fn describe(label: &str, observation: &Observation) -> String {
    let bytes = &observation.output;
    let text = String::from_utf8_lossy(bytes);
    let printable: String = text
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .collect();
    format!(
        "{label}: bytes={} dsr={} alt_screen={} bracketed_paste={} mouse={} hide_cursor={} \
         clear={} exited={:?} printable_head={:?}",
        bytes.len(),
        count(bytes, DSR_REQUEST),
        count(bytes, b"\x1b[?1049h"),
        count(bytes, b"\x1b[?2004h"),
        count(bytes, b"\x1b[?1000h") + count(bytes, b"\x1b[?1002h") + count(bytes, b"\x1b[?1006h"),
        count(bytes, b"\x1b[?25l"),
        count(bytes, b"\x1b[2J") + count(bytes, b"\x1b[H"),
        observation.exited,
        printable.chars().take(160).collect::<String>(),
    )
}

#[test]
fn real_claude_in_a_real_pty_reports_what_it_needs() {
    let Some(binary) = installed_claude() else {
        eprintln!("跳过：本机没有 claude");
        return;
    };
    eprintln!("claude binary = {binary}");

    // 第一段：不应答任何查询，看它自己会说什么、会不会卡住。
    let silent = observe(&binary, false, Duration::from_secs(6), None);
    eprintln!("[1] 不应答 DSR  {}", describe("silent", &silent));

    // 第二段：应答 DSR，并在一段时间后从「GUI 侧」写入一个字符，看是否产生新输出。
    let answered = observe(
        &binary,
        true,
        Duration::from_secs(14),
        Some((Duration::from_secs(9), b"x".to_vec())),
    );
    eprintln!("[2] 应答 DSR  {}", describe("answered+input", &answered));

    // 实测结论断言（全部基于上面的真实字节）。
    assert!(
        !silent.output.is_empty() || !answered.output.is_empty(),
        "真实 Claude 必须有输出"
    );
    assert!(!answered.output.is_empty(), "应答 DSR 之后必须继续产生输出");
    assert!(
        answered.output.len() >= silent.output.len(),
        "应答 DSR 不应减少输出（silent={} answered={}）",
        silent.output.len(),
        answered.output.len()
    );
    // `.cmd` 直接作为 program 就能起来（否则 spawn/读取都会失败）。
    assert!(
        binary.ends_with("claude.cmd"),
        "本机实测应选中 claude.cmd：{binary}"
    );
}
