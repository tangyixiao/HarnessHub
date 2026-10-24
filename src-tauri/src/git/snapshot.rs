use super::{
    runner::{self, Limits},
    status::parse_status,
    GitCaptureError, StatusCounts,
};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitObservation {
    NotRepository,
    Repository(GitSnapshot),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSnapshot {
    pub worktree_root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub head: Option<String>,
    pub staged_files: u64,
    pub unstaged_files: u64,
    pub untracked_files: u64,
    pub conflicted_files: u64,
}
fn args(path: &Path, tail: &[&str]) -> Vec<OsString> {
    let mut args = vec![OsString::from("-C"), path.as_os_str().to_owned()];
    args.extend(tail.iter().map(OsString::from));
    args
}
fn git(path: &Path, tail: &[&str]) -> Result<Vec<u8>, GitCaptureError> {
    let output = raw_git(path, tail)?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(GitCaptureError::CommandFailed)
    }
}
fn raw_git(path: &Path, tail: &[&str]) -> Result<runner::Output, GitCaptureError> {
    runner::run(
        &OsString::from("git"),
        &args(path, tail),
        path,
        Limits::default(),
    )
}
fn path_output(mut bytes: Vec<u8>) -> Result<PathBuf, GitCaptureError> {
    #[cfg(windows)]
    if bytes.ends_with(b"\r\n") {
        bytes.truncate(bytes.len() - 2);
    } else if bytes.ends_with(b"\n") {
        bytes.pop();
    }
    #[cfg(unix)]
    if bytes.ends_with(b"\n") {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Err(GitCaptureError::MalformedOutput);
    }
    #[cfg(unix)]
    let p = {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(OsString::from_vec(bytes))
    };
    #[cfg(not(unix))]
    let p =
        PathBuf::from(std::str::from_utf8(&bytes).map_err(|_| GitCaptureError::MalformedOutput)?);
    if !p.is_absolute() {
        return Err(GitCaptureError::MalformedOutput);
    }
    Ok(p)
}
pub fn capture(path: &Path) -> Result<GitObservation, GitCaptureError> {
    let abs = std::fs::canonicalize(path).map_err(|_| GitCaptureError::InvalidPath)?;
    if !abs.is_dir() {
        return Err(GitCaptureError::InvalidPath);
    }
    let top_output = raw_git(&abs, &["rev-parse", "--show-toplevel"])?;
    if !top_output.status.success() {
        if is_not_repository_diagnostic(&top_output.stderr) {
            return Ok(GitObservation::NotRepository);
        }
        return Err(GitCaptureError::CommandFailed);
    }
    let top = path_output(top_output.stdout)?;
    let bare = git(&top, &["rev-parse", "--is-bare-repository"])?;
    if bare == b"true\n" {
        return Ok(GitObservation::NotRepository);
    }
    if bare != b"false\n" {
        return Err(GitCaptureError::MalformedOutput);
    }
    let gd = path_output(git(&top, &["rev-parse", "--absolute-git-dir"])?)?;
    let cd = path_output(git(
        &top,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?)?;
    let head = match git(&top, &["rev-parse", "--verify", "HEAD^{commit}"]) {
        Ok(v) => {
            let s = std::str::from_utf8(&v)
                .map_err(|_| GitCaptureError::MalformedOutput)?
                .strip_suffix('\n')
                .unwrap_or(std::str::from_utf8(&v).map_err(|_| GitCaptureError::MalformedOutput)?);
            if !((s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())) {
                return Err(GitCaptureError::MalformedOutput);
            }
            Some(s.to_owned())
        }
        Err(GitCaptureError::CommandFailed) => {
            let is_unborn = valid_unborn_head(&top, git(&top, &["symbolic-ref", "-q", "HEAD"]))?;
            if is_unborn {
                None
            } else {
                return Err(GitCaptureError::CommandFailed);
            }
        }
        Err(e) => return Err(e),
    };
    let status = git(
        &top,
        &[
            "-c",
            "core.fsmonitor=false",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    let StatusCounts {
        staged_files,
        unstaged_files,
        untracked_files,
        conflicted_files,
    } = parse_status(&status)?;
    Ok(GitObservation::Repository(GitSnapshot {
        worktree_root: top,
        git_dir: gd,
        common_dir: cd,
        head,
        staged_files,
        unstaged_files,
        untracked_files,
        conflicted_files,
    }))
}
fn valid_unborn_head(
    root: &Path,
    symref: Result<Vec<u8>, GitCaptureError>,
) -> Result<bool, GitCaptureError> {
    let symref = symref?;
    let Ok(reference) = std::str::from_utf8(&symref) else {
        return Ok(false);
    };
    let reference = reference.strip_suffix('\n').unwrap_or(reference);
    if !reference.starts_with("refs/heads/") || reference.contains('\n') {
        return Ok(false);
    }
    missing_branch_ref(raw_git(
        root,
        &["show-ref", "--verify", "--quiet", reference],
    ))
}
fn missing_branch_ref(
    probe: Result<runner::Output, GitCaptureError>,
) -> Result<bool, GitCaptureError> {
    let output = probe?;
    if output.status.success() {
        Ok(false)
    } else if output.status.code() == Some(1) && output.stderr.is_empty() {
        Ok(true)
    } else {
        Err(GitCaptureError::CommandFailed)
    }
}
fn is_not_repository_diagnostic(stderr: &[u8]) -> bool {
    stderr == b"fatal: not a git repository (or any of the parent directories): .git\n"
        || stderr == b"fatal: this operation must be run in a work tree\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn fixture_git(root: &Path) -> Command {
        let support = root.join(".git/hh-fixture");
        std::fs::create_dir_all(support.join("hooks")).unwrap();
        std::fs::create_dir_all(support.join("template")).unwrap();
        let config = support.join("empty-config");
        std::fs::write(&config, b"").unwrap();
        let mut command = Command::new("git");
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy().to_ascii_uppercase();
            if matches!(
                name.as_str(),
                "GIT_DIR"
                    | "GIT_WORK_TREE"
                    | "GIT_INDEX_FILE"
                    | "GIT_COMMON_DIR"
                    | "GIT_PREFIX"
                    | "GIT_OBJECT_DIRECTORY"
                    | "GIT_ALTERNATE_OBJECT_DIRECTORIES"
                    | "GIT_CONFIG"
                    | "GIT_CONFIG_COUNT"
                    | "GIT_CONFIG_PARAMETERS"
            ) || name.starts_with("GIT_CONFIG_KEY_")
                || name.starts_with("GIT_CONFIG_VALUE_")
            {
                command.env_remove(key);
            }
        }
        command
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", &config)
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_TEMPLATE_DIR", support.join("template"))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .args([
                "-c",
                "user.name=Harness Hub Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
            ])
            .arg("-c")
            .arg(format!(
                "core.hooksPath={}",
                support.join("hooks").display()
            ));
        command
    }

    struct Repo(PathBuf);
    impl Repo {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("hh-git-capture-{}", uuid::Uuid::new_v4()));
            Self::at(root)
        }
        fn at(root: PathBuf) -> Self {
            std::fs::create_dir_all(&root).unwrap();
            let repo = Self(root);
            repo.git(&["init", "-q"]);
            repo
        }
        fn git(&self, args: &[&str]) {
            let status = fixture_git(&self.0).args(args).status().unwrap();
            assert!(status.success());
        }
    }
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    #[ignore = "subprocess helper; parent supplies disposable paths"]
    fn fixture_environment_helper() {
        let root = PathBuf::from(std::env::var_os("HH_FIXTURE_CHILD_ROOT").unwrap());
        let owner = root.parent().unwrap();
        assert_eq!(owner.parent(), Some(std::env::temp_dir().as_path()));
        let owner_name = owner.file_name().unwrap().to_str().unwrap();
        uuid::Uuid::parse_str(owner_name.strip_prefix("hh-git-capture-").unwrap()).unwrap();
        assert!(matches!(
            root.file_name().unwrap().to_str().unwrap(),
            "child-index" | "child-repository" | "child-config"
        ));
        let repo = Repo::at(root);
        assert!(repo.0.join(".git").is_dir(), "init stays inside fixture");
        std::fs::write(repo.0.join("file"), b"fixture").unwrap();
        repo.git(&["add", "file"]);
        repo.git(&["commit", "-qm", "fixture"]);
        assert!(
            repo.0.join(".git/index").is_file(),
            "index stays inside fixture"
        );
    }

    #[test]
    fn fixture_git_isolates_inherited_repository_and_config_environment() {
        let protected = Repo::new();
        std::fs::write(protected.0.join("file"), b"protected").unwrap();
        protected.git(&["add", "file"]);
        protected.git(&["commit", "-qm", "protected"]);
        let paths = [".git/config", ".git/HEAD", ".git/index", "file"];
        let before: Vec<_> = paths
            .iter()
            .map(|p| std::fs::read(protected.0.join(p)).unwrap())
            .collect();
        for mode in ["index", "repository", "config"] {
            let mut child = Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "git::snapshot::tests::fixture_environment_helper",
                    "--ignored",
                    "--nocapture",
                ])
                .env(
                    "HH_FIXTURE_CHILD_ROOT",
                    protected.0.join(format!("child-{mode}")),
                );
            match mode {
                "index" => {
                    child.env("GIT_INDEX_FILE", protected.0.join(".git/index"));
                }
                "repository" => {
                    child
                        .env("GIT_DIR", protected.0.join(".git"))
                        .env("GIT_WORK_TREE", &protected.0)
                        .env("GIT_COMMON_DIR", protected.0.join(".git"))
                        .env("GIT_OBJECT_DIRECTORY", protected.0.join(".git/objects"))
                        .env(
                            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                            protected.0.join(".git/objects"),
                        )
                        .env("GIT_PREFIX", "polluted/");
                }
                "config" => {
                    child
                        .env("GIT_CONFIG_COUNT", "1")
                        .env("GIT_CONFIG_KEY_0", "core.bare")
                        .env("GIT_CONFIG_VALUE_0", "true")
                        .env("GIT_CONFIG_PARAMETERS", "'core.bare=true'");
                }
                _ => unreachable!(),
            }
            let output = child.output().unwrap();
            for (path, expected) in paths.iter().zip(&before) {
                assert_eq!(
                    &std::fs::read(protected.0.join(path)).unwrap(),
                    expected,
                    "protected sentinel unchanged ({mode}, {path})"
                );
            }
            assert!(
                output.status.success(),
                "isolated child {mode}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }

    #[test]
    fn existing_non_repo_is_not_repository_but_invalid_paths_are_errors() {
        let temp = Repo::new();
        let empty = std::env::temp_dir().join(format!("hh-nonrepo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&empty).unwrap();
        assert_eq!(capture(&empty).unwrap(), GitObservation::NotRepository);
        let _ = std::fs::remove_dir(&empty);
        let missing = temp.0.join("missing");
        assert_eq!(capture(&missing), Err(GitCaptureError::InvalidPath));
        let file = temp.0.join("file");
        std::fs::write(&file, b"").unwrap();
        assert_eq!(capture(&file), Err(GitCaptureError::InvalidPath));
    }

    #[test]
    fn invalid_config_is_command_failed_and_does_not_expose_stderr() {
        let repo = Repo::new();
        std::fs::write(repo.0.join(".git/config"), b"SECRET_SENTINEL_BROKEN\n").unwrap();
        let error = capture(&repo.0).unwrap_err();
        assert_eq!(error, GitCaptureError::CommandFailed);
        assert!(!format!("{error:?}{error}").contains("SECRET_SENTINEL"));
    }

    #[test]
    fn config_error_path_containing_not_repository_diagnostic_is_not_swallowed() {
        let stderr = b"fatal: bad config line 1 in file /tmp/not a git repository (or any of the parent directories)/repo/.git/config\n";
        assert!(!is_not_repository_diagnostic(stderr));
        assert!(is_not_repository_diagnostic(
            b"fatal: not a git repository (or any of the parent directories): .git\n"
        ));
    }

    #[test]
    fn unborn_head_check_propagates_runner_failures_from_both_probes() {
        let failures = [
            GitCaptureError::CommandNotFound,
            GitCaptureError::LaunchFailed,
            GitCaptureError::Timeout,
            GitCaptureError::OutputLimit,
            GitCaptureError::ReadFailed,
        ];
        for failure in failures {
            assert_eq!(
                valid_unborn_head(Path::new("unused"), Err(failure.clone())),
                Err(failure.clone())
            );
            assert_eq!(missing_branch_ref(Err(failure.clone())), Err(failure));
        }
    }

    #[test]
    fn unborn_head_is_none_and_commit_head_is_complete() {
        let repo = Repo::new();
        let GitObservation::Repository(empty) = capture(&repo.0).unwrap() else {
            panic!("expected repository")
        };
        assert_eq!(empty.head, None);
        assert_eq!(
            (
                empty.staged_files,
                empty.unstaged_files,
                empty.untracked_files,
                empty.conflicted_files
            ),
            (0, 0, 0, 0)
        );
        std::fs::write(repo.0.join("file"), b"data").unwrap();
        repo.git(&["add", "file"]);
        repo.git(&["commit", "-qm", "initial"]);
        let GitObservation::Repository(committed) = capture(&repo.0).unwrap() else {
            panic!("expected repository")
        };
        let head = committed.head.unwrap();
        let branch = String::from_utf8(
            fixture_git(&repo.0)
                .args(["symbolic-ref", "--short", "HEAD"])
                .current_dir(&repo.0)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        assert!(matches!(head.len(), 40 | 64));
        assert!(head.bytes().all(|byte| byte.is_ascii_hexdigit()));
        repo.git(&["checkout", "--detach", "-q"]);
        let GitObservation::Repository(detached) = capture(&repo.0).unwrap() else {
            panic!("expected repository")
        };
        assert_eq!(detached.head.as_deref(), Some(head.as_str()));
        std::fs::write(repo.0.join("untracked"), b"untracked").unwrap();
        let GitObservation::Repository(dirty) = capture(&repo.0).unwrap() else {
            panic!("expected repository")
        };
        assert_eq!(dirty.untracked_files, 1);
        let broken_ref = repo
            .0
            .join(".git")
            .join("refs")
            .join("heads")
            .join(branch.trim());
        std::fs::create_dir_all(broken_ref.parent().unwrap()).unwrap();
        std::fs::write(
            repo.0.join(".git").join("HEAD"),
            format!("ref: refs/heads/{}\n", branch.trim()),
        )
        .unwrap();
        std::fs::write(broken_ref, b"1111111111111111111111111111111111111111\n").unwrap();
        assert_eq!(capture(&repo.0), Err(GitCaptureError::CommandFailed));
    }

    #[test]
    fn bare_repository_is_not_a_worktree() {
        let owner = Repo::new();
        let parent = &owner.0;
        let bare = parent.join("bare.git");
        let output = fixture_git(parent)
            .args(["init", "--bare", "-q"])
            .arg(&bare)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(capture(&bare).unwrap(), GitObservation::NotRepository);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_repository_path_is_passed_as_os_string() {
        use std::os::unix::ffi::OsStringExt;
        let owner = Repo::new();
        let mut name = b"nonutf8-".to_vec();
        name.push(0xff);
        let root = owner.0.join(OsString::from_vec(name));
        std::fs::create_dir_all(&root).unwrap();
        let init = fixture_git(&root)
            .args(["init", "-q"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(init.status.success());
        assert!(matches!(capture(&root), Ok(GitObservation::Repository(_))));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn path_output_removes_only_one_terminal_newline() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        #[cfg(windows)]
        let root_text = root.to_string_lossy();
        #[cfg(windows)]
        {
            assert_eq!(
                path_output(format!("{root_text}\r\n").into_bytes()).unwrap(),
                root
            );
            assert_eq!(
                path_output(format!("{root_text}\n\n").into_bytes()).unwrap(),
                PathBuf::from(format!("{root_text}\n"))
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::{OsStrExt, OsStringExt};
            let mut one_lf = root.as_os_str().as_bytes().to_vec();
            one_lf.push(b'\n');
            assert_eq!(path_output(one_lf).unwrap(), root);
            let mut path_ending_lf = root.as_os_str().as_bytes().to_vec();
            path_ending_lf.extend_from_slice(b"\n\n");
            let mut expected = root.as_os_str().as_bytes().to_vec();
            expected.push(b'\n');
            assert_eq!(
                path_output(path_ending_lf).unwrap(),
                PathBuf::from(OsString::from_vec(expected))
            );
            let mut path_ending_cr = root.as_os_str().as_bytes().to_vec();
            path_ending_cr.extend_from_slice(b"\r\n");
            let mut expected = root.as_os_str().as_bytes().to_vec();
            expected.push(b'\r');
            assert_eq!(
                path_output(path_ending_cr).unwrap(),
                PathBuf::from(OsString::from_vec(expected))
            );
        }
    }
}
