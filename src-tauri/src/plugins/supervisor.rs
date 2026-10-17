//! 插件子进程的监督者：spawn、握手、方法调用、崩溃隔离。
//!
//! 进程模型（ADR-0018 决策 1）：第三方插件一律 **out-of-process**，
//! 用行分隔 JSON-RPC 2.0 over stdio 通信 —— 与 MCP 同形态。选择它的理由不是
//! 「安全沙箱」（v0.1 根本没有沙箱），而是三条工程事实：
//!
//! ```text
//! 1. 进程边界让「插件写崩了」只影响它自己（R14：插件故障不得拖垮主程序）
//! 2. 语言无关：插件可以是 Python / Node / 任何能读 stdin 的程序
//! 3. 协议可被第三方独立实现（SDK 只需描述 stdio 上的 JSON）
//! ```
//!
//! 线程模型：spawn 之后立刻起两个守护线程 ——
//!
//! ```text
//! stdout 读线程 → 逐行 decode_line → mpsc channel → 调用方用 recv_timeout 取
//! stderr 读线程 → 写入 64 KiB 环形缓冲（崩溃时把末尾几行附进错误信息）
//! ```
//!
//! 为什么读 stdout 必须用独立线程：`std::process::ChildStdout` 的 `read` 是阻塞的，
//! 在调用线程里直接读会让「方法超时」无法实现（超时判断自己都需要另一个线程来计时）。
//!
//! 为什么超时后**杀掉进程**而不是继续等：超时意味着协议流已经无法对齐
//! （我们不知道下一行是超时方法的迟到响应还是别的请求），继续复用这条流只会
//! 让错误错位到后续调用。杀进程 + 下次 lazy respawn 是唯一可靠的做法。

use std::collections::{BTreeSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::error::{Error, Result};
use crate::plugins::manifest::{Capabilities, Manifest, API_VERSION};
use crate::plugins::rpc::{self, Incoming, RpcError};

/// 握手超时。10s 足够 Python/Node 冷启动，又不至于让 UI 长时间无反馈。
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// 单次方法调用超时。
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// stderr 环形缓冲上限。
///
/// 插件可能疯狂写 stderr（无限循环里打日志）。不设上限等于给宿主一个内存炸弹；
/// 64 KiB 大约能留下崩溃前几百行，够诊断。
pub const STDERR_LIMIT: usize = 64 * 1024;

/// 关闭时先礼后兵的时间窗：关掉 stdin 后等它自己退出这么久，再强杀。
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// 握手协商出来的允许方法集。
///
/// 它是 `supported_methods ∩ manifest [capabilities]` 的交集：
/// 插件说自己会什么不算数，manifest 声明它被允许什么才算数（ADR-0018 决策 2）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AllowedMethods {
    methods: BTreeSet<String>,
}

impl AllowedMethods {
    pub fn from_intersection(supported: &[String], capabilities: &Capabilities) -> Self {
        let methods = supported
            .iter()
            .filter(|method| capabilities.allows(method))
            .cloned()
            .collect();
        Self { methods }
    }

    pub fn allows(&self, method: &str) -> bool {
        self.methods.contains(method)
    }

    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.methods.iter()
    }

    pub fn len(&self) -> usize {
        self.methods.len()
    }

    pub fn is_empty(&self) -> bool {
        self.methods.is_empty()
    }
}

/// 插件的握手应答。
#[derive(Debug, Clone, PartialEq)]
pub struct HandshakeInfo {
    pub plugin_id: String,
    pub plugin_version: String,
    pub allowed: AllowedMethods,
}

/// stderr 环形缓冲：保留最后 64 KiB。
#[derive(Debug, Default)]
struct RingBuffer {
    lines: VecDeque<String>,
    bytes: usize,
}

impl RingBuffer {
    fn push(&mut self, line: String) {
        self.bytes += line.len() + 1;
        self.lines.push_back(line);

        while self.bytes > STDERR_LIMIT && self.lines.len() > 1 {
            if let Some(dropped) = self.lines.pop_front() {
                self.bytes -= dropped.len() + 1;
            }
        }
    }

    /// 末尾若干行，用 `\n` 连接。错误信息用这个，避免把 64 KiB 全塞进 UI。
    fn tail(&self, max_lines: usize) -> String {
        let start = self.lines.len().saturating_sub(max_lines);
        self.lines
            .iter()
            .skip(start)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn byte_len(&self) -> usize {
        self.bytes
    }
}

/// 一次 Host API 调用（插件 → 宿主）的处理回调。
///
/// 用 trait 对象而不是直接传 `&Connection`，是为了让 supervisor 不依赖 rusqlite：
/// 进程监督与权限记录是两件事，混在一起会让 supervisor 的单测必须建库。
///
/// 刻意**不加** `Send` 约束：调用方是同一个线程（refresh 或 IPC 命令），
/// 加 `Send` 只会逼实现方把 `&Connection` 包进 `Mutex`，凭空引入一次锁。
pub trait HostApi {
    /// 注意：返回类型必须写全 `std::result::Result`。
    /// `crate::error::Result<T>` 是单参数别名，写成 `Result<Value, RpcError>` 会被
    /// 解析成「给一个参数别名传两个参数」，报 E0107 而不是类型不匹配。
    fn dispatch(&mut self, method: &str, params: &Value) -> std::result::Result<Value, RpcError>;
}

/// 拒绝一切 Host API 的空实现（单测/不需要审计的场景用）。
pub struct NoHostApi;

impl HostApi for NoHostApi {
    fn dispatch(&mut self, method: &str, _params: &Value) -> std::result::Result<Value, RpcError> {
        Err(RpcError::new(
            rpc::NOT_IMPLEMENTED,
            format!("host api unavailable: {method}"),
        ))
    }
}

/// 一个已 spawn 的插件进程。
pub struct PluginProcess {
    plugin_id: String,
    child: Child,
    /// `None` = stdin 已关闭（shutdown 之后）。
    stdin: Option<ChildStdin>,
    lines: Receiver<Incoming>,
    stderr: Arc<Mutex<RingBuffer>>,
    /// manifest 声明的能力，握手时与插件自报的 supported_methods 求交集。
    capabilities: Capabilities,
    allowed: AllowedMethods,
    next_id: u64,
    /// 超时/崩溃后置位：后续调用直接报错的依据，避免对着死流继续写。
    broken: Option<String>,
    #[cfg(windows)]
    containment: Option<crate::pty::containment::Containment>,
}

impl PluginProcess {
    /// spawn 插件进程（不做握手）。握手见 [`PluginProcess::handshake`]。
    ///
    /// `command` 来自 manifest 的 `[adapter].command`，`command[0]` 是程序、其余是参数。
    /// **不经过 shell**：用户可控字符串进 shell 等于把注入面交给插件作者。
    pub fn spawn(manifest: &Manifest, plugin_root: &std::path::Path) -> Result<Self> {
        let program = manifest
            .command
            .first()
            .ok_or_else(|| Error::PluginSpawn("manifest [adapter].command 为空".to_string()))?;

        let mut command = Command::new(program);
        command
            .args(&manifest.command[1..])
            .current_dir(plugin_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // 插件是后台进程，不该弹黑框（用户体验与「不抢焦点」）。
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = command.spawn().map_err(|error| {
            Error::PluginSpawn(format!(
                "{program} 启动失败（cwd={}）：{error}",
                plugin_root.display()
            ))
        })?;

        // ADR-0013：必须在子进程刚起来时立刻纳入 Job Object，
        // 晚了它可能已经派生出孙进程，那些孙进程不会被 Job 抓到。
        #[cfg(windows)]
        let containment = match crate::pty::containment::Containment::create() {
            Ok(job) => {
                if let Err(error) = job.assign(child.id()) {
                    eprintln!("插件 {} 纳入 Job Object 失败：{error}", manifest.id);
                    None
                } else {
                    Some(job)
                }
            }
            Err(error) => {
                eprintln!("插件 {} 的 Job Object 创建失败：{error}", manifest.id);
                None
            }
        };

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::PluginSpawn("拿不到插件 stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::PluginSpawn("拿不到插件 stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Error::PluginSpawn("拿不到插件 stderr".to_string()))?;

        let (sender, lines) = mpsc::channel();
        thread::Builder::new()
            .name(format!("plugin-{}-stdout", manifest.id))
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
                        Err(error) => {
                            // 坏行不该杀死读线程：插件可能只是打了一行非协议日志到 stdout。
                            // 记下来，继续读下一行。
                            eprintln!("插件 stdout 出现无法解析的行：{error}");
                        }
                    }
                }
            })
            .map_err(|error| Error::PluginSpawn(format!("stdout 读线程启动失败：{error}")))?;

        let stderr_buffer = Arc::new(Mutex::new(RingBuffer::default()));
        let stderr_writer = Arc::clone(&stderr_buffer);
        thread::Builder::new()
            .name(format!("plugin-{}-stderr", manifest.id))
            .spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    match stderr_writer.lock() {
                        Ok(mut buffer) => buffer.push(line),
                        // 锁中毒时静默丢弃：stderr 是诊断数据，不值得为它 panic 宿主。
                        Err(_) => break,
                    }
                }
            })
            .map_err(|error| Error::PluginSpawn(format!("stderr 读线程启动失败：{error}")))?;

        Ok(Self {
            plugin_id: manifest.id.clone(),
            child,
            stdin: Some(stdin),
            lines,
            stderr: stderr_buffer,
            capabilities: manifest.capabilities.clone(),
            allowed: AllowedMethods::default(),
            next_id: 1,
            broken: None,
            #[cfg(windows)]
            containment,
        })
    }

    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn allowed(&self) -> &AllowedMethods {
        &self.allowed
    }

    /// 最后一次失败原因（`None` 表示流仍然健康）。
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

    pub fn stderr_byte_len(&self) -> usize {
        match self.stderr.lock() {
            Ok(buffer) => buffer.byte_len(),
            Err(_) => 0,
        }
    }

    /// 把 manifest 声明的能力摊平成 JSON，随 `initialize` 一起下发给插件。
    ///
    /// 键名与 manifest 的 `[capabilities]` 完全一致：插件读到的就是作者写下的那几个名字，
    /// 不必记第二套命名。
    fn capability_map(&self) -> Value {
        serde_json::json!({
            "detect": self.capabilities.detect,
            "launch": self.capabilities.launch,
            "resume": self.capabilities.resume,
            "usage": self.capabilities.usage,
            "trace": self.capabilities.trace,
            "profiles": self.capabilities.profiles,
            "mcp": self.capabilities.mcp,
            "skills": self.capabilities.skills,
        })
    }

    /// 握手：发 `initialize`，等应答 + `initialized` 通知，算出允许方法集。
    ///
    /// 校验顺序刻意是「先对上身份、再谈能力」：一个 id 不匹配的回包说明我们
    /// 连到了错误的进程，此时任何后续结论都不可信。
    pub fn handshake(&mut self, host_version: &str) -> Result<HandshakeInfo> {
        let id = self.take_id();
        let params = serde_json::json!({
            "host_version": host_version,
            "api_version": API_VERSION,
            "manifest_capabilities": self.capability_map(),
        });

        self.write_line(&rpc::encode_request(id, "initialize", &params))?;

        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        let response = self.wait_response(id, deadline, &mut NoHostApi)?;

        let plugin_id = response
            .get("plugin_id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::PluginHandshake("initialize 应答缺少 plugin_id".to_string()))?
            .to_string();

        if plugin_id != self.plugin_id {
            return Err(Error::PluginHandshake(format!(
                "插件自报 id 与 manifest 不一致：回包 {plugin_id}，manifest {}",
                self.plugin_id
            )));
        }

        let plugin_version = response
            .get("plugin_version")
            .and_then(Value::as_str)
            .unwrap_or("0.0.0")
            .to_string();

        let supported: Vec<String> = response
            .get("supported_methods")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        // `initialized` 通知：收不到说明插件没走完握手流程，但它已经答了 initialize，
        // 只要 stdout 上出现过就说明进程活着。这里不强制要求，避免对最简实现过于苛刻。
        let _ = self.wait_notification("initialized", deadline);

        // capability negotiation：交集决定宿主还能调什么。
        let allowed = AllowedMethods::from_intersection(&supported, &self.capabilities);
        self.allowed = allowed.clone();

        Ok(HandshakeInfo {
            plugin_id,
            plugin_version,
            allowed,
        })
    }

    /// 发一次方法调用并等它的响应；期间插件发来的 Host API 请求交给 `host` 处理。
    pub fn call(&mut self, method: &str, params: &Value, host: &mut dyn HostApi) -> Result<Value> {
        if let Some(reason) = &self.broken {
            return Err(Error::PluginDegraded(format!(
                "插件 {} 已失效：{reason}",
                self.plugin_id
            )));
        }

        if !self.allowed.allows(method) {
            // 未协商通过的方法一律拒绝：capability negotiation 不生效就等于没有协商。
            return Err(Error::PluginProtocol(format!(
                "插件 {} 未被允许调用 {method}（manifest 未声明或插件不支持）",
                self.plugin_id
            )));
        }

        let id = self.take_id();
        self.write_line(&rpc::encode_request(id, method, params))?;

        let deadline = Instant::now() + CALL_TIMEOUT;
        self.wait_response(id, deadline, host)
    }

    /// 关 stdin → 等 `SHUTDOWN_GRACE` → 强杀进程树。
    pub fn shutdown(&mut self) {
        // 先关 stdin：行为良好的插件读到 EOF 会自己退出。
        if let Some(stdin) = self.stdin.take() {
            drop(stdin);
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

    /// 强杀进程树（Job Object 有效时连孙进程一起）。
    fn kill(&mut self) {
        #[cfg(windows)]
        if let Some(job) = &self.containment {
            if let Err(error) = job.terminate_tree() {
                eprintln!("终止插件 {} 的进程树失败：{error}", self.plugin_id);
            }
            let _ = self.child.wait();
            return;
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// 标记为坏流并杀掉进程：超时、写失败、EOF 都走这里。
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
            return Err(Error::PluginDegraded(format!(
                "插件 {} 的 stdin 已关闭",
                self.plugin_id
            )));
        };

        stdin
            .write_all(line.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|error| {
                // 写不进去基本等于对端已死。语义上这是「插件失效」而不是「启动失败」，
                // 所以这里用 PluginDegraded —— 上层据此标记 degraded 并发起 respawn。
                self.broken = Some(format!("写入插件 stdin 失败：{error}"));
                Error::PluginDegraded(format!(
                    "写入插件 {} stdin 失败（进程可能已退出）：{error}",
                    self.plugin_id
                ))
            })
    }

    /// 等 id 匹配的响应；途中把插件的反向请求交给 `host`。
    fn wait_response(
        &mut self,
        id: u64,
        deadline: Instant,
        host: &mut dyn HostApi,
    ) -> Result<Value> {
        loop {
            let incoming = self.recv_until(deadline)?;
            match incoming {
                Incoming::Response {
                    id: response_id,
                    result,
                    error,
                } => {
                    if response_id.as_u64() != Some(id) {
                        // 迟到的响应（上一个超时方法的）直接丢弃：它对应的调用早已失败。
                        eprintln!(
                            "插件 {} 收到过期响应（期望 id={id}，实际 {response_id}），已丢弃",
                            self.plugin_id
                        );
                        continue;
                    }
                    if let Some(error) = error {
                        return Err(Error::PluginProtocol(format!(
                            "插件返回错误 {}：{}",
                            error.code, error.message
                        )));
                    }
                    return Ok(result);
                }
                Incoming::Request {
                    id: request_id,
                    method,
                    params,
                } => {
                    // 插件 → 宿主：Host API 调用。应答后继续等自己那条响应。
                    let line = match host.dispatch(&method, &params) {
                        Ok(value) => rpc::encode_response_ok(&request_id, &value),
                        Err(error) => rpc::encode_response_err(&request_id, &error),
                    };
                    self.write_line(&line)?;
                }
                Incoming::Notification { method, .. } => {
                    // v0.1 没有订阅任何通知：收到就丢，只留一行诊断。
                    eprintln!("插件 {} 发来未消费的通知 {method}", self.plugin_id);
                }
            }
        }
    }

    fn wait_notification(&mut self, expected: &str, deadline: Instant) -> Result<()> {
        loop {
            let incoming = self.recv_until(deadline)?;
            match incoming {
                Incoming::Notification { method, .. } if method == expected => return Ok(()),
                Incoming::Notification { .. } => continue,
                other => {
                    eprintln!(
                        "插件 {} 在握手期间收到非通知消息：{other:?}",
                        self.plugin_id
                    );
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
                Err(Error::PluginTimeout(format!(
                    "插件 {} 在 {}ms 内未应答。stderr 末尾：\n{tail}",
                    self.plugin_id,
                    CALL_TIMEOUT.as_millis()
                )))
            }
            Err(RecvTimeoutError::Disconnected) => {
                let tail = self.stderr_tail(20);
                self.break_stream("stdout 已关闭".to_string());
                let code = self.child.try_wait().ok().flatten();
                Err(Error::PluginDegraded(format!(
                    "插件 {} 的 stdout 已关闭（退出状态 {code:?}）。stderr 末尾：\n{tail}",
                    self.plugin_id
                )))
            }
        }
    }
}

impl Drop for PluginProcess {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn python_available() -> bool {
        Command::new("python")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/plugins")
            .join(name)
    }

    fn manifest_for(id: &str, script: &str) -> Manifest {
        Manifest {
            id: id.to_string(),
            name: id.to_string(),
            version: "0.1.0".to_string(),
            api_version: API_VERSION.to_string(),
            command: vec!["python".to_string(), script.to_string()],
            capabilities: Capabilities {
                usage: true,
                ..Capabilities::default()
            },
        }
    }

    #[test]
    fn allowed_methods_is_the_intersection() {
        let capabilities = Capabilities {
            usage: true,
            skills: true,
            ..Capabilities::default()
        };

        let allowed = AllowedMethods::from_intersection(
            &[
                "usage/import".to_string(),
                "skill/list".to_string(),
                // 插件自称支持，但 manifest 没允许 trace → 必须被剔除。
                "trace/subscribe".to_string(),
            ],
            &capabilities,
        );

        assert!(allowed.allows("usage/import"));
        assert!(allowed.allows("skill/list"));
        assert!(!allowed.allows("trace/subscribe"));
        assert_eq!(allowed.len(), 2);
    }

    #[test]
    fn ring_buffer_keeps_only_the_tail() {
        let mut buffer = RingBuffer::default();
        for index in 0..10_000 {
            buffer.push(format!("line {index} {}", "x".repeat(50)));
        }

        assert!(
            buffer.byte_len() <= STDERR_LIMIT + 200,
            "{}",
            buffer.byte_len()
        );
        let tail = buffer.tail(3);
        assert!(tail.contains("line 9999"), "{tail}");
        assert!(!tail.contains("line 0 "), "{tail}");
    }

    #[test]
    fn handshake_succeeds_against_echo_fixture() {
        if !python_available() {
            eprintln!("跳过：PATH 里没有 python");
            return;
        }

        let manifest = manifest_for("echo-plugin", "echo_plugin.py");
        let mut process = PluginProcess::spawn(&manifest, &fixture("")).expect("spawn");
        let info = process.handshake("0.1.0").expect("握手");

        assert_eq!(info.plugin_id, "echo-plugin");
        assert!(info.allowed.allows("usage/import"));

        let result = process
            .call(
                "usage/import",
                &serde_json::json!({"source": "echo"}),
                &mut NoHostApi,
            )
            .expect("调用");
        assert_eq!(result["ok"], true);
    }

    #[test]
    fn handshake_times_out_when_plugin_is_silent_and_kills_it() {
        if !python_available() {
            eprintln!("跳过：PATH 里没有 python");
            return;
        }

        let manifest = manifest_for("silent-plugin", "silent_plugin.py");
        let mut process = PluginProcess::spawn(&manifest, &fixture("")).expect("spawn");
        let pid = process.pid();

        let started = Instant::now();
        let error = process.handshake("0.1.0").expect_err("静默插件必须超时");
        assert!(matches!(error, Error::PluginTimeout(_)), "{error}");
        assert!(
            started.elapsed() >= Duration::from_secs(9),
            "过早失败：{:?}",
            started.elapsed()
        );
        assert!(process.broken_reason().is_some());

        // 超时必须真的把进程杀掉，否则静默插件会一直挂在后台。
        assert!(!process_alive(pid), "超时后插件进程仍然存活：{pid}");
    }

    #[test]
    fn crashing_plugin_surfaces_degraded_without_panicking_host() {
        if !python_available() {
            eprintln!("跳过：PATH 里没有 python");
            return;
        }

        let manifest = manifest_for("crashing-plugin", "crashing_plugin.py");
        let mut process = PluginProcess::spawn(&manifest, &fixture("")).expect("spawn");
        process.handshake("0.1.0").expect("握手");

        // 插件在握手后立刻 os._exit(1)：下一次调用应当拿到结构化错误而不是 panic。
        thread::sleep(Duration::from_millis(300));
        let error = process
            .call("usage/import", &serde_json::json!({}), &mut NoHostApi)
            .expect_err("崩溃插件必须报错");

        let text = error.to_string();
        assert!(
            text.contains("stdout 已关闭") || text.contains("已失效"),
            "{text}"
        );
        assert!(process.broken_reason().is_some());
    }

    #[test]
    fn unbounded_stderr_does_not_grow_the_buffer() {
        if !python_available() {
            eprintln!("跳过：PATH 里没有 python");
            return;
        }

        let manifest = manifest_for("chatty-plugin", "chatty_stderr.py");
        let mut process = PluginProcess::spawn(&manifest, &fixture("")).expect("spawn");
        process.handshake("0.1.0").expect("握手");

        // 让插件往 stderr 猛写 2 秒。
        let _ = process.call("flood/stderr", &serde_json::json!({}), &mut NoHostApi);
        thread::sleep(Duration::from_millis(500));

        let bytes = process.stderr_byte_len();
        assert!(bytes <= STDERR_LIMIT + 4096, "stderr 缓冲无界增长：{bytes}");
    }

    #[cfg(windows)]
    fn process_alive(pid: u32) -> bool {
        let output = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .expect("tasklist");
        String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
    }

    #[cfg(not(windows))]
    fn process_alive(_pid: u32) -> bool {
        true
    }
}
