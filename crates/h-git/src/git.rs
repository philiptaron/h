//! Running `git`.
//!
//! Git's stdout is sent to stderr, so that stdout stays free for the single directory the shell
//! functions `cd` to.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::fd::AsFd;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

/// Why a `git` command did not succeed.
#[derive(Debug)]
pub enum GitError {
    /// `git` could not be started at all.
    Spawn(std::io::Error),
    /// `git` ran and failed with this exit code.
    Failed { args: Vec<OsString>, code: u8 },
}

impl GitError {
    /// An exit code to pass on: git's own, or 127 when it could not be run.
    pub fn code(&self) -> u8 {
        match self {
            GitError::Spawn(_) => 127,
            GitError::Failed { code, .. } => *code,
        }
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::Spawn(err) => write!(f, "failed to run git: {err}"),
            GitError::Failed { args, code } => {
                let shown: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
                write!(f, "git {} failed with status {code}", shown.join(" "))
            }
        }
    }
}

/// The exit code of a finished process, or 1 when it was killed by a signal.
pub fn exit_code(status: ExitStatus) -> u8 {
    status.code().map_or(1, |code| code as u8)
}

fn stderr_as_stdout() -> Stdio {
    match std::io::stderr().as_fd().try_clone_to_owned() {
        Ok(fd) => Stdio::from(fd),
        Err(_) => Stdio::null(),
    }
}

/// A `git` command running in `dir` (with `-C`), its stdout redirected to stderr.
pub fn command(dir: Option<&Path>, args: &[impl AsRef<OsStr>]) -> Command {
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    cmd.args(args).stdout(stderr_as_stdout());
    cmd
}

/// Run `git` with `args`, in `dir` if given.
pub fn run(dir: Option<&Path>, args: &[impl AsRef<OsStr>]) -> Result<(), GitError> {
    let status = command(dir, args).status().map_err(GitError::Spawn)?;
    if status.success() {
        Ok(())
    } else {
        let args = args.iter().map(|a| a.as_ref().to_owned()).collect();
        Err(GitError::Failed { args, code: exit_code(status) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_failures() {
        let err = run(None, &["--no-such-option"]).unwrap_err();
        assert!(matches!(err, GitError::Failed { .. }), "{err:?}");
        assert_eq!(err.code(), 129);
        assert_eq!(err.to_string(), "git --no-such-option failed with status 129");
    }
}
