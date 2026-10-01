//! End-to-end tests that the git h runs ignores the repository h itself was started in.
//!
//! Git exports `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE` and friends to hooks and to the
//! commands of `git rebase --exec` and `git bisect run`, so h may well be run with them set.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;

const H: &str = env!("CARGO_BIN_EXE_h");

const PROJ_URL: &str = "https://example.com/owner/proj.git";
const PROJ: &str = "example.com/owner/proj";

/// A source repository published at [`PROJ_URL`], and an unrelated repository, the victim.
struct Sandbox {
    tmp: tempfile::TempDir,
    src: PathBuf,
    victim: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src/proj");
        make_git_repo(&src);
        rewrite_url(tmp.path(), PROJ_URL, &src);
        let victim = tmp.path().join("victim");
        make_git_repo(&victim);
        Sandbox { tmp, src, victim }
    }

    fn root(&self) -> &Path {
        self.tmp.path()
    }

    /// An `h` command with git isolated and the victim's environment set as git sets it for hooks.
    fn command(&self) -> Command {
        let mut cmd = command(H);
        isolate_git(&mut cmd, self.root());
        let git_dir = self.victim.join(".git");
        cmd.current_dir(&self.victim)
            .env("GIT_DIR", &git_dir)
            .env("GIT_WORK_TREE", &self.victim)
            .env("GIT_INDEX_FILE", git_dir.join("index"))
            .env("GIT_OBJECT_DIRECTORY", git_dir.join("objects"))
            .env("GIT_COMMON_DIR", &git_dir)
            .env("GIT_PREFIX", "");
        cmd
    }

    fn h(&self, args: &[&str]) -> Run {
        let out = run(self.command().args(args));
        assert_eq!(out.code, Some(0), "h {args:?}: {out:?}");
        out
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = run(git_command(self.root()).arg("-C").arg(dir).args(args));
        assert_eq!(out.code, Some(0), "git {args:?} in {dir:?}: {out:?}");
        out.stdout
    }

    /// Everything about the victim that a stray git command could change.
    fn victim_state(&self) -> String {
        let git_dir = self.victim.join(".git");
        let mut entries: Vec<String> = fs::read_dir(&self.victim)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        format!(
            "{entries:?}\n{}\n{:?}\n{}{}",
            fs::read_to_string(git_dir.join("config")).unwrap(),
            fs::read(git_dir.join("index")).unwrap(),
            self.git(&self.victim, &["for-each-ref", "--format=%(refname) %(objectname)"]),
            self.git(&self.victim, &["count-objects", "-v"]),
        )
    }

    fn head(&self, dir: &Path) -> String {
        self.git(dir, &["rev-parse", "HEAD"]).trim().to_string()
    }
}

#[test]
fn store_commands_ignore_the_repository_h_runs_in() {
    let sb = Sandbox::new();
    let before = sb.victim_state();
    let store = sb.root().join("store");
    let store_arg = store.to_str().unwrap();

    sb.h(&["--store", store_arg, "store", "add", PROJ_URL]);
    assert_eq!(sb.h(&["--store", store_arg, "store", "list"]).stdout, format!("{PROJ}\n"));
    let out = sb.h(&["--store", store_arg, "store", "show", "proj", "main:README"]);
    assert_eq!(out.stdout, "hello\n");
    let main = sb.git(&store, &["rev-parse", &format!("{PROJ}/main")]);
    assert_eq!(main.trim(), sb.head(&sb.src));
    assert_eq!(sb.git(&store, &["config", &format!("remote.{PROJ}.url")]).trim(), PROJ_URL);

    assert_eq!(sb.victim_state(), before, "the victim repository is untouched");
}

#[test]
fn clones_ignore_the_repository_h_runs_in() {
    let sb = Sandbox::new();
    let before = sb.victim_state();

    for (root, extra) in [("code", &[][..]), ("containers", &["--container"])] {
        let root = sb.root().join(root);
        let mut args = vec!["--root", root.to_str().unwrap(), "go", PROJ_URL];
        args.extend(extra);
        args.extend(["--", "-c", "user.name=Me"]);
        let out = sb.h(&args);
        let clone = root.join(PROJ);
        assert_eq!(out.stdout, format!("{}\n", clone.display()));
        assert_eq!(sb.git(&clone, &["rev-parse", "origin/main"]).trim(), sb.head(&sb.src));
        assert_eq!(sb.git(&clone, &["config", "user.name"]).trim(), "Me", "{extra:?}");
        if extra.is_empty() {
            assert_eq!(fs::read_to_string(clone.join("README")).unwrap(), "hello\n");
        }
    }

    assert_eq!(sb.victim_state(), before, "the victim repository is untouched");
}

#[test]
fn configuration_from_git_c_still_applies() {
    // Hooks of a `git -c ... commit` see the `-c` settings in the environment, in either of
    // git's two encodings, and h passes both on, as git does when it changes repository.
    let sb = Sandbox::new();
    let store = sb.root().join("store");
    let (one, two) = (sb.root().join("src/one"), sb.root().join("src/two"));
    make_git_repo(&one);
    make_git_repo(&two);
    // Without the rewrites these name missing local paths, so a failure never reaches a network.
    let one_url = "file:///h-test-missing/env/one.git";
    let two_url = "file:///h-test-missing/params/two.git";
    let two_param = format!("'url.file://{}.insteadof'='{two_url}'", two.display());
    let out = run(sb
        .command()
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", format!("url.file://{}.insteadOf", one.display()))
        .env("GIT_CONFIG_VALUE_0", one_url)
        .env("GIT_CONFIG_PARAMETERS", two_param)
        .arg("--store")
        .arg(&store)
        .args(["store", "add", one_url, two_url]));
    assert_eq!(out.code, Some(0), "{out:?}");
    for (name, src) in [("h-test-missing/env/one", &one), ("h-test-missing/params/two", &two)] {
        let main = sb.git(&store, &["rev-parse", &format!("{name}/main")]);
        assert_eq!(main.trim(), sb.head(src), "{name}");
    }
}
