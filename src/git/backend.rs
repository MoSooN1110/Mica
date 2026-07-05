use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use super::{DiffTarget, FileDiff, GitBranch, GitStatus, parse_porcelain_v2, parse_unified_diff};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError {
    pub args: Vec<String>,
    pub exit_code: Option<i32>,
    pub stderr: String,
}

impl fmt::Display for GitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let detail = self.stderr.trim();
        let command = if self.args.is_empty() {
            "git".to_owned()
        } else {
            format!("git {}", self.args.join(" "))
        };
        let exit = self.exit_code.map_or_else(
            || "could not start".to_owned(),
            |code| format!("exit {code}"),
        );
        if detail.is_empty() {
            write!(formatter, "{command} failed ({exit})")
        } else {
            write!(formatter, "{command} failed ({exit}): {detail}")
        }
    }
}

impl std::error::Error for GitError {}

pub trait GitBackend: Send + Sync {
    fn repository_root(&self) -> Result<PathBuf, GitError>;
    fn status(&self) -> Result<GitStatus, GitError>;
    fn diff(&self, path: &Path, target: DiffTarget) -> Result<FileDiff, GitError>;
    fn stage_file(&self, path: &Path) -> Result<(), GitError>;
    fn unstage_file(&self, path: &Path) -> Result<(), GitError>;
    fn restore_file(&self, path: &Path) -> Result<(), GitError>;
    fn stage_hunk(&self, patch: &str) -> Result<(), GitError>;
    fn unstage_hunk(&self, patch: &str) -> Result<(), GitError>;
    fn restore_hunk(&self, patch: &str) -> Result<(), GitError>;
    fn commit(&self, message: &str) -> Result<(), GitError>;
    fn branches(&self) -> Result<Vec<GitBranch>, GitError>;
    fn switch_branch(&self, branch: &str) -> Result<(), GitError>;
    fn create_branch(&self, branch: &str) -> Result<(), GitError>;
    fn fetch(&self) -> Result<(), GitError>;
    fn pull(&self) -> Result<(), GitError>;
    fn push(&self) -> Result<(), GitError>;
}

#[derive(Debug, Clone)]
pub struct GitCliBackend {
    root: PathBuf,
}

impl GitCliBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn run<I, S>(&self, args: I) -> Result<Output, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_with_input(args, None)
    }

    fn run_with_input<I, S>(&self, args: I, input: Option<&[u8]>) -> Result<Output, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.run_with_input_allow(args, input, &[0])
    }

    fn run_with_input_allow<I, S>(
        &self,
        args: I,
        input: Option<&[u8]>,
        allowed_exit_codes: &[i32],
    ) -> Result<Output, GitError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args = args
            .into_iter()
            .map(|arg| arg.as_ref().to_os_string())
            .collect::<Vec<_>>();
        let mut command = Command::new("git");
        command
            .current_dir(&self.root)
            .args(&args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if input.is_some() {
            command.stdin(Stdio::piped());
        }
        let mut child = command.spawn().map_err(|error| GitError {
            args: display_args(&args),
            exit_code: None,
            stderr: error.to_string(),
        })?;
        if let Some(input) = input {
            let write_result = child
                .stdin
                .take()
                .ok_or_else(|| GitError {
                    args: display_args(&args),
                    exit_code: None,
                    stderr: "git stdin was unavailable".to_owned(),
                })?
                .write_all(input);
            if let Err(error) = write_result {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GitError {
                    args: display_args(&args),
                    exit_code: None,
                    stderr: error.to_string(),
                });
            }
        }
        let output = child.wait_with_output().map_err(|error| GitError {
            args: display_args(&args),
            exit_code: None,
            stderr: error.to_string(),
        })?;
        if output
            .status
            .code()
            .is_some_and(|code| allowed_exit_codes.contains(&code))
        {
            Ok(output)
        } else {
            Err(GitError {
                args: display_args(&args),
                exit_code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }

    fn ensure_repository(&self) -> Result<(), GitError> {
        self.repository_root().map(|_| ())
    }

    fn apply_patch(&self, patch: &str, cached: bool, reverse: bool) -> Result<(), GitError> {
        self.ensure_repository()?;
        let mut args = vec![OsString::from("apply"), OsString::from("--unidiff-zero")];
        if cached {
            args.push(OsString::from("--cached"));
        }
        if reverse {
            args.push(OsString::from("--reverse"));
        }
        args.push(OsString::from("-"));
        self.run_with_input(args, Some(patch.as_bytes()))
            .map(|_| ())
    }
}

impl GitBackend for GitCliBackend {
    fn repository_root(&self) -> Result<PathBuf, GitError> {
        let output = self.run(["rev-parse", "--show-toplevel"])?;
        Ok(PathBuf::from(
            String::from_utf8_lossy(&output.stdout).trim(),
        ))
    }

    fn status(&self) -> Result<GitStatus, GitError> {
        let output = self.run(["status", "--porcelain=v2", "--branch", "-z"])?;
        Ok(parse_porcelain_v2(&output.stdout))
    }

    fn diff(&self, path: &Path, target: DiffTarget) -> Result<FileDiff, GitError> {
        let mut args = vec![
            OsString::from("diff"),
            OsString::from("--no-ext-diff"),
            OsString::from("--no-color"),
        ];
        if target == DiffTarget::Staged {
            args.push(OsString::from("--cached"));
        }
        args.push(OsString::from("--"));
        args.push(path.as_os_str().to_os_string());
        let mut output = self.run(args)?;
        if target == DiffTarget::WorkingTree
            && output.stdout.is_empty()
            && self
                .run([
                    OsStr::new("ls-files"),
                    OsStr::new("--error-unmatch"),
                    OsStr::new("--"),
                    path.as_os_str(),
                ])
                .is_err()
        {
            output = self.run_with_input_allow(
                [
                    OsStr::new("diff"),
                    OsStr::new("--no-index"),
                    OsStr::new("--no-color"),
                    OsStr::new("--"),
                    OsStr::new("/dev/null"),
                    path.as_os_str(),
                ],
                None,
                &[0, 1],
            )?;
        }
        Ok(parse_unified_diff(
            path.to_path_buf(),
            target,
            String::from_utf8_lossy(&output.stdout).into_owned(),
        ))
    }

    fn stage_file(&self, path: &Path) -> Result<(), GitError> {
        self.ensure_repository()?;
        self.run([OsStr::new("add"), OsStr::new("--"), path.as_os_str()])
            .map(|_| ())
    }

    fn unstage_file(&self, path: &Path) -> Result<(), GitError> {
        self.ensure_repository()?;
        self.run([
            OsStr::new("restore"),
            OsStr::new("--staged"),
            OsStr::new("--"),
            path.as_os_str(),
        ])
        .map(|_| ())
    }

    fn restore_file(&self, path: &Path) -> Result<(), GitError> {
        self.ensure_repository()?;
        self.run([
            OsStr::new("restore"),
            OsStr::new("--worktree"),
            OsStr::new("--"),
            path.as_os_str(),
        ])
        .map(|_| ())
    }

    fn stage_hunk(&self, patch: &str) -> Result<(), GitError> {
        self.apply_patch(patch, true, false)
    }

    fn unstage_hunk(&self, patch: &str) -> Result<(), GitError> {
        self.apply_patch(patch, true, true)
    }

    fn restore_hunk(&self, patch: &str) -> Result<(), GitError> {
        self.apply_patch(patch, false, true)
    }

    fn commit(&self, message: &str) -> Result<(), GitError> {
        if message.trim().is_empty() {
            return Err(GitError {
                args: vec!["commit".to_owned()],
                exit_code: None,
                stderr: "commit message cannot be empty".to_owned(),
            });
        }
        self.ensure_repository()?;
        self.run_with_input(["commit", "-F", "-"], Some(message.as_bytes()))
            .map(|_| ())
    }

    fn branches(&self) -> Result<Vec<GitBranch>, GitError> {
        let output = self.run([
            "for-each-ref",
            "--format=%(refname:short)%00%(HEAD)%00%(upstream:short)%00%(refname)",
            "refs/heads",
            "refs/remotes",
        ])?;
        let mut branches = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(parse_branch)
            .collect::<Vec<_>>();
        branches.sort_by(|left, right| {
            right
                .current
                .cmp(&left.current)
                .then(left.remote.cmp(&right.remote))
                .then(left.name.cmp(&right.name))
        });
        Ok(branches)
    }

    fn switch_branch(&self, branch: &str) -> Result<(), GitError> {
        self.ensure_repository()?;
        self.run(["switch", "--", branch]).map(|_| ())
    }

    fn create_branch(&self, branch: &str) -> Result<(), GitError> {
        self.ensure_repository()?;
        self.run(["switch", "-c", branch]).map(|_| ())
    }

    fn fetch(&self) -> Result<(), GitError> {
        self.ensure_repository()?;
        self.run(["fetch", "--all", "--prune"]).map(|_| ())
    }

    fn pull(&self) -> Result<(), GitError> {
        self.ensure_repository()?;
        self.run(["pull", "--ff-only"]).map(|_| ())
    }

    fn push(&self) -> Result<(), GitError> {
        self.ensure_repository()?;
        self.run(["push"]).map(|_| ())
    }
}

fn display_args(args: &[OsString]) -> Vec<String> {
    args.iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn parse_branch(line: &str) -> Option<GitBranch> {
    let mut fields = line.split('\0');
    let name = fields.next()?.to_owned();
    let current = fields.next()? == "*";
    let upstream = fields
        .next()
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let full_name = fields.next()?;
    Some(GitBranch {
        name,
        current,
        remote: full_name.starts_with("refs/remotes/"),
        upstream,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    struct TestRepo(PathBuf);

    impl TestRepo {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path =
                std::env::temp_dir().join(format!("mica-git-{}-{nonce}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            let repo = Self(path);
            repo.git(&["init", "-q"]);
            repo.git(&["config", "user.name", "Mica Tests"]);
            repo.git(&["config", "user.email", "mica@example.invalid"]);
            fs::write(repo.0.join("tracked.txt"), "first\nsecond\n").unwrap();
            repo.git(&["add", "tracked.txt"]);
            repo.git(&["commit", "-q", "-m", "initial"]);
            repo
        }

        fn backend(&self) -> GitCliBackend {
            GitCliBackend::new(&self.0)
        }

        fn git(&self, args: &[&str]) {
            assert!(
                Command::new("git")
                    .current_dir(&self.0)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
    }

    impl Drop for TestRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn status_diff_stage_unstage_restore_and_commit() {
        let repo = TestRepo::new();
        let backend = repo.backend();
        fs::write(repo.0.join("tracked.txt"), "first\nchanged\n").unwrap();
        fs::write(repo.0.join("new.txt"), "new\n").unwrap();

        let status = backend.status().unwrap();
        assert_eq!(status.files.len(), 2);
        let diff = backend
            .diff(Path::new("tracked.txt"), DiffTarget::WorkingTree)
            .unwrap();
        assert_eq!(diff.hunks.len(), 1);
        let untracked = backend
            .diff(Path::new("new.txt"), DiffTarget::WorkingTree)
            .unwrap();
        assert_eq!(untracked.hunks.len(), 1);
        assert!(untracked.raw.contains("+new"));

        backend.stage_file(Path::new("tracked.txt")).unwrap();
        assert!(
            backend
                .status()
                .unwrap()
                .files
                .iter()
                .any(|file| file.staged)
        );
        backend.unstage_file(Path::new("tracked.txt")).unwrap();
        backend.restore_file(Path::new("tracked.txt")).unwrap();
        assert_eq!(
            fs::read_to_string(repo.0.join("tracked.txt")).unwrap(),
            "first\nsecond\n"
        );

        backend.stage_file(Path::new("new.txt")).unwrap();
        backend.commit("add new file").unwrap();
        assert!(backend.status().unwrap().files.is_empty());
    }

    #[test]
    fn hunk_operations_and_branch_switching_work() {
        let repo = TestRepo::new();
        let backend = repo.backend();
        fs::write(repo.0.join("tracked.txt"), "first\nchanged\n").unwrap();
        let patch = backend
            .diff(Path::new("tracked.txt"), DiffTarget::WorkingTree)
            .unwrap()
            .hunks
            .remove(0)
            .patch;

        backend.stage_hunk(&patch).unwrap();
        assert!(
            backend
                .diff(Path::new("tracked.txt"), DiffTarget::WorkingTree)
                .unwrap()
                .raw
                .is_empty()
        );
        let staged_patch = backend
            .diff(Path::new("tracked.txt"), DiffTarget::Staged)
            .unwrap()
            .hunks
            .remove(0)
            .patch;
        backend.unstage_hunk(&staged_patch).unwrap();
        backend.restore_hunk(&patch).unwrap();

        backend.create_branch("feature").unwrap();
        assert_eq!(backend.status().unwrap().branch.as_deref(), Some("feature"));
        assert!(
            backend
                .branches()
                .unwrap()
                .iter()
                .any(|branch| branch.current && branch.name == "feature")
        );
    }

    #[test]
    fn empty_commit_message_is_rejected_before_running_git() {
        let error = GitCliBackend::new("/missing").commit("  ").unwrap_err();
        assert!(error.stderr.contains("cannot be empty"));
    }

    #[test]
    fn fetch_pull_and_push_use_configured_upstream() {
        let repo = TestRepo::new();
        let backend = repo.backend();
        let remote = repo.0.join(".git/test-remote.git");
        assert!(
            Command::new("git")
                .args(["init", "--bare", "-q"])
                .arg(&remote)
                .status()
                .unwrap()
                .success()
        );
        repo.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
        repo.git(&["push", "-q", "--set-upstream", "origin", "HEAD"]);

        backend.fetch().unwrap();
        backend.pull().unwrap();
        backend.push().unwrap();
    }
}
