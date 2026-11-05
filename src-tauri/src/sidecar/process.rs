//! 单个 Python sidecar 子进程的监督：spawn / 握手 / 调用 / 关闭（F20 spec §4.3）。
//!
//! 进程模型与 `plugins::supervisor` 同构（ADR-0006 / ADR-0013）：
//! 行分隔 JSON-RPC 2.0 over stdio、Windows 用 Job Object 管进程树、
//! 坏 stdout 行不杀读线程、stderr 末尾 64 KiB 留给诊断。
//! **本模块是领域模块，不依赖任何 Tauri GUI 类型。**
//!
//! 与插件监督器的两处刻意差异：
//! 1. 复用 `crate::plugins::rpc` 的编解码器，不另写一套（spec §2）；
//! 2. F20 不提供 Host API：sidecar 反向发来的 Request / Notification 一律丢弃。

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::{Error, Result};
use crate::plugins::rpc::{self, Incoming};
use crate::sidecar::config::{
    SidecarConfig, PROTOCOL_VERSION, REQUIRED_CAPABILITIES, RUNTIME_NAME,
};
use crate::sidecar::dto::RuntimeInfo;

/// 握手超时：10s 足够 Python 冷启动，又不至于让 UI 长时间无反馈。
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// 单次方法调用超时。
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// stderr 环形缓冲上限（64 KiB，防 stderr 洪水打爆宿主内存）。
pub const STDERR_LIMIT: usize = 64 * 1024;
/// 关闭时先礼后兵的时间窗：关 stdin 后等它自己退出这么久，再强杀进程树。
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// 取版本号 `.` 之前的主号；握手只比较主号，小版本允许独立演进。
fn major(version: &str) -> &str {
    version.split('.').next().unwrap_or(version)
}

/// stderr 环形缓冲（与 supervisor 私有类型同名但独立实现，不动既有模块）。
#[derive(Debug, Default)]
struct RingBuffer {
    bytes: VecDeque<u8>,
}

impl RingBuffer {
    fn push(&mut self, chunk: &[u8]) {
        if chunk.len() >= STDERR_LIMIT {
            self.bytes.clear();
            self.bytes.extend(&chunk[chunk.len() - STDERR_LIMIT..]);
        } else {
            let overflow = (self.bytes.len() + chunk.len()).saturating_sub(STDERR_LIMIT);
            self.bytes.drain(..overflow);
            self.bytes.extend(chunk);
        }
    }

    fn tail(&self, max_lines: usize) -> String {
        let bytes: Vec<u8> = self.bytes.iter().copied().collect();
        let text = String::from_utf8_lossy(&bytes);
        let mut lines: Vec<&str> = text.lines().rev().take(max_lines).collect();
        lines.reverse();
        lines.join("\n")
    }
}

/// 一次 `initialize` 握手的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct HandshakeInfo {
    pub runtime: RuntimeInfo,
    pub negotiated_capabilities: Vec<String>,
    pub protocol_version: String,
}

/// 一个已 spawn 的 Python sidecar 进程。
pub struct SidecarProcess {
    child: Child,
    /// `None` = stdin 已关闭（shutdown 之后）。
    stdin: Option<ChildStdin>,
    lines: Receiver<Incoming>,
    stderr: Arc<Mutex<RingBuffer>>,
    next_id: u64,
    /// 超时 / 崩溃后置位，后续调用直接失败，避免对着死流继续写。
    broken: Option<String>,
    #[cfg(windows)]
    containment: Option<crate::pty::containment::Containment>,
}

impl SidecarProcess {
    /// spawn 进程（不做握手）。握手见 [`SidecarProcess::handshake`]。
    ///
    /// 启动命令固定 `<python> -m harness_hub_runtime`，`PYTHONPATH` 指向 `runtime_root`，
    /// **不经 shell、不拼接用户输入**。
    pub fn spawn(config: &SidecarConfig) -> Result<Self> {
        let mut command = Command::new(&config.python);
        command
            .args(["-m", "harness_hub_runtime"])
            .env("PYTHONPATH", &config.runtime_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // 后台进程不该弹黑框。
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command.spawn().map_err(|error| {
            Error::SidecarSpawn(format!(
                "{} 启动失败（PYTHONPATH={}）：{error}",
                config.python.display(),
                config.runtime_root.display()
            ))
        })?;

        // ADR-0013：spawn 后立刻纳入 Job Object，晚了抓不到孙进程。
        #[cfg(windows)]
        let containment = match crate::pty::containment::Containment::create() {
            Ok(job) => match job.assign(child.id()) {
                Ok(()) => Some(job),
                Err(error) => {
                    eprintln!("Python sidecar 纳入 Job Object 失败：{error}");
                    None
                }
            },
            Err(error) => {
                eprintln!("Python sidecar 的 Job Object 创建失败：{error}");
                None
            }
        };

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::SidecarSpawn("拿不到 sidecar stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::SidecarSpawn("拿不到 sidecar stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::SidecarSpawn("拿不到 sidecar stderr".to_string()))?;

        let (sender, lines) = mpsc::channel();
        thread::Builder::new()
            .name("sidecar-stdout".to_string())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    match rpc::decode_line(&line) {
                        Ok(Some(incoming)) => {
                            if sender.send(incoming).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {}
                        // 坏行不杀读线程：可能只是 runtime 打了一行非协议日志。
                        Err(error) => eprintln!("sidecar stdout 出现无法解析的行：{error}"),
                    }
                }
            })
            .map_err(|error| Error::SidecarSpawn(format!("stdout 读线程启动失败：{error}")))?;

        let stderr_buffer = Arc::new(Mutex::new(RingBuffer::default()));
        let stderr_writer = Arc::clone(&stderr_buffer);
        thread::Builder::new()
            .name("sidecar-stderr".to_string())
            .spawn(move || {
                let mut stderr = stderr;
                let mut chunk = [0u8; 8192];
                loop {
                    let count = match stderr.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(count) => count,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    };
                    match stderr_writer.lock() {
                        Ok(mut buffer) => buffer.push(&chunk[..count]),
                        Err(_) => break,
                    }
                }
            })
            .map_err(|error| Error::SidecarSpawn(format!("stderr 读线程启动失败：{error}")))?;

        Ok(Self {
            child,
            stdin: Some(stdin),
            lines,
            stderr: stderr_buffer,
            next_id: 1,
            broken: None,
            #[cfg(windows)]
            containment,
        })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// 进程是否还活着（供 manager 的 `status()` 做崩溃检出）。
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn broken_reason(&self) -> Option<&str> {
        self.broken.as_deref()
    }

    /// stderr 末尾若干行（诊断用）。
    pub fn stderr_tail(&self, max_lines: usize) -> String {
        match self.stderr.lock() {
            Ok(buffer) => buffer.tail(max_lines),
            Err(_) => String::new(),
        }
    }

    /// 握手：发 `initialize`，协商 protocolVersion / 必需能力 / runtime 名字。
    ///
    /// 五条校验（spec §3）：主版本一致、必需能力齐全、`runtime.name` 相符、
    /// 超时 10s、**不等 `initialized` 通知**（真实 runtime 不发）。
    pub fn handshake(&mut self, config: &SidecarConfig) -> Result<HandshakeInfo> {
        let id = self.take_id();
        let params = serde_json::json!({
            "hostVersion": config.host_version,
            "protocolVersion": PROTOCOL_VERSION,
            "requiredCapabilities": REQUIRED_CAPABILITIES,
        });

        self.write_line(&rpc::encode_request(id, "initialize", &params))?;
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        let response = self.wait_response(id, deadline)?;

        // ① 协议主版本必须一致。
        let protocol_version = response
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                Error::SidecarHandshake("initialize 应答缺少 protocolVersion".to_string())
            })?
            .to_string();
        if major(&protocol_version) != major(PROTOCOL_VERSION) {
            return Err(Error::SidecarHandshake(format!(
                "协议主版本不一致：Host {PROTOCOL_VERSION}，sidecar {protocol_version}"
            )));
        }

        // ② 必需能力必须全部出现在自报 capabilities 中。
        let capabilities: Vec<String> = response
            .get("capabilities")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let missing: Vec<&str> = REQUIRED_CAPABILITIES
            .iter()
            .copied()
            .filter(|required| !capabilities.iter().any(|item| item == required))
            .collect();
        if !missing.is_empty() {
            return Err(Error::SidecarHandshake(format!(
                "缺少必需能力：{}",
                missing.join(", ")
            )));
        }

        // ③ runtime 自报信息：名字必须吻合，防止误连到别的 Python 进程。
        let runtime = response
            .get("runtime")
            .ok_or_else(|| Error::SidecarHandshake("initialize 应答缺少 runtime".to_string()))?;
        let info = parse_runtime_info(runtime)?;
        if info.name != RUNTIME_NAME {
            return Err(Error::SidecarHandshake(format!(
                "runtime 名字不符：期望 {RUNTIME_NAME}，实际 {}",
                info.name
            )));
        }

        Ok(HandshakeInfo {
            runtime: info,
            negotiated_capabilities: capabilities,
            protocol_version,
        })
    }

    /// 调一次方法并等应答。只允许已经协商通过的方法（ping / runtime.info / shutdown）。
    pub fn call(&mut self, method: &str, params: &Value) -> Result<Value> {
        if let Some(reason) = &self.broken {
            return Err(Error::SidecarDegraded(format!("sidecar 已失效：{reason}")));
        }
        let id = self.take_id();
        self.write_line(&rpc::encode_request(id, method, params))?;
        let deadline = Instant::now() + CALL_TIMEOUT;
        self.wait_response(id, deadline)
    }

    /// 便捷方法：ping 必须回 `{"pong":true}`。
    pub fn ping(&mut self) -> Result<bool> {
        let result = self.call("ping", &serde_json::json!({}))?;
        Ok(result.get("pong").and_then(Value::as_bool) == Some(true))
    }

    /// 关 stdin → 等 `SHUTDOWN_GRACE` → 强杀进程树。幂等，可在 Drop 里再调一次。
    pub fn shutdown(&mut self) {
        // 已经退出的进程（崩溃恢复路径）无需再写 shutdown 请求，避免对着死管道写。
        let already_dead = matches!(self.child.try_wait(), Ok(Some(_)));
        // 先礼：发 shutdown 请求，行为良好的 runtime 回包后自己退出。
        if !already_dead && self.stdin.is_some() && self.broken.is_none() {
            let id = self.take_id();
            let line = rpc::encode_request(id, "shutdown", &serde_json::json!({}));
            if self.write_line(&line).is_err() {
                // 写不进去也不致命，后面还有关 stdin + 强杀兜底。
                eprintln!("sidecar shutdown 请求写入失败，转入强关流程");
            }
        }
        if let Some(stdin) = self.stdin.take() {
            drop(stdin);
        }

        if already_dead {
            return;
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(20));
                }
                _ => break,
            }
        }
        self.kill();
    }

    /// 强杀进程树。kill 之后必须 `wait()`（否则 clippy::zombie_processes）。
    fn kill(&mut self) {
        #[cfg(windows)]
        if let Some(job) = &self.containment {
            if let Err(error) = job.terminate_tree() {
                eprintln!("终止 Python sidecar 进程树失败：{error}");
            }
            let _ = self.child.wait();
            return;
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn break_stream(&mut self, reason: String) {
        if self.broken.is_none() {
            self.broken = Some(reason);
        }
        self.kill();
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn write_line(&mut self, line: &str) -> Result<()> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(Error::SidecarDegraded("sidecar stdin 已关闭".to_string()));
        };
        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|error| {
                self.broken = Some(format!("写入 sidecar stdin 失败：{error}"));
                Error::SidecarDegraded(format!(
                    "写入 sidecar stdin 失败（进程可能已退出）：{error}"
                ))
            })
    }

    /// 等 id 匹配的响应；过期 id、反向请求、通知全部丢弃（F20 无 Host API）。
    fn wait_response(&mut self, id: u64, deadline: Instant) -> Result<Value> {
        loop {
            let incoming = self.recv_until(deadline)?;
            match incoming {
                Incoming::Response {
                    id: response_id,
                    result,
                    error,
                } => {
                    if response_id.as_u64() != Some(id) {
                        eprintln!(
                            "sidecar 收到过期响应（期望 id={id}，实际 {response_id}），已丢弃"
                        );
                        continue;
                    }
                    if let Some(error) = error {
                        return Err(Error::SidecarProtocol(format!(
                            "sidecar 返回错误 {}：{}",
                            error.code, error.message
                        )));
                    }
                    return Ok(result);
                }
                Incoming::Request { method, .. } => {
                    // F20 不提供 Host API：sidecar 不该反向调用宿主，丢弃并留诊断。
                    eprintln!("sidecar 发来未支持的反向请求 {method}，已丢弃");
                }
                Incoming::Notification { method, .. } => {
                    eprintln!("sidecar 发来未消费的通知 {method}，已丢弃");
                }
            }
        }
    }

    fn recv_until(&mut self, deadline: Instant) -> Result<Incoming> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match self.lines.recv_timeout(remaining) {
            Ok(incoming) => Ok(incoming),
            Err(RecvTimeoutError::Timeout) => {
                let tail = self.stderr_tail(20);
                self.break_stream("超时".to_string());
                Err(Error::SidecarTimeout(format!(
                    "sidecar 在 {}ms 内未应答。stderr 末尾：\n{tail}",
                    CALL_TIMEOUT.as_millis()
                )))
            }
            Err(RecvTimeoutError::Disconnected) => {
                let tail = self.stderr_tail(20);
                self.break_stream("stdout 已关闭".to_string());
                let code = self.child.try_wait().ok().flatten();
                Err(Error::SidecarDegraded(format!(
                    "sidecar stdout 已关闭（退出状态 {code:?}）。stderr 末尾：\n{tail}"
                )))
            }
        }
    }
}

impl Drop for SidecarProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 把 `runtime` JSON 解析成强类型 DTO（键名由 dto.rs 的 contract test 锁定）。
fn parse_runtime_info(value: &Value) -> Result<RuntimeInfo> {
    fn required_str(value: &Value, key: &str) -> Result<String> {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| Error::SidecarHandshake(format!("runtime 缺少 {key}")))
    }
    fn string_array(value: &Value, key: &str) -> Vec<String> {
        value
            .get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }

    Ok(RuntimeInfo {
        name: required_str(value, "name")?,
        version: required_str(value, "version")?,
        transport: required_str(value, "transport")?,
        python: required_str(value, "python")?,
        protocol_version: required_str(value, "protocolVersion")?,
        providers: string_array(value, "providers"),
        capabilities: string_array(value, "capabilities"),
        methods: string_array(value, "methods"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn major_only_compares_the_first_segment() {
        assert_eq!(major("1.0"), "1");
        assert_eq!(major("1.9"), "1");
        assert_eq!(major("2.0"), "2");
        assert_eq!(major("2"), "2");
    }

    #[test]
    fn ring_buffer_keeps_only_the_tail() {
        let mut buffer = RingBuffer::default();
        buffer.push(b"first\n");
        buffer.push(b"second\n");
        buffer.push(b"third\n");
        assert_eq!(buffer.tail(2), "second\nthird");
    }

    #[test]
    fn ring_buffer_caps_a_single_oversized_chunk() {
        let mut buffer = RingBuffer::default();
        let huge = vec![b'x'; STDERR_LIMIT * 2];
        buffer.push(&huge);
        assert_eq!(buffer.bytes.len(), STDERR_LIMIT);
    }

    #[test]
    fn parse_runtime_info_requires_the_locked_keys() {
        let value = serde_json::json!({
            "name": "harness-hub-runtime",
            "version": "0.5.0",
            "transport": "stdio-jsonrpc",
            "python": "3.14.3",
            "protocolVersion": "1.0",
            "providers": [],
            "capabilities": ["ping"],
            "methods": ["ping"]
        });
        let info = parse_runtime_info(&value).expect("解析");
        assert_eq!(info.name, "harness-hub-runtime");
        assert_eq!(info.version, "0.5.0");
        assert_eq!(info.protocol_version, "1.0");
        assert_eq!(info.methods, vec!["ping"]);
    }

    #[test]
    fn parse_runtime_info_rejects_missing_name() {
        let value = serde_json::json!({"version": "0.5.0"});
        let error = parse_runtime_info(&value).expect_err("缺 name 必须失败");
        assert!(
            matches!(error, Error::SidecarHandshake(_)),
            "实际错误：{error:?}"
        );
    }
}
