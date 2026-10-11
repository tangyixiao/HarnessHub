//! Gemini Terminal 生命周期 spike：真实 `gemini.cmd` 进真实 PTY，**只测量不推断**。
//!
//! 2026-10-11 实测结论（gemini-cli 0.63.0，`D:\npm-global\gemini.cmd`）：
//!
//! 1. `.cmd` 可以直接作为 `program`（CreateProcessW + ConPTY）；
//! 2. **发送 DSR（`ESC[6n`）并等待**：不应答就只有 4 个字节然后永久停住，与 Claude 同型；
//! 3. 无 alternate screen；bracketed paste（`?2004h`）开启；mouse 关闭；
//!    另有 kitty keyboard（`?u`）与 XTVERSION（`>q`）查询，PTY 层一律原样透传；
//! 4. 未信任目录先弹「Do you trust the files in this folder?」三选一对话框；
//!    `--skip-trust` 可跳过它直达认证对话框（本机认证被 Google 拒绝：
//!    「This client is no longer supported … migrate to Antigravity」），
//!    两个对话框都验证了 Ctrl+C×2 → 退出码 0 的自然退出路径；
//! 5. 对话框级输入（方向键移动单选项）有可见的字节回应；chat 级交互在本机
//!    因认证失效不可达 —— 按 ADR-0005 属 readiness，不回写 capability。
//!
//! **PTY 层不解析任何序列**：本文件只统计字节模式，终端协议仍归 xterm.js
//! （ADR-0009）。这里只做 test-only 的观察与应答。
//!
//! 非 Windows 或没装 Gemini 的机器直接跳过。

#![cfg(windows)]

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use harness_hub_lib::harness::adapter::HarnessAdapter;
use harness_hub_lib::harness::adapters::gemini_cli::GeminiCliAdapter;
use harness_hub_lib::harness::probe::SystemHostProbe;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

const DSR_REQUEST: &[u8] = b"\x1b[6n";
const DSR_REPLY: &[u8] = b"\x1b[1;1R";
const E2E_WORKSPACE: &str = r"D:\HarnessHub-E2E\gemini-terminal";

fn installed_gemini() -> Option<String> {
    let adapter = GeminiCliAdapter::new(Arc::new(SystemHostProbe::new()), None);
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

/// 起真实 Gemini，读 `deadline`，期间可选地应答 DSR、可选地在某个时刻写入字节。
///
/// 实测 CreateProcess 偶发一次「os error 2」瞬态失败（后续多次 spawn 均正常），
/// 这里对 spawn 做一次重试，并在输出里记录。
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
                .expect("spawn gemini（重试后仍失败）")
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
    // **不 join 读取线程**：与 Claude spike 相同的理由 —— shim 被杀后 node 子进程
    // 可能仍持有 PTY。让线程随测试进程退出，并用**定向**树杀清理。
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
fn real_gemini_in_a_real_pty_reports_what_it_needs() {
    let Some(binary) = installed_gemini() else {
        eprintln!("跳过：本机没有 gemini");
        return;
    };
    assert!(Path::new(E2E_WORKSPACE).is_dir(), "E2E 工作区必须存在");
    eprintln!("gemini binary = {binary}");

    // [1] --skip-trust + 不应答任何查询：看它自己会说什么、会不会卡住。
    let silent = observe(
        &binary,
        &["--skip-trust"],
        false,
        Duration::from_secs(8),
        None,
    );
    eprintln!("[1] skip-trust 不应答  {}", describe("silent", &silent));

    // [2] --skip-trust + 应答 DSR + 一段时间后写一个字符。
    let answered = observe(
        &binary,
        &["--skip-trust"],
        true,
        Duration::from_secs(14),
        Some((Duration::from_secs(9), b"x".to_vec())),
    );
    eprintln!(
        "[2] skip-trust 应答+输入  {}",
        describe("answered+input", &answered)
    );

    // [3] 不带 --skip-trust + 应答 DSR：未信任目录的对话框形态（内容只记录不断言，
    //     目录一旦被用户信任，这里看到的可能是聊天/认证画面而不是信任框）。
    let trust = observe(&binary, &[], true, Duration::from_secs(8), None);
    eprintln!("[3] 未信任目录  {}", describe("trust-dialog", &trust));

    // [4] 应答 DSR + Ctrl+C×2：TUI 的自然退出路径（两个对话框状态下都实测过）。
    let quit = observe(
        &binary,
        &["--skip-trust"],
        true,
        Duration::from_secs(30),
        Some((Duration::from_secs(10), b"\x03".to_vec())),
    );
    eprintln!("[4] Ctrl+C  {}", describe("ctrl-c", &quit));
    let double = observe(
        &binary,
        &["--skip-trust"],
        true,
        Duration::from_secs(30),
        Some((Duration::from_secs(10), b"\x03\x03".to_vec())),
    );
    eprintln!("[5] Ctrl+C×2  {}", describe("ctrl-c-twice", &double));

    // 实测结论断言（只断言协议层的稳定事实）。
    assert!(
        !silent.output.is_empty() || !answered.output.is_empty(),
        "真实 Gemini 必须有输出"
    );
    assert!(
        count(&silent.output, DSR_REQUEST) >= 1,
        "Gemini 会发送 DSR（ESC[6n）；不应答时只有这些字节"
    );
    assert!(
        silent.output.len() < 64,
        "不应答 DSR 时 Gemini 停在等待（实测 4 字节），不应画出 TUI"
    );
    assert!(
        count(&answered.output, b"\x1b[?2004h") >= 1,
        "应答 DSR 之后必须画出 TUI（bracketed paste 开启是真 TUI 标志）"
    );
    assert!(
        !trust.output.is_empty() && count(&trust.output, DSR_REQUEST) >= 1,
        "未信任目录的观察同样以 DSR 查询开始"
    );
    assert!(
        binary.ends_with("gemini.cmd"),
        "本机实测应选中 gemini.cmd：{binary}"
    );
    assert!(
        double.exited.is_some(),
        "Ctrl+C×2 必须让 TUI 自然退出（实测两个对话框状态都是退出码 0）"
    );
    eprintln!("[5] Ctrl+C×2 自然退出，exit_code={:?}", double.exited);
}
