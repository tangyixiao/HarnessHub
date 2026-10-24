use harness_hub_lib::git::{capture, GitCaptureError, GitObservation, GitSnapshot};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    root: PathBuf,
    hooks: PathBuf,
    template: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("hh-git-integration-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).expect("create fixture root");
        let hooks = root.join("empty-hooks");
        let template = root.join("empty-template");
        fs::create_dir(&hooks).expect("create empty hooks");
        fs::create_dir(&template).expect("create empty template");
        Self {
            root,
            hooks,
            template,
        }
    }
    fn command(&self, repo: &Path, args: &[&str]) -> Output {
        let mut command = Command::new("git");
        command
            .args([
                "-c",
                "user.name=Harness Hub Test",
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
                "-c",
                "core.autocrlf=false",
            ])
            .arg("-c")
            .arg(format!("core.hooksPath={}", self.hooks.display()))
            .arg("-C")
            .arg(repo)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join("no-global-config"))
            .env("GIT_TEMPLATE_DIR", &self.template)
            .env("GIT_TERMINAL_PROMPT", "0");
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_PARAMETERS",
        ] {
            command.env_remove(key);
        }
        command.output().expect("launch fixture Git")
    }
    fn git(&self, repo: &Path, args: &[&str]) -> Vec<u8> {
        let result = self.command(repo, args);
        assert!(result.status.success(), "fixture Git stage failed");
        result.stdout
    }
    fn repo(&self, name: &str, sha256: bool) -> PathBuf {
        let repo = self.root.join(name);
        fs::create_dir(&repo).expect("create fixture repo");
        let format = if sha256 {
            "--object-format=sha256"
        } else {
            "--object-format=sha1"
        };
        self.git(&repo, &["init", "-q", "--initial-branch=main", format]);
        repo
    }
    fn write(&self, repo: &Path, name: &str, content: &[u8]) {
        let path = repo.join(name);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
        fs::write(path, content).expect("write fixture file");
    }
    fn commit(&self, repo: &Path) {
        self.git(repo, &["add", "--all"]);
        self.git(repo, &["commit", "-qm", "fixture"]);
    }
    fn committed(&self) -> PathBuf {
        let repo = self.repo("repo", false);
        self.write(&repo, "tracked", b"base\n");
        self.commit(&repo);
        repo
    }
    fn head(&self, repo: &Path) -> String {
        String::from_utf8(self.git(repo, &["rev-parse", "--verify", "HEAD^{commit}"]))
            .expect("ASCII fixture object ID")
            .trim_end()
            .to_owned()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn snapshot(path: &Path) -> GitSnapshot {
    match capture(path).expect("production capture succeeds") {
        GitObservation::Repository(snapshot) => snapshot,
        GitObservation::NotRepository => panic!("expected fixture repository"),
    }
}
fn counts(s: &GitSnapshot) -> (u64, u64, u64, u64) {
    (
        s.staged_files,
        s.unstaged_files,
        s.untracked_files,
        s.conflicted_files,
    )
}
fn same_path(a: &Path, b: &Path) {
    assert!(
        a.canonicalize().expect("canonical capture path")
            == b.canonicalize().expect("canonical fixture path"),
        "paths refer to the same directory"
    );
}

#[test]
fn clean_root_subdirectory_and_complete_head() {
    let f = Fixture::new();
    let repo = f.committed();
    fs::create_dir(repo.join("subdirectory")).expect("create subdirectory");
    let root = snapshot(&repo);
    let child = snapshot(&repo.join("subdirectory"));
    assert!(root == child, "root and subdirectory observations agree");
    assert_eq!(counts(&root), (0, 0, 0, 0));
    assert!(
        root.head.as_deref() == Some(f.head(&repo).as_str()),
        "complete HEAD agrees"
    );
    assert!(
        root.worktree_root.is_absolute()
            && root.git_dir.is_absolute()
            && root.common_dir.is_absolute()
    );
    same_path(&root.worktree_root, &repo);
    same_path(&root.git_dir, &root.common_dir);
}

#[test]
fn partially_staged_file_and_each_untracked_file_excluding_ignored() {
    let f = Fixture::new();
    let repo = f.committed();
    f.write(&repo, ".gitignore", b"ignored/\n");
    f.commit(&repo);
    f.write(&repo, "tracked", b"staged\n");
    f.git(&repo, &["add", "tracked"]);
    f.write(&repo, "tracked", b"working\n");
    f.write(&repo, "new/a", b"a");
    f.write(&repo, "new/b", b"b");
    f.write(&repo, "ignored/secret", b"ignored");
    assert_eq!(counts(&snapshot(&repo)), (1, 1, 2, 0));
}

#[test]
fn staged_rename_counts_once() {
    let f = Fixture::new();
    let repo = f.committed();
    f.git(&repo, &["mv", "tracked", "renamed"]);
    assert_eq!(counts(&snapshot(&repo)), (1, 0, 0, 0));
}

#[test]
fn real_merge_conflict_is_exclusive() {
    let f = Fixture::new();
    let repo = f.committed();
    f.git(&repo, &["checkout", "-qb", "other"]);
    f.write(&repo, "tracked", b"other\n");
    f.commit(&repo);
    f.git(&repo, &["checkout", "-q", "main"]);
    f.write(&repo, "tracked", b"main\n");
    f.commit(&repo);
    let merge = f.command(&repo, &["merge", "--no-edit", "other"]);
    assert_eq!(merge.status.code(), Some(1), "fixture merge must conflict");
    assert_eq!(counts(&snapshot(&repo)), (0, 0, 0, 1));
}

#[test]
fn detached_head_and_linked_worktree_share_only_common_directory() {
    let f = Fixture::new();
    let repo = f.committed();
    let expected = f.head(&repo);
    f.git(&repo, &["checkout", "--detach", "-q"]);
    let main = snapshot(&repo);
    assert!(main.head.as_deref() == Some(expected.as_str()));
    let linked = f.root.join("linked");
    f.git(
        &repo,
        &[
            "worktree",
            "add",
            "--detach",
            "-q",
            linked.to_str().expect("fixture path UTF-8"),
        ],
    );
    let other = snapshot(&linked);
    same_path(&other.worktree_root, &linked);
    assert!(main.worktree_root != other.worktree_root && main.git_dir != other.git_dir);
    same_path(&main.common_dir, &other.common_dir);
    assert!(other.head.as_deref() == Some(expected.as_str()));
    assert_eq!(counts(&other), (0, 0, 0, 0));
}

#[test]
fn cold_unborn_invalid_nonrepository_bare_and_corrupt_head() {
    let f = Fixture::new();
    let repo = f.repo("unborn", false);
    let unborn = snapshot(&repo);
    assert!(unborn.head.is_none());
    assert_eq!(counts(&unborn), (0, 0, 0, 0));
    let plain = f.root.join("plain");
    fs::create_dir(&plain).expect("create nonrepository");
    assert!(matches!(capture(&plain), Ok(GitObservation::NotRepository)));
    assert!(matches!(
        capture(&f.root.join("missing")),
        Err(GitCaptureError::InvalidPath)
    ));
    f.write(&plain, "file", b"file");
    assert!(matches!(
        capture(&plain.join("file")),
        Err(GitCaptureError::InvalidPath)
    ));
    let bare = f.root.join("bare");
    fs::create_dir(&bare).expect("create bare root");
    f.git(&bare, &["init", "--bare", "-q"]);
    assert!(matches!(capture(&bare), Ok(GitObservation::NotRepository)));
    f.write(&repo, "tracked", b"base\n");
    f.commit(&repo);
    fs::write(
        repo.join(".git/HEAD"),
        b"1111111111111111111111111111111111111111\n",
    )
    .expect("corrupt fixture HEAD");
    assert!(matches!(
        capture(&repo),
        Err(GitCaptureError::CommandFailed)
    ));
}

#[test]
fn capture_preserves_head_index_working_bytes_and_never_executes_fsmonitor() {
    let f = Fixture::new();
    let repo = f.committed();
    f.write(&repo, "tracked", b"staged\n");
    f.git(&repo, &["add", "tracked"]);
    f.write(&repo, "tracked", b"working\n");
    f.write(&repo, "untracked", b"untracked bytes\0\xff");
    let sentinel = f.root.join("fsmonitor-fired");
    let script = f.root.join("fsmonitor-hook");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf fired > '{}'\nprintf 'token\\0'\n",
            sentinel.to_string_lossy().replace('\\', "/")
        ),
    )
    .expect("write fsmonitor hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))
            .expect("make hook executable");
    }
    // Prove the configured hook can execute before testing that capture suppresses it.
    f.git(
        &repo,
        &[
            "config",
            "core.fsmonitor",
            &script.to_string_lossy().replace('\\', "/"),
        ],
    );
    f.git(&repo, &["status", "--porcelain=v1"]);
    assert!(sentinel.exists(), "fsmonitor positive control executes");
    fs::remove_file(&sentinel).expect("reset fsmonitor sentinel");
    let head_before = f.head(&repo);
    let paths = [
        repo.join(".git/HEAD"),
        repo.join(".git/index"),
        repo.join("tracked"),
        repo.join("untracked"),
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|p| fs::read(p).expect("read before capture"))
        .collect();
    assert_eq!(counts(&snapshot(&repo)), (1, 1, 1, 0));
    for (path, expected) in paths.iter().zip(before) {
        assert!(
            fs::read(path).expect("read after capture") == expected,
            "capture preserves bytes"
        );
    }
    assert!(
        !sentinel.exists(),
        "capture disables executable fsmonitor hook"
    );
    assert!(
        f.head(&repo) == head_before,
        "capture preserves commit object ID"
    );
}

#[test]
fn real_sha256_repository_returns_complete_64_hex_head() {
    let f = Fixture::new();
    let repo = f.repo("sha256", true);
    f.write(&repo, "tracked", b"SHA256 fixture\n");
    f.commit(&repo);
    let s = snapshot(&repo);
    let head = s.head.expect("committed SHA256 HEAD");
    assert_eq!(head.len(), 64);
    assert!(head.bytes().all(|b| b.is_ascii_hexdigit()));
    assert!(head == f.head(&repo), "SHA256 HEAD agrees");
}

#[test]
fn current_repository_read_only_smoke() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("repository parent");
    let s = snapshot(root);
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--verify", "HEAD^{commit}"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .expect("read smoke HEAD");
    assert!(output.status.success(), "read smoke HEAD succeeds");
    let expected = String::from_utf8(output.stdout).expect("ASCII smoke HEAD");
    assert!(
        s.head.as_deref() == Some(expected.trim_end()),
        "smoke HEAD agrees"
    );
    assert!(s.worktree_root.is_absolute() && s.git_dir.is_absolute() && s.common_dir.is_absolute());
}

#[cfg(unix)]
#[test]
fn real_newline_filename_is_counted_once() {
    let f = Fixture::new();
    let repo = f.committed();
    f.write(&repo, "line\nbreak", b"content");
    assert_eq!(counts(&snapshot(&repo)), (0, 0, 1, 0));
}
