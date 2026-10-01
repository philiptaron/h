//! Tests for `h-shell-init`, including running its output in real shells.

mod common;

use std::path::Path;
use std::process::Command;

use common::*;

const H: &str = env!("CARGO_BIN_EXE_h");
const H_SHELL_INIT: &str = env!("CARGO_BIN_EXE_h-shell-init");
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A bash with the binaries under test available as variables.
fn bash(root: &Path, script: &str) -> Command {
    let mut cmd = common::bash(root, script);
    cmd.env("H", H).env("H_SHELL_INIT", H_SHELL_INIT);
    cmd
}

/// A zsh with the binaries under test available as variables.
fn zsh(root: &Path, script: &str) -> Command {
    let mut cmd = common::zsh(root, script);
    cmd.env("H", H).env("H_SHELL_INIT", H_SHELL_INIT);
    cmd
}

#[test]
fn h_init_help() {
    for flag in ["-h", "--help"] {
        let out = run(command(H_SHELL_INIT).arg(flag));
        assert_eq!(out.code, Some(0));
        assert_eq!(
            out.stdout,
            format!(
                "h-shell-init {VERSION}\nUsage: eval \"$(h-shell-init [--pushd] [--name NAME] \
                 [--store DIR] [--git-opts \"OPTIONS\"] [code-root])\"\n"
            )
        );
    }
}

#[test]
fn h_init_version() {
    for flag in ["-V", "--version"] {
        let out = run(command(H_SHELL_INIT).arg(flag));
        assert_eq!((out.code, out.stdout), (Some(0), format!("h-shell-init {VERSION}\n")));
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
    let h = canonical(Path::new(H));
    assert!(out.stdout.contains(&format!("command {h} --root \"/code\" go \"$@\"")), "{out:?}");
    // Not run from bash or zsh, so no completion.
    assert!(!out.stdout.contains("complete"), "{out:?}");
}

#[test]
fn h_init_from_path_calls_h_by_absolute_path() {
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = Path::new(H_SHELL_INIT).parent().unwrap();
    let path = std::env::join_paths([bin_dir.to_path_buf()]).unwrap();
    let out = run(command("h-shell-init").current_dir(tmp.path()).env("PATH", path).arg("/code"));
    let h = canonical(Path::new(H));
    assert!(out.stdout.contains(&format!("command {h} --root")), "{out:?}");
}

#[test]
fn h_init_code_root_defaults() {
    let out = run(command(H_SHELL_INIT).env("HOME", "/home/test"));
    assert!(out.stdout.contains("--root \"/home/test/src\" go"), "{out:?}");
    assert!(!out.stdout.contains("--store"), "{out:?}");

    let out = run(command(H_SHELL_INIT).env("HOME", "/home/test").env("H_CODE_ROOT", "~/code"));
    assert!(out.stdout.contains("--root \"/home/test/code\" go"), "{out:?}");

    let out = run(command(H_SHELL_INIT).env("H_CODE_ROOT", "/env").arg("/arg"));
    assert!(out.stdout.contains("--root \"/arg\" go"), "{out:?}");

    let out =
        run(command(H_SHELL_INIT).env("HOME", "/home/test").args(["--store", "~/store", "/arg"]));
    assert!(out.stdout.contains("--root \"/arg\" --store \"/home/test/store\" go"), "{out:?}");
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
    // On failure h prints the physical current directory (getcwd), so the shell ends up there.
    let physical = canonical(&project);
    assert_eq!(out.stdout, format!("ret=0\n{}\nret=1\n{physical}\n", project.display()));
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
        r#"eval "$("$H_SHELL_INIT" --git-opts "-c user.name=\"Me Too\"" "$ROOT")"
           h https://example.com/new/repo.git --branch dev; echo "ret=$?"; pwd"#,
    );
    git.apply(&mut cmd);
    let out = run(cmd.env("FAKE_GIT_MKDIR", "1"));

    let path = tmp.path().join("example.com/new/repo");
    assert_eq!(out.stdout, format!("ret=0\n{}\n", path.display()), "{out:?}");
    assert_eq!(
        git.args().unwrap(),
        [
            "-c",
            "user.name=Me Too",
            "clone",
            "--recursive",
            "-c",
            "user.name=Me Too",
            "--branch",
            "dev",
            "--",
            "https://example.com/new/repo.git",
            path.to_str().unwrap()
        ]
    );
}

#[test]
fn bash_h_store_runs_in_place() {
    let tmp = code_tree();
    let store = tmp.path().join("store");
    let mut cmd = bash(
        tmp.path(),
        r#"eval "$("$H_SHELL_INIT" --store "$ROOT/store" "$ROOT")"
           cd github.com
           h store init >/dev/null; echo "ret=$?"
           h store path
           h store list; echo "ret=$?"
           pwd"#,
    );
    isolate_git(&mut cmd, tmp.path());
    let out = run(cmd);
    assert_eq!(out.stderr, "", "{out:?}");
    assert_eq!(
        out.stdout,
        format!("ret=0\n{}\nret=0\n{}\n", store.display(), tmp.path().join("github.com").display())
    );
    assert!(store.join("HEAD").is_file());
}

#[test]
fn bash_h_store_jumps_to_a_project_named_store() {
    let tmp = code_tree();
    mkdirs(tmp.path(), &["github.com/owner/store"]);
    let out = run(bash(
        tmp.path(),
        r#"eval "$("$H_SHELL_INIT" --store "$ROOT/the-store" "$ROOT")"
           h store; echo "ret=$?"
           pwd"#,
    ));
    assert_eq!(out.stderr, "", "{out:?}");
    assert_eq!(
        out.stdout,
        format!("ret=0\n{}\n", tmp.path().join("github.com/owner/store").display())
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
