//! Running `git`.
//!
//! Git's stdout is sent to stderr, so that stdout stays free for the single directory the shell
//! functions `cd` to. Commands that need git's output capture it explicitly.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
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

/// Run `git` with `args` and return what it printed on stdout.
pub fn output(dir: Option<&Path>, args: &[impl AsRef<OsStr>]) -> Result<String, GitError> {
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    let out = cmd.args(args).output().map_err(GitError::Spawn)?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let args = args.iter().map(|a| a.as_ref().to_owned()).collect();
        Err(GitError::Failed { args, code: exit_code(out.status) })
    }
}

/// Run `git` with `args`, leaving stdout connected so the user sees what it prints.
pub fn passthrough(dir: Option<&Path>, args: &[impl AsRef<OsStr>]) -> Result<(), GitError> {
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    let status = cmd.args(args).status().map_err(GitError::Spawn)?;
    if status.success() {
        Ok(())
    } else {
        let args = args.iter().map(|a| a.as_ref().to_owned()).collect();
        Err(GitError::Failed { args, code: exit_code(status) })
    }
}

/// The `key=value` pairs given as `-c key=value` (or `-ckey=value`) among git options.
///
/// These are what the shell functions carry: the identity to clone with. `git clone -c` writes
/// them into the new repository, and repositories created another way get them through
/// `git config` instead.
pub fn config_pairs(opts: &[OsString]) -> Vec<(OsString, OsString)> {
    let mut pairs = Vec::new();
    let mut iter = opts.iter();
    while let Some(opt) = iter.next() {
        let bytes = opt.as_bytes();
        let setting = if bytes == b"-c" {
            match iter.next() {
                Some(next) => next.as_bytes(),
                None => break,
            }
        } else if let Some(rest) = bytes.strip_prefix(b"-c") {
            rest
        } else {
            continue;
        };
        if let Some(eq) = setting.iter().position(|&b| b == b'=') {
            let key = OsStr::from_bytes(&setting[..eq]).to_owned();
            let value = OsStr::from_bytes(&setting[eq + 1..]).to_owned();
            pairs.push((key, value));
        }
    }
    pairs
}

/// The git options that are not `-c key=value` pairs.
pub fn non_config_opts(opts: &[OsString]) -> Vec<OsString> {
    let mut rest = Vec::new();
    let mut iter = opts.iter();
    while let Some(opt) = iter.next() {
        let bytes = opt.as_bytes();
        if bytes == b"-c" {
            iter.next();
        } else if !bytes.starts_with(b"-c") {
            rest.push(opt.clone());
        }
    }
    rest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn extracts_config_pairs() {
        let got = config_pairs(&opts(&["-c", "user.name=Me", "--depth", "1", "-cx.y=z", "-c"]));
        let want = [("user.name", "Me"), ("x.y", "z")];
        let want: Vec<(OsString, OsString)> =
            want.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect();
        assert_eq!(got, want);
        assert_eq!(config_pairs(&opts(&["-c", "novalue"])), []);
    }

    #[test]
    fn keeps_the_other_options() {
        let got = non_config_opts(&opts(&["-c", "user.name=Me", "--depth", "1", "-cx.y=z"]));
        assert_eq!(got, opts(&["--depth", "1"]));
    }

    #[test]
    fn reports_failures() {
        let err = run(None, &["--no-such-option"]).unwrap_err();
        assert!(matches!(err, GitError::Failed { .. }), "{err:?}");
        assert_eq!(err.code(), 129);
        assert_eq!(err.to_string(), "git --no-such-option failed with status 129");
    }

    #[test]
    fn captures_output() {
        assert!(output(None, &["--version"]).unwrap().starts_with("git version"));
    }
}
