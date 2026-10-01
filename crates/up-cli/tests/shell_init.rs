//! Tests for `up-shell-init`, including running its output in real shells.

mod common;

use std::path::Path;
use std::process::Command;

use common::*;

const UP: &str = env!("CARGO_BIN_EXE_up");
const UP_SHELL_INIT: &str = env!("CARGO_BIN_EXE_up-shell-init");

/// A bash with the binaries under test available as variables.
fn bash(root: &Path, script: &str) -> Command {
    let mut cmd = common::bash(root, script);
    cmd.env("UP", UP).env("UP_SHELL_INIT", UP_SHELL_INIT);
    cmd
}

/// A zsh with the binaries under test available as variables.
fn zsh(root: &Path, script: &str) -> Command {
    let mut cmd = common::zsh(root, script);
    cmd.env("UP", UP).env("UP_SHELL_INIT", UP_SHELL_INIT);
    cmd
}

#[test]
fn up_init_output() {
    let out = run(command(UP_SHELL_INIT));
    assert_eq!(out.code, Some(0));
    assert_eq!(
        out.stdout,
        format!(
            "up() {{\n  _up_dir=$(command {UP} \"$@\")\n  if [ $? = 0 ]; then\n    \
             [ \"$_up_dir\" != \"$PWD\" ] && cd \"$_up_dir\"\n  fi\n}}\n"
        )
    );
}

#[test]
fn up_init_help_and_errors() {
    let out = run(command(UP_SHELL_INIT).arg("--help"));
    assert_eq!(
        (out.code, out.stdout.as_str()),
        (Some(0), "Usage: eval \"$(up-shell-init [--pushd])\"\n")
    );
    let out = run(command(UP_SHELL_INIT).arg("--bogus"));
    assert_eq!((out.code, out.stderr.as_str()), (Some(1), "Unknown option: --bogus\n"));
}

#[test]
fn bash_up_changes_directory() {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["proj/.git", "proj/a/b"]);
    let out = run(bash(
        tmp.path(),
        r#"eval "$("$UP_SHELL_INIT")"
           cd proj/a/b; up; pwd"#,
    ));
    assert_eq!(out.stderr, "");
    assert_eq!(out.stdout, format!("{}\n", tmp.path().join("proj").display()));
}

#[test]
fn zsh_up_changes_directory() {
    if !have("zsh") {
        eprintln!("zsh not found; skipping");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["proj/.git", "proj/a/b"]);
    let out = run(zsh(
        tmp.path(),
        r#"eval "$("$UP_SHELL_INIT")"
           cd proj/a/b; up; pwd"#,
    ));
    assert_eq!(out.stderr, "");
    assert_eq!(out.stdout, format!("{}\n", tmp.path().join("proj").display()));
}
