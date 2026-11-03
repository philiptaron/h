//! Running `git`.
//!
//! Git's stdout is sent to stderr, so that stdout stays free for the single directory the shell
//! functions `cd` to. Commands that need git's output capture it explicitly. Every git runs free
//! of the repository h itself was started in, such as the one a hook's `GIT_DIR` names.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

/// Arguments for git of different types, such as `&str`, `String`, `OsString` and `&Path`, as
/// the one array of `&OsStr` that [`run`] and the others take:
/// `git::run(&git::args!["init", "--quiet", format, path])`.
macro_rules! args {
    ($($arg:expr),+ $(,)?) => {
        [$(::std::ffi::OsStr::new(&$arg)),+]
    };
}
pub(crate) use args;

/// Why a `git` command did not succeed.
#[derive(Debug)]
pub enum GitError {
    /// `git` could not be started at all.
    Spawn(std::io::Error),
    /// `git` ran and failed with this exit code.
    Failed { args: Vec<OsString>, code: u8 },
    /// The request made no sense, so `git` was not run.
    Invalid(String),
    /// A file or directory h works on itself, beside git, could not be made or written:
    /// `action` is what h was doing to `path`, such as `write`.
    File { action: &'static str, path: PathBuf, err: std::io::Error },
    /// What h could not do, because of `cause`.
    Context { what: String, cause: Box<GitError> },
}

impl GitError {
    /// An exit code to pass on: git's own, or 127 when it could not be run.
    pub fn code(&self) -> u8 {
        match self {
            GitError::Spawn(_) => 127,
            GitError::Failed { code, .. } => *code,
            GitError::Invalid(_) | GitError::File { .. } => 1,
            GitError::Context { cause, .. } => cause.code(),
        }
    }

    /// This error, said as what h could not do, `what`, because of it. Git has said why already,
    /// as it went. An [`GitError::Invalid`] says it all for people already and stays as it is.
    pub fn context(self, what: impl Into<String>) -> GitError {
        match self {
            GitError::Invalid(_) => self,
            cause => GitError::Context { what: what.into(), cause: Box::new(cause) },
        }
    }
}

/// The git command `args` run: the first argument that is not one of git's own options, such as
/// the `-c` settings of the identity, which say nothing about what failed.
fn subcommand(args: &[OsString]) -> Option<&OsStr> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-c" || arg == "-C" {
            iter.next();
        } else if !arg.as_bytes().starts_with(b"-") {
            return Some(arg);
        }
    }
    None
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GitError::Spawn(err) => write!(f, "failed to run git: {err}"),
            // Every argument would bury what failed under the identity and the options, and git
            // has already said why; `GIT_TRACE=1` shows the whole command.
            GitError::Failed { args, code } => match subcommand(args) {
                Some(sub) => write!(f, "git {} exited with status {code}", sub.to_string_lossy()),
                None => write!(f, "git exited with status {code}"),
            },
            GitError::Invalid(msg) => f.write_str(msg),
            GitError::File { action, path, err } => {
                write!(f, "could not {action} {}: {err}", path.display())
            }
            GitError::Context { what, cause } => write!(f, "{what}: {cause}"),
        }
    }
}

/// The [`GitError::File`] for `action`, such as `write`, failing on `path`, for `map_err`.
pub fn file_error(action: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> GitError {
    let path = path.to_path_buf();
    move |err| GitError::File { action, path, err }
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

/// The variables that tie git to one repository, which git sets for hooks and for the commands
/// of `git rebase --exec` and `git bisect run`: git's own `local_repo_env`, as printed by `git
/// rev-parse --local-env-vars`, less the configuration given with `git -c`
/// (`GIT_CONFIG_PARAMETERS` and `GIT_CONFIG_COUNT`), which git too keeps when it runs a command
/// in another repository.
pub const LOCAL_REPO_ENV: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

/// Git working on the repository in one directory, as `git -C <dir>` does, made by [`at`]. The
/// functions [`run`], [`capture_output`] and [`prepare_command`] are git working on no repository
/// in particular, as for `git init <dir>` and `git clone`. Either way, git runs free of the
/// repository h was started in.
#[derive(Debug, Clone, Copy)]
pub struct Git<'a> {
    dir: Option<&'a Path>,
}

/// Git working on the repository at `dir`.
pub fn at(dir: &Path) -> Git<'_> {
    Git { dir: Some(dir) }
}

/// Prepare a `git` command with `args`, its stdout redirected to stderr.
pub fn prepare_command(args: &[impl AsRef<OsStr>]) -> Command {
    Git { dir: None }.prepare_command(args)
}

/// Run `git` with `args`.
pub fn run(args: &[impl AsRef<OsStr>]) -> Result<(), GitError> {
    Git { dir: None }.run(args)
}

/// Run `git` with `args` and return what it printed on stdout.
pub fn capture_output(args: &[impl AsRef<OsStr>]) -> Result<String, GitError> {
    Git { dir: None }.capture_output(args)
}

impl Git<'_> {
    /// Prepare a `git` command with `args`, its stdout still attached to h's, free of the
    /// repository h was started in, so that it works on this one (or the repository it creates)
    /// and nothing else.
    fn prepare_attached_command(self, args: &[impl AsRef<OsStr>]) -> Command {
        let mut cmd = Command::new("git");
        for var in LOCAL_REPO_ENV {
            cmd.env_remove(var);
        }
        if let Some(dir) = self.dir {
            cmd.arg("-C").arg(dir);
        }
        cmd.args(args);
        cmd
    }

    /// Prepare a `git` command with `args`, its stdout redirected to stderr, so that h's stdout
    /// stays free for the directory it prints.
    pub fn prepare_command(self, args: &[impl AsRef<OsStr>]) -> Command {
        let mut cmd = self.prepare_attached_command(args);
        cmd.stdout(stderr_as_stdout());
        cmd
    }

    /// Run `git` with `args`.
    pub fn run(self, args: &[impl AsRef<OsStr>]) -> Result<(), GitError> {
        let status = self.prepare_command(args).status().map_err(GitError::Spawn)?;
        if status.success() { Ok(()) } else { Err(failed(args, status)) }
    }

    /// Run `git` with `args` and return what it printed on stdout.
    pub fn capture_output(self, args: &[impl AsRef<OsStr>]) -> Result<String, GitError> {
        let out = self.prepare_attached_command(args).output().map_err(GitError::Spawn)?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(failed(args, out.status))
        }
    }

    /// Query the repository: what `git` with `args` printed on stdout, trimmed, or `None` when it
    /// fails or prints nothing.
    pub fn query_output(self, args: &[impl AsRef<OsStr>]) -> Option<String> {
        let out = self.capture_output(args).ok()?;
        Some(out.trim().to_string()).filter(|out| !out.is_empty())
    }

    /// The commit `rev` names, if it names one.
    pub fn resolve_commit(self, rev: &str) -> Option<String> {
        self.query_output(&["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
    }

    /// Read the URL of the remote `remote` as the configuration has it, before any `insteadOf`.
    pub fn read_remote_url(self, remote: &str) -> Option<String> {
        self.query_output(&["config", "--get", &format!("remote.{remote}.url")])
    }

    /// Run `git` with `args`, feeding it `input` on stdin.
    pub fn run_with_input(self, args: &[impl AsRef<OsStr>], input: &[u8]) -> Result<(), GitError> {
        use std::io::Write;
        let mut child =
            self.prepare_command(args).stdin(Stdio::piped()).spawn().map_err(GitError::Spawn)?;
        let written = child.stdin.take().expect("stdin is piped").write_all(input);
        let status = child.wait().map_err(GitError::Spawn)?;
        if status.success() { written.map_err(GitError::Spawn) } else { Err(failed(args, status)) }
    }

    /// Run `git` with `args`, leaving stdout connected so the user sees what it prints.
    pub fn run_attached(self, args: &[impl AsRef<OsStr>]) -> Result<(), GitError> {
        let status = self.prepare_attached_command(args).status().map_err(GitError::Spawn)?;
        if status.success() { Ok(()) } else { Err(failed(args, status)) }
    }
}

/// How `git` with `args` failed, having finished with `status`.
pub fn failed(args: &[impl AsRef<OsStr>], status: ExitStatus) -> GitError {
    let args = args.iter().map(|a| a.as_ref().to_owned()).collect();
    GitError::Failed { args, code: exit_code(status) }
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

/// `-c key=value` arguments for git itself, one pair for each of `config`.
pub fn config_args(config: &[(OsString, OsString)]) -> Vec<OsString> {
    let mut args = Vec::new();
    for (key, value) in config {
        let mut setting = key.clone();
        setting.push("=");
        setting.push(value);
        args.extend([OsString::from("-c"), setting]);
    }
    args
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
    use crate::test_util::{opts, pairs, strings};

    #[test]
    fn extracts_config_pairs() {
        let got = config_pairs(&opts(&["-c", "user.name=Me", "--depth", "1", "-cx.y=z", "-c"]));
        assert_eq!(got, pairs(&[("user.name", "Me"), ("x.y", "z")]));
        assert_eq!(config_pairs(&opts(&["-c", "novalue"])), []);
    }

    #[test]
    fn config_becomes_git_options() {
        let config = pairs(&[("user.name", "Me Too"), ("credential.username", "me")]);
        assert_eq!(
            strings(config_args(&config)),
            ["-c", "user.name=Me Too", "-c", "credential.username=me"]
        );
    }

    #[test]
    fn keeps_the_other_options() {
        let got = non_config_opts(&opts(&["-c", "user.name=Me", "--depth", "1", "-cx.y=z"]));
        assert_eq!(got, opts(&["--depth", "1"]));
    }

    #[test]
    fn reports_failures() {
        let err = run(&["--no-such-option"]).unwrap_err();
        assert!(matches!(err, GitError::Failed { .. }), "{err:?}");
        assert_eq!(err.code(), 129);
        assert_eq!(err.to_string(), "git exited with status 129");
        let err = run(&["-c", "user.name=A B", "-C", "/", "no-such-command", "x"]);
        assert_eq!(err.unwrap_err().to_string(), "git no-such-command exited with status 1");
    }

    #[test]
    fn says_which_file_could_not_be_written() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("no/such/dir/alternates");
        let err = std::fs::write(&file, "x").map_err(file_error("write", &file)).unwrap_err();
        let said = format!("could not write {}: No such file or directory", file.display());
        assert!(err.to_string().starts_with(&said), "{err}");
        assert_eq!(err.code(), 1);
    }

    #[test]
    fn says_what_failed_in_context() {
        let failed = GitError::Failed { args: vec!["clone".into(), "url".into()], code: 128 };
        let err = failed.context("could not clone url");
        assert_eq!(err.to_string(), "could not clone url: git clone exited with status 128");
        assert_eq!(err.code(), 128);
        let invalid = GitError::Invalid("--x cannot be used with --container".into());
        assert_eq!(
            invalid.context("could not clone url").to_string(),
            "--x cannot be used with --container"
        );
    }

    #[test]
    fn captures_output() {
        assert!(capture_output(&["--version"]).unwrap().starts_with("git version"));
    }

    #[test]
    fn clears_what_git_clears_for_another_repository() {
        let kept = ["GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT"];
        let local = capture_output(&["rev-parse", "--local-env-vars"]).unwrap();
        let mut want: Vec<&str> = local.lines().filter(|var| !kept.contains(var)).collect();
        want.sort();
        let mut have = LOCAL_REPO_ENV.to_vec();
        have.sort();
        assert_eq!(have, want, "LOCAL_REPO_ENV is out of date with this git");

        let cmd = Git { dir: None }.prepare_attached_command(&["--version"]);
        let removed: Vec<_> = cmd.get_envs().filter(|(_, value)| value.is_none()).collect();
        assert_eq!(removed.len(), LOCAL_REPO_ENV.len(), "{removed:?}");
    }
}
