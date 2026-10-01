//! Helpers shared by the integration tests.

#![allow(dead_code)]

use std::borrow::BorrowMut;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

/// A command with an environment insulated from the user's: no proxies, no direnv.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    for var in [
        "DIRENV_DIR",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

/// The result of running a command, with output decoded as UTF-8.
#[derive(Debug)]
pub struct Run {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub fn run(mut cmd: impl BorrowMut<Command>) -> Run {
    let Output { status, stdout, stderr } =
        cmd.borrow_mut().output().expect("failed to spawn command");
    Run {
        code: status.code(),
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
    }
}

/// Create each of `dirs` (relative paths) under `root`.
pub fn mkdirs(root: &Path, dirs: &[&str]) {
    for dir in dirs {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
}

/// The canonical form of `path`, which is what `getcwd` reports.
pub fn canonical(path: &Path) -> String {
    fs::canonicalize(path).unwrap().to_str().unwrap().to_string()
}

/// Whether `program` can be found on `PATH`.
pub fn have(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// A shell command running `script` in `root`, with `$ROOT` and `$HOME` set to `root`.
pub fn shell(program: &str, args: &[&str], root: &Path, script: &str) -> Command {
    let mut cmd = command(program);
    cmd.args(args).arg(script).current_dir(root).env("HOME", root).env("ROOT", root);
    cmd
}

/// A non-interactive bash, ignoring the user's startup files.
pub fn bash(root: &Path, script: &str) -> Command {
    shell("bash", &["--norc", "--noprofile", "-c"], root, script)
}

/// A zsh ignoring the user's startup files.
pub fn zsh(root: &Path, script: &str) -> Command {
    shell("zsh", &["-f", "-c"], root, script)
}
