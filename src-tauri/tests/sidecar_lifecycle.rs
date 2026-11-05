//! F20 Python sidecar 生命周期真机测试（spec §6 / plan Task 3-4）。
//!
//! 与「夹具预置 Ready」的假测试划清界限：这里从 `SidecarManager::new`
//! （无任何预置进程）开始，`start()` 走完 **真 spawn → 真 stdio 握手**，
//! 用的是仓库里真实的 `python -m harness_hub_runtime`。
//!
//! 覆盖 ROADMAP F20 的五项验收：
//! 1. 启停：`stop()` 之后 `process_alive(pid) == false`；
//! 2. 握手：`initialize` 协商出 protocolVersion / capabilities / runtime；
//! 3. 能力协商：`negotiatedCapabilities` 必须含三个必需能力；
//! 4. 崩溃恢复：外部 kill 后 `status()` 翻 Degraded，`restart()` 回 Ready；
//! 5. 不拖累主功能：解释器缺失时 `start()` 返回 Degraded（不是 Err、不 panic）。
//!
//! 本机没有 `python` 时**明确跳过**（与 `real_plugins.rs` 的惯例一致）；
//! 「解释器路径不存在」与「握手协议不兼容」两条负路径用 mock 模块，不依赖真 runtime。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use harness_hub_lib::sidecar::config::SidecarConfig;
use harness_hub_lib::sidecar::dto::SidecarState;
use harness_hub_lib::sidecar::manager::SidecarManager;
use serde_json::{json, Value};

/// `CARGO_MANIFEST_DIR` 是 `src-tauri/`，往上一级才是仓库根。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 有父目录")
        .to_path_buf()
}

fn runtime_root() -> PathBuf {
    repo_root().join("python")
}

fn python_available() -> bool {
    Command::new("python")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// 没有解释器就跳过，绝不伪装通过。
fn require_python() -> bool {
    if python_available() {
        true
    } else {
        eprintln!("SKIP: PATH 里没有 python，真机 sidecar 测试跳过");
        false
    }
}

fn test_config() -> SidecarConfig {
    SidecarConfig {
        python: PathBuf::from("python"),
        runtime_root: runtime_root(),
        host_version: "0.5.0".to_string(),
    }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    let output = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output();
    match output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\"")),
        Err(_) => false,
    }
}

#[cfg(not(windows))]
fn process_alive(pid: u32) -> bool {
    // Linux 上 `kill -0` 无法区分「存活」与「僵尸」：被 SIGKILL 的 sidecar
    // 子进程在父进程 `wait()` 之前仍是僵尸，kill -0 依旧返回成功。崩溃恢复
    // 用例是「外部 kill」，此时生产代码还没 try_wait()，所以必须把 /proc 里
    // `State: Z (zombie)` 视为已死亡，否则 wait_until_dead 会假阴性（Windows
    // 的 tasklist 没有僵尸语义，本地能过、Linux CI 会挂）。
    if let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) {
        let is_zombie = status
            .lines()
            .find_map(|line| line.strip_prefix("State:"))
            .is_some_and(|value| value.trim_start().starts_with('Z'));
        if is_zombie {
            return false;
        }
    }
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn kill_process(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .output();
}

#[cfg(not(windows))]
fn kill_process(pid: u32) {
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
}

fn wait_until_dead(pid: u32, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_alive(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !process_alive(pid)
}

/// 验收 #1/#2/#3：**真冷启动**（不注入 Ready）。
#[test]
fn cold_start_spawns_real_python_and_negotiates_to_ready() {
    if !require_python() {
        return;
    }

    let manager = SidecarManager::new(test_config());
    let before = manager.status();
    assert_eq!(before.state, SidecarState::Stopped, "{before:?}");
    assert_eq!(before.pid, None);
    assert_eq!(before.runtime, None);
    assert_eq!(before.restart_count, 0);

    let status = manager.start();
    assert_eq!(
        status.state,
        SidecarState::Ready,
        "真冷启动必须到 Ready，last_error={:?}",
        status.last_error
    );
    let runtime = status.runtime.expect("Ready 必须带 runtime 自报信息");
    assert_eq!(runtime.name, "harness-hub-runtime");
    assert_eq!(runtime.version, "0.5.0");
    assert_eq!(runtime.transport, "stdio-jsonrpc");
    assert_eq!(runtime.protocol_version, "1.0");
    assert_eq!(status.protocol_version.as_deref(), Some("1.0"));
    for capability in ["ping", "runtime.info", "shutdown"] {
        assert!(
            status
                .negotiated_capabilities
                .iter()
                .any(|item| item == capability),
            "协商结果缺少必需能力 {capability}：{:?}",
            status.negotiated_capabilities
        );
    }
    assert!(status.pid.is_some(), "Ready 必须带 pid");
    assert_eq!(status.last_error, None);

    manager.shutdown();
}

/// 验收 #2：握手之后 ping / runtime.info 可用，且与协商结果同形状。
#[test]
fn ping_and_runtime_info_work_after_handshake() {
    if !require_python() {
        return;
    }

    let manager = SidecarManager::new(test_config());
    let status = manager.start();
    assert_eq!(status.state, SidecarState::Ready, "{status:?}");

    let pong = manager.call("ping", &json!({})).expect("ping 调用失败");
    assert_eq!(pong.get("pong").and_then(Value::as_bool), Some(true));

    let info = manager
        .call("runtime.info", &json!({}))
        .expect("runtime.info 调用失败");
    assert_eq!(
        info.get("name").and_then(Value::as_str),
        Some("harness-hub-runtime")
    );
    assert_eq!(
        info.get("protocolVersion").and_then(Value::as_str),
        Some("1.0")
    );
    assert_eq!(info.get("version").and_then(Value::as_str), Some("0.5.0"));
    assert!(
        info.get("methods").and_then(Value::as_array).is_some(),
        "runtime.info 必须带 methods 数组：{info:?}"
    );
    // initialize.result.runtime 与 runtime.info 是同一个构造函数产出的。
    let negotiated = status.runtime.expect("Ready 带 runtime");
    assert_eq!(
        info.get("capabilities").cloned(),
        Some(json!(negotiated.capabilities)),
        "两处 runtime 形状必须一致"
    );

    manager.shutdown();
}

/// 验收 #1：`stop()` 之后进程确实消失，且重复 stop 幂等。
#[test]
fn stop_terminates_process_and_is_idempotent() {
    if !require_python() {
        return;
    }

    let manager = SidecarManager::new(test_config());
    let started = manager.start();
    assert_eq!(started.state, SidecarState::Ready, "{started:?}");
    let pid = started.pid.expect("Ready 带 pid");
    assert!(process_alive(pid), "start() 后进程 {pid} 应存活");

    let stopped = manager.stop();
    assert_eq!(stopped.state, SidecarState::Stopped, "{stopped:?}");
    assert_eq!(stopped.pid, None);
    assert!(
        wait_until_dead(pid, Duration::from_secs(5)),
        "stop() 之后进程 {pid} 仍存活"
    );
    assert!(!process_alive(pid), "stop() 之后进程 {pid} 仍存活");

    assert_eq!(manager.stop().state, SidecarState::Stopped, "stop 必须幂等");
    assert_eq!(manager.status().state, SidecarState::Stopped);
}

/// 验收 #4：进程被外部杀掉后 status() 检出 Degraded，restart() 恢复且计数自增。
#[test]
fn crash_is_detected_as_degraded_and_restart_recovers() {
    if !require_python() {
        return;
    }

    let manager = SidecarManager::new(test_config());
    let started = manager.start();
    assert_eq!(started.state, SidecarState::Ready, "{started:?}");
    let pid = started.pid.expect("Ready 带 pid");

    kill_process(pid);
    assert!(
        wait_until_dead(pid, Duration::from_secs(5)),
        "外部 kill 后进程 {pid} 仍存活"
    );

    let degraded = manager.status();
    assert_eq!(
        degraded.state,
        SidecarState::Degraded,
        "status() 必须检出死亡并翻 Degraded：{degraded:?}"
    );
    assert!(
        degraded
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("进程已退出"),
        "Degraded 原因应点名进程退出：{degraded:?}"
    );

    let recovered = manager.restart();
    assert_eq!(
        recovered.state,
        SidecarState::Ready,
        "restart() 必须重新拉起并握手：{recovered:?}"
    );
    assert!(
        recovered.restart_count >= 1,
        "restart() 必须自增 restart_count：{recovered:?}"
    );
    assert_ne!(recovered.pid, Some(pid), "restart 必须换一个新进程");

    manager.shutdown();
}

/// 验收 #5：解释器不存在时 start() 返回 Degraded，而不是 Err / panic。
#[test]
fn missing_interpreter_degrades_without_returning_error() {
    let manager = SidecarManager::new(SidecarConfig {
        python: repo_root().join("no-such-python-interpreter-9f3b.exe"),
        runtime_root: runtime_root(),
        host_version: "0.5.0".to_string(),
    });

    let status = manager.start();
    assert_eq!(status.state, SidecarState::Degraded, "{status:?}");
    assert!(
        status
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("启动失败"),
        "Degraded 应带可读的启动失败原因：{status:?}"
    );
    // 主 Harness 功能不受影响：状态查询与 stop 仍然可用。
    assert_eq!(manager.status().state, SidecarState::Degraded);
    assert_eq!(manager.stop().state, SidecarState::Stopped);
}

/// 握手负路径：sidecar 自报不兼容的 protocolVersion → Degraded（不是崩溃）。
#[test]
fn incompatible_protocol_version_degrades() {
    if !require_python() {
        return;
    }

    let manager = SidecarManager::new(SidecarConfig {
        python: PathBuf::from("python"),
        runtime_root: repo_root().join("src-tauri/tests/fixtures/sidecar/bad_protocol"),
        host_version: "0.5.0".to_string(),
    });

    let status = manager.start();
    assert_eq!(status.state, SidecarState::Degraded, "{status:?}");
    assert!(
        status
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("握手失败"),
        "应点名握手失败：{status:?}"
    );

    manager.shutdown();
}
