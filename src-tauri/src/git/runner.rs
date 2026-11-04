use super::GitCaptureError;
use std::{
    ffi::OsString,
    io::{self, Read},
    path::Path,
    process::{Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub timeout: Duration,
    pub stdout: usize,
    pub stderr: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(3),
            stdout: 1_048_576,
            stderr: 1_048_576,
        }
    }
}
#[derive(Debug)]
pub(super) struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: ExitStatus,
}
pub(super) fn run(
    program: &OsString,
    args: &[OsString],
    cwd: &Path,
    limits: Limits,
) -> Result<Output, GitCaptureError> {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(cwd);
    run_command(cmd, limits)
}
fn run_command(mut cmd: Command, limits: Limits) -> Result<Output, GitCaptureError> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env("LANG", "C");
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
        "GIT_PREFIX",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ] {
        cmd.env_remove(key);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(windows)]
    let mut containment = Some(
        crate::pty::containment::Containment::create()
            .map_err(|_| GitCaptureError::LaunchFailed)?,
    );
    #[cfg(not(windows))]
    let containment: Option<()> = None;
    let mut child = cmd.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            GitCaptureError::CommandNotFound
        } else {
            GitCaptureError::LaunchFailed
        }
    })?;
    #[cfg(windows)]
    if containment.as_ref().unwrap().assign(child.id()).is_err() {
        terminate_child_tree(&mut child, None);
        return Err(GitCaptureError::LaunchFailed);
    }
    #[cfg(windows)]
    reconcile_job_descendants(containment.as_ref().unwrap(), child.id());
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let tx1 = tx.clone();
    let out_cap = limits.stdout;
    let err_cap = limits.stderr;
    let out_thread = thread::spawn(move || {
        let _ = tx1.send((true, drain(stdout, out_cap)));
    });
    let err_thread = thread::spawn(move || {
        let _ = tx.send((false, drain(stderr, err_cap)));
    });
    let start = Instant::now();
    let mut out = None;
    let mut err = None;
    loop {
        while let Ok((is_out, value)) = rx.try_recv() {
            if is_out {
                out = Some(value)
            } else {
                err = Some(value)
            }
        }
        if out
            .as_ref()
            .is_some_and(|v: &Result<Vec<u8>, DrainError>| v.is_err())
            || err
                .as_ref()
                .is_some_and(|v: &Result<Vec<u8>, DrainError>| v.is_err())
        {
            let terminated = terminate_child_tree(&mut child, containment.as_ref());
            #[cfg(windows)]
            if !terminated {
                drop(containment.take());
            }
            #[cfg(not(windows))]
            let _ = terminated;
            let _ = out_thread.join();
            let _ = err_thread.join();
            return Err(
                if out
                    .as_ref()
                    .is_some_and(|r| matches!(r, Err(DrainError::Limit)))
                    || err
                        .as_ref()
                        .is_some_and(|r| matches!(r, Err(DrainError::Limit)))
                {
                    GitCaptureError::OutputLimit
                } else {
                    GitCaptureError::ReadFailed
                },
            );
        }
        if start.elapsed() >= limits.timeout {
            let terminated = terminate_child_tree(&mut child, containment.as_ref());
            #[cfg(windows)]
            if !terminated {
                drop(containment.take());
            }
            #[cfg(not(windows))]
            let _ = terminated;
            let _ = out_thread.join();
            let _ = err_thread.join();
            return Err(GitCaptureError::Timeout);
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(_) => {
                let terminated = terminate_child_tree(&mut child, containment.as_ref());
                #[cfg(windows)]
                if !terminated {
                    drop(containment.take());
                }
                #[cfg(not(windows))]
                let _ = terminated;
                let _ = out_thread.join();
                let _ = err_thread.join();
                return Err(GitCaptureError::ReadFailed);
            }
        };
        if let Some(status) = status {
            if out.is_none() || err.is_none() {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
            let _ = out_thread.join();
            let _ = err_thread.join();
            return Ok(Output {
                stdout: out.unwrap().map_err(|e| {
                    if matches!(e, DrainError::Limit) {
                        GitCaptureError::OutputLimit
                    } else {
                        GitCaptureError::ReadFailed
                    }
                })?,
                stderr: err.unwrap().map_err(|e| {
                    if matches!(e, DrainError::Limit) {
                        GitCaptureError::OutputLimit
                    } else {
                        GitCaptureError::ReadFailed
                    }
                })?,
                status,
            });
        }
        thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(windows)]
fn terminate_child_tree(
    child: &mut std::process::Child,
    containment: Option<&crate::pty::containment::Containment>,
) -> bool {
    use crate::pty::containment::{descendants_of, terminate_pid};
    let terminated = if let Some(job) = containment {
        reconcile_job_descendants(job, child.id());
        job.terminate_tree().is_ok()
    } else {
        let mut ok = true;
        for pid in descendants_of(child.id()) {
            ok &= terminate_pid(pid).is_ok();
        }
        ok
    };
    let _ = child.kill();
    let _ = child.wait();
    terminated
}

#[cfg(windows)]
fn reconcile_job_descendants(job: &crate::pty::containment::Containment, root: u32) {
    use crate::pty::containment::{descendants_of, SWEEP_ROUNDS_LIMIT};
    // This narrows the post-spawn assignment window; it does not claim race-free
    // creation-time containment (see ADR-0013).
    for _ in 0..SWEEP_ROUNDS_LIMIT {
        let mut found_unassigned = false;
        for pid in descendants_of(root) {
            if !job.is_member(pid).unwrap_or(false) {
                found_unassigned = true;
                let _ = job.assign(pid);
            }
        }
        if !found_unassigned {
            break;
        }
    }
}

#[cfg(unix)]
fn terminate_child_tree(child: &mut std::process::Child, _containment: Option<&()>) -> bool {
    // process_group(0) is set atomically by the OS at spawn, so descendants inherit
    // the group before the runner begins monitoring output.
    let ok = unsafe { kill(-(child.id() as i32), 9) == 0 };
    let _ = child.kill();
    let _ = child.wait();
    ok
}

#[cfg(all(not(unix), not(windows)))]
fn terminate_child_tree(child: &mut std::process::Child, _containment: Option<&()>) -> bool {
    let _ = child.kill();
    let _ = child.wait();
    true
}

#[cfg(unix)]
extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}
#[derive(Debug)]
enum DrainError {
    Limit,
    Read,
}
fn drain<R: Read>(mut reader: R, cap: usize) -> Result<Vec<u8>, DrainError> {
    let mut data = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if data.len().saturating_add(n) > cap {
                    let keep = cap.saturating_sub(data.len());
                    data.extend_from_slice(&buf[..keep]);
                    return Err(DrainError::Limit);
                } else {
                    data.extend_from_slice(&buf[..n]);
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(DrainError::Read),
        }
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore]
    fn child_fixture() {
        let Ok(mode) = std::env::var("HH_RUNNER_CHILD_MODE") else {
            return;
        };
        match mode.as_str() {
            "env" => println!(
                "ENV:{}|{}|{}|{}",
                std::env::var("GIT_OPTIONAL_LOCKS").unwrap_or_default(),
                std::env::var("GIT_TERMINAL_PROMPT").unwrap_or_default(),
                std::env::var("LC_ALL").unwrap_or_default(),
                [
                    "GIT_DIR",
                    "GIT_WORK_TREE",
                    "GIT_INDEX_FILE",
                    "GIT_COMMON_DIR",
                    "GIT_PREFIX",
                    "GIT_OBJECT_DIRECTORY",
                    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                ]
                .iter()
                .map(|k| std::env::var(k).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("|")
            ),
            "fail" => {
                eprintln!("SECRET_SENTINEL");
                std::process::exit(17);
            }
            "sleep" => {
                std::thread::sleep(Duration::from_secs(30));
            }
            "inherited-pipe" | "inherited-pipe-output" | "grandchild" => {
                if mode == "grandchild" {
                    write_pid_env("HH_RUNNER_GRANDCHILD_PID_FILE");
                    std::thread::sleep(Duration::from_secs(8));
                    return;
                }
                write_pid_env("HH_RUNNER_PARENT_PID_FILE");
                while !std::path::Path::new(&std::env::var("HH_RUNNER_RELEASE_FILE").unwrap())
                    .exists()
                {
                    std::thread::yield_now();
                }
                let mut child = Command::new(std::env::current_exe().unwrap());
                child
                    .args([
                        "--exact",
                        "git::runner::tests::child_fixture",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("HH_RUNNER_CHILD_MODE", "grandchild");
                let mut grandchild = child.spawn().unwrap();
                let grandchild_waiter = std::thread::spawn(move || grandchild.wait());
                while !std::path::Path::new(
                    &std::env::var("HH_RUNNER_GRANDCHILD_PID_FILE").unwrap(),
                )
                .exists()
                {
                    std::thread::yield_now();
                }
                if mode == "inherited-pipe-output" {
                    for _ in 0..100_000 {
                        if std::io::Write::write_all(&mut std::io::stdout(), b"0123456789abcdef")
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(30));
                let _ = grandchild_waiter.join();
            }
            "stdout" => loop {
                print!("0123456789abcdef");
                let _ = std::io::Write::flush(&mut std::io::stdout());
            },
            "stderr" => loop {
                eprint!("0123456789abcdef");
                let _ = std::io::Write::flush(&mut std::io::stderr());
            },
            "both" => {
                let out = std::thread::spawn(|| {
                    let mut bytes = b"OUT_BEGIN".to_vec();
                    bytes.extend(b"MARKER0123456789".repeat(50_000));
                    bytes.extend(b"OUT_END");
                    let _ = std::io::Write::write_all(&mut std::io::stdout(), &bytes);
                });
                let err = std::thread::spawn(|| {
                    let bytes = vec![b'E'; 100_000];
                    let _ = std::io::Write::write_all(&mut std::io::stderr(), &bytes);
                });
                let _ = out.join();
                let _ = err.join();
            }
            _ => {}
        }
    }

    fn write_pid_env(key: &str) {
        if let Ok(path) = std::env::var(key) {
            std::fs::write(path, std::process::id().to_string()).unwrap();
        }
    }

    fn command(mode: &str) -> Command {
        let mut c = Command::new(std::env::current_exe().unwrap());
        c.args([
            "--exact",
            "git::runner::tests::child_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("HH_RUNNER_CHILD_MODE", mode)
        .env("GIT_DIR", "inherited-secret")
        .env("GIT_WORK_TREE", "inherited-secret")
        .env("GIT_INDEX_FILE", "inherited-secret")
        .env("GIT_COMMON_DIR", "inherited-secret")
        .env("GIT_PREFIX", "inherited-secret")
        .env("GIT_OBJECT_DIRECTORY", "inherited-secret")
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", "inherited-secret")
        .env("GIT_OPTIONAL_LOCKS", "1")
        .env("GIT_TERMINAL_PROMPT", "1")
        .env("LC_ALL", "polluted");
        c
    }
    fn limits(timeout: Duration, cap: usize) -> Limits {
        Limits {
            timeout,
            stdout: cap,
            stderr: cap,
        }
    }

    #[test]
    fn stdout_limit_terminates_before_timeout() {
        let start = Instant::now();
        let result = run_command(command("stdout"), limits(Duration::from_secs(5), 128));
        assert_eq!(result.unwrap_err(), GitCaptureError::OutputLimit);
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn stderr_limit_terminates_before_timeout() {
        let start = Instant::now();
        let result = run_command(command("stderr"), limits(Duration::from_secs(5), 128));
        assert_eq!(result.unwrap_err(), GitCaptureError::OutputLimit);
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn drains_both_large_streams_and_keeps_stdout() {
        let output =
            run_command(command("both"), limits(Duration::from_secs(5), 2_000_000)).unwrap();
        let begin = output
            .stdout
            .windows(9)
            .position(|w| w == b"OUT_BEGIN")
            .unwrap()
            + 9;
        let end = output
            .stdout
            .windows(7)
            .position(|w| w == b"OUT_END")
            .unwrap();
        let payload = &output.stdout[begin..end];
        assert_eq!(payload, b"MARKER0123456789".repeat(50_000));
        assert_eq!(output.stderr.len(), 100_000);
    }
    #[test]
    fn timeout_kills_and_reaps_child() {
        let start = Instant::now();
        let result = run_command(command("sleep"), limits(Duration::from_millis(100), 4096));
        assert_eq!(result.unwrap_err(), GitCaptureError::Timeout);
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn timeout_and_output_limit_kill_descendants_holding_inherited_pipes() {
        for (mode, timeout, cap) in [
            ("inherited-pipe", Duration::from_millis(500), 4096),
            ("inherited-pipe-output", Duration::from_secs(5), 128),
        ] {
            let temp = std::env::temp_dir().join(format!(
                "hh-runner-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            let _ = std::fs::remove_dir_all(&temp);
            std::fs::create_dir_all(&temp).unwrap();
            let parent_file = temp.join("parent.pid");
            let grandchild_file = temp.join("grandchild.pid");
            let release_file = temp.join("test.release");
            let mut cmd = command(mode);
            cmd.env("HH_RUNNER_PARENT_PID_FILE", &parent_file)
                .env("HH_RUNNER_GRANDCHILD_PID_FILE", &grandchild_file)
                .env("HH_RUNNER_RELEASE_FILE", &release_file);
            let limits = limits(timeout, cap);
            let (tx, rx) = mpsc::channel();
            let worker = thread::spawn(move || {
                let started = Instant::now();
                let result = run_command(cmd, limits);
                let _ = tx.send((result, started.elapsed()));
            });
            let deadline = Instant::now() + Duration::from_secs(2);
            let parent_pid =
                wait_for_pid(&parent_file, deadline).expect("child did not publish a complete pid");
            std::fs::write(&release_file, b"go").unwrap();
            let grandchild_pid = wait_for_pid(&grandchild_file, deadline)
                .expect("grandchild did not publish a complete pid");
            let received = rx.recv_timeout(Duration::from_millis(1800));
            let pids = [parent_pid, grandchild_pid];
            let (result, elapsed) = match received {
                Ok(value) => value,
                Err(_) => {
                    for pid in pids {
                        terminate_test_pid(pid);
                    }
                    worker
                        .join()
                        .expect("runner worker should finish after test cleanup");
                    panic!("runner did not finish before bounded cleanup deadline")
                }
            };
            worker.join().unwrap();
            assert_eq!(
                result.unwrap_err(),
                if mode == "inherited-pipe" {
                    GitCaptureError::Timeout
                } else {
                    GitCaptureError::OutputLimit
                }
            );
            assert!(
                elapsed < Duration::from_millis(1800),
                "cleanup took {elapsed:?}"
            );
            assert!(
                wait_test_pids_dead(&pids, Duration::from_secs(2)),
                "owned helper process survived cleanup: {pids:?}"
            );
            let _ = std::fs::remove_dir_all(temp);
        }
    }
    #[test]
    fn classifies_spawn_and_exit_without_leaking_child_output() {
        let mut missing = Command::new("Z:\\definitely-not-a-real-command.exe");
        missing.current_dir(std::env::current_dir().unwrap());
        assert_eq!(
            run_command(missing, limits(Duration::from_secs(1), 1024)).unwrap_err(),
            GitCaptureError::CommandNotFound
        );
        let launch_error = run_command(
            Command::new(std::env::temp_dir()),
            limits(Duration::from_secs(1), 1024),
        )
        .unwrap_err();
        assert_eq!(launch_error, GitCaptureError::LaunchFailed);
        assert!(!format!("{launch_error:?}").contains(&std::env::temp_dir().display().to_string()));
        let failed = run_command(command("fail"), limits(Duration::from_secs(2), 4096)).unwrap();
        assert_eq!(failed.status.code(), Some(17));
        assert!(String::from_utf8_lossy(&failed.stderr).contains("SECRET_SENTINEL"));
        let safe_error = GitCaptureError::CommandFailed;
        assert!(!safe_error.to_string().contains("SECRET_SENTINEL"));
        assert!(!format!("{safe_error:?}").contains("SECRET_SENTINEL"));
    }
    #[test]
    fn configures_safe_environment_without_global_mutation() {
        let output = run_command(command("env"), limits(Duration::from_secs(2), 4096)).unwrap();
        let output = String::from_utf8_lossy(&output.stdout);
        assert!(output.contains("ENV:0|0|C|||||||"), "{output}");
    }
    #[test]
    fn drain_io_error_is_not_an_output_limit() {
        struct Broken;
        impl Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("broken"))
            }
        }
        assert!(matches!(drain(Broken, 8), Err(DrainError::Read)));
    }

    fn wait_for_pid(path: &std::path::Path, deadline: Instant) -> Option<u32> {
        while Instant::now() < deadline {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(pid) = text.parse() {
                    return Some(pid);
                }
            }
            thread::yield_now();
        }
        None
    }
    fn wait_test_pids_dead(pids: &[u32], timeout: Duration) -> bool {
        #[cfg(windows)]
        {
            crate::pty::containment::test_support::wait_until_dead(pids, timeout)
        }
        #[cfg(unix)]
        {
            let deadline = Instant::now() + timeout;
            while Instant::now() < deadline {
                if pids.iter().all(|pid| !test_pid_alive(*pid)) {
                    return true;
                }
                thread::yield_now();
            }
            pids.iter().all(|pid| !test_pid_alive(*pid))
        }
        #[cfg(all(not(windows), not(unix)))]
        {
            let _ = (pids, timeout);
            true
        }
    }
    #[cfg(unix)]
    fn test_pid_alive(pid: u32) -> bool {
        unsafe { kill(pid as i32, 0) == 0 }
    }
    #[cfg(windows)]
    fn terminate_test_pid(pid: u32) {
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output();
    }
    #[cfg(unix)]
    fn terminate_test_pid(pid: u32) {
        unsafe {
            let _ = kill(pid as i32, 9);
        }
    }
    #[cfg(unix)]
    extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
}
