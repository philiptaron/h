//! What the binaries print.
//!
//! The shell functions from `h-shell-init` and `up-shell-init` `cd` to whatever the binaries
//! print on stdout, so stdout carries exactly one directory and messages go to stderr. On
//! failure the current directory is printed, which keeps the user where they are.

use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::ExitCode;

/// Print a path followed by a newline to stdout, preserving non-UTF-8 bytes.
pub fn print_path(path: &Path) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(path.as_os_str().as_bytes());
    let _ = out.write_all(b"\n");
}

/// Print the current directory to stdout, if it can be determined.
pub fn print_cwd() {
    if let Ok(cwd) = std::env::current_dir() {
        print_path(&cwd);
    }
}

/// Print `msg` to stderr and return a failing exit code.
pub fn fail(msg: &str) -> ExitCode {
    eprintln!("{msg}");
    ExitCode::FAILURE
}

/// Print the current directory to stdout and `msg` to stderr, then return a failing exit code.
pub fn fail_with_cwd(msg: &str) -> ExitCode {
    print_cwd();
    fail(msg)
}
