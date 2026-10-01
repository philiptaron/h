//! End-to-end tests for the `up` binary.

mod common;

use std::path::Path;

use common::*;

const UP: &str = env!("CARGO_BIN_EXE_up");

fn up(cwd: &Path, home: &Path) -> Run {
    run(command(UP).current_dir(cwd).env("PWD", cwd).env("HOME", home))
}

fn assert_printed(run: &Run, path: &Path) {
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(run.stdout, format!("{}\n", path.display()));
    assert_eq!(run.stderr, "");
}

fn projects() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["home", "outer/.hg", "outer/inner/.git", "outer/inner/src/deep", "plain"]);
    tmp
}

#[test]
fn help_reports_not_installed() {
    let tmp = tempfile::tempdir().unwrap();
    for flag in ["-h", "--help"] {
        let run = run(command(UP).current_dir(tmp.path()).arg(flag));
        assert_eq!(run.code, Some(1));
        assert_eq!(run.stdout, format!("{}\n", canonical(tmp.path())));
        assert_eq!(
            run.stderr,
            "up is not installed\n\nUsage: eval \"$(up-shell-init [--pushd])\"\n"
        );
    }
}

#[test]
fn climbs_to_project_root() {
    let tmp = projects();
    let run = up(&tmp.path().join("outer/inner/src/deep"), &tmp.path().join("home"));
    assert_printed(&run, &tmp.path().join("outer/inner"));
}

#[test]
fn climbs_out_of_nested_projects() {
    let tmp = projects();
    let run = up(&tmp.path().join("outer/inner"), &tmp.path().join("home"));
    assert_printed(&run, &tmp.path().join("outer"));
}

#[test]
fn stays_put_outside_projects() {
    let tmp = projects();
    let cwd = tmp.path().join("plain");
    assert_printed(&up(&cwd, &tmp.path().join("home")), &cwd);
}

#[test]
fn stops_at_home() {
    let tmp = projects();
    let cwd = tmp.path().join("outer/inner/src/deep");
    assert_printed(&up(&cwd, &tmp.path().join("outer/inner/src")), &cwd);
}

#[test]
fn honors_direnv_dir() {
    let tmp = projects();
    let cwd = tmp.path().join("plain");
    let run = run(command(UP)
        .current_dir(&cwd)
        .env("PWD", &cwd)
        .env("HOME", tmp.path().join("home"))
        .env("DIRENV_DIR", format!("-{}", tmp.path().display())));
    assert_printed(&run, tmp.path());
}

#[test]
fn follows_logical_pwd_through_symlinks() {
    let tmp = projects();
    std::os::unix::fs::symlink(tmp.path().join("outer/inner/src"), tmp.path().join("link"))
        .unwrap();
    // Physically this is inside `outer/inner`, but `$PWD` says it is under `link`, which has no
    // project above it.
    let cwd = tmp.path().join("link/deep");
    assert_printed(&up(&cwd, &tmp.path().join("home")), &cwd);
}

#[test]
fn falls_back_to_getcwd_without_pwd() {
    let tmp = projects();
    let cwd = tmp.path().join("outer/inner/src/deep");
    let run = run(command(UP).current_dir(&cwd).env_remove("PWD").env("HOME", tmp.path()));
    assert_printed(&run, Path::new(&canonical(&cwd)).parent().unwrap().parent().unwrap());
}
