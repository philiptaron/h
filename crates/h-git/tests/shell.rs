//! Tests for `h-shell-init` and `up-shell-init`, including running their output in real shells.

mod common;

use std::path::Path;
use std::process::Command;

use common::*;

#[test]
fn h_init_help() {
    for flag in ["-h", "--help"] {
        let out = run(command(H_SHELL_INIT).arg(flag));
        assert_eq!(out.code, Some(0));
        assert_eq!(
            out.stdout,
            "Usage: eval \"$(h-shell-init [--pushd] [--name NAME] [--git-opts \"OPTIONS\"] \
             [code-root])\"\n"
        );
    }
}

#[test]
fn h_init_rejects_unknown_options() {
    let out = run(command(H_SHELL_INIT).arg("--bogus"));
    assert_eq!(out.code, Some(1));
    assert_eq!(out.stdout, "");
    assert_eq!(out.stderr, "Unknown option: --bogus\n");
}

#[test]
fn h_init_points_at_sibling_h() {
    let out = run(command(H_SHELL_INIT).arg("/code"));
    assert_eq!(out.code, Some(0));
    assert!(out.stdout.contains(&format!("command {H} --resolve \"/code\" \"$@\"")), "{out:?}");
    // Not run from bash or zsh, so no completion.
    assert!(!out.stdout.contains("complete"), "{out:?}");
}

#[test]
fn h_init_code_root_defaults() {
    let out = run(command(H_SHELL_INIT).env("HOME", "/home/test"));
    assert!(out.stdout.contains("--resolve \"/home/test/src\""), "{out:?}");

    let out = run(command(H_SHELL_INIT).env("HOME", "/home/test").env("H_CODE_ROOT", "~/code"));
    assert!(out.stdout.contains("--resolve \"/home/test/code\""), "{out:?}");

    let out = run(command(H_SHELL_INIT).env("H_CODE_ROOT", "/env").arg("/arg"));
    assert!(out.stdout.contains("--resolve \"/arg\""), "{out:?}");
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

/// A shell command running `script`, with the binaries and `root` available as variables.
fn shell(program: &str, args: &[&str], root: &Path, script: &str) -> Command {
    let mut cmd = command(program);
    cmd.args(args)
        .arg(script)
        .current_dir(root)
        .env("HOME", root)
        .env("ROOT", root)
        .env("H_SHELL_INIT", H_SHELL_INIT)
        .env("UP_SHELL_INIT", UP_SHELL_INIT);
    cmd
}

fn bash(root: &Path, script: &str) -> Command {
    shell("bash", &["--norc", "--noprofile", "-c"], root, script)
}

fn zsh(root: &Path, script: &str) -> Command {
    shell("zsh", &["-f", "-c"], root, script)
}

fn code_tree() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(
        tmp.path(),
        &["github.com/owner/project", "github.com/owner/other", "gitlab.com/group/deep"],
    );
    tmp
}

#[test]
fn bash_h_changes_directory() {
    let tmp = code_tree();
    let out = run(bash(
        tmp.path(),
        r#"eval "$("$H_SHELL_INIT" "$ROOT")"
           h project; echo "ret=$?"; pwd
           h nope; echo "ret=$?"; pwd"#,
    ));
    let project = tmp.path().join("github.com/owner/project");
    assert_eq!(out.stdout, format!("ret=0\n{}\nret=1\n{}\n", project.display(), project.display()));
    assert_eq!(out.stderr, "nope not found\n");
}

#[test]
fn bash_h_with_custom_name_and_pushd() {
    let tmp = code_tree();
    let out = run(bash(
        tmp.path(),
        r#"eval "$("$H_SHELL_INIT" --pushd --name j "$ROOT")"
           j project >/dev/null; pwd; dirs -p | wc -l"#,
    ));
    let project = tmp.path().join("github.com/owner/project");
    assert_eq!(out.stdout, format!("{}\n2\n", project.display()), "{out:?}");
}

#[test]
fn bash_h_passes_git_opts() {
    let tmp = code_tree();
    let git = FakeGit::install(tmp.path());
    let mut cmd = bash(
        tmp.path(),
        r#"eval "$("$H_SHELL_INIT" --git-opts "--depth 1" "$ROOT")"
           h https://example.com/new/repo.git --branch dev; echo "ret=$?"; pwd"#,
    );
    git.apply(&mut cmd);
    let out = run(cmd.env("FAKE_GIT_MKDIR", "1"));

    let path = tmp.path().join("example.com/new/repo");
    assert_eq!(out.stdout, format!("ret=0\n{}\n", path.display()), "{out:?}");
    assert_eq!(
        git.args().unwrap(),
        [
            "clone",
            "--depth",
            "1",
            "--branch",
            "dev",
            "--",
            "https://example.com/new/repo.git",
            path.to_str().unwrap()
        ]
    );
}

#[test]
fn bash_completion() {
    let tmp = code_tree();
    if !run(bash(tmp.path(), "type compgen")).code.is_some_and(|c| c == 0) {
        eprintln!("bash lacks programmable completion; skipping");
        return;
    }
    mkdirs(tmp.path(), &["github.com/owner/.hidden"]);
    let out = run(bash(
        tmp.path(),
        r#"eval "$("$H_SHELL_INIT" "$ROOT")"
           complete -p h
           COMP_WORDS=(h o); COMP_CWORD=1; _h_complete; printf '%s\n' "${COMPREPLY[@]}"
           COMP_WORDS=(h .); COMP_CWORD=1; _h_complete; echo "hidden=${#COMPREPLY[@]}""#,
    ));
    assert_eq!(out.stderr, "");
    assert_eq!(out.stdout, "complete -F _h_complete h\nother\nowner\nhidden=0\n");
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
fn zsh_h_and_completion() {
    if !have("zsh") {
        eprintln!("zsh not found; skipping");
        return;
    }
    let tmp = code_tree();
    let out = run(zsh(
        tmp.path(),
        r#"autoload -Uz compinit && compinit -u -D
           eval "$("$H_SHELL_INIT" "$ROOT")"
           echo "completer=$_comps[h]"
           h deep; echo "ret=$?"; pwd"#,
    ));
    assert_eq!(out.stderr, "");
    assert_eq!(
        out.stdout,
        format!(
            "completer=_h_complete\nret=0\n{}\n",
            tmp.path().join("gitlab.com/group/deep").display()
        )
    );
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
