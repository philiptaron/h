//! End-to-end tests for cloning submodules, alone and with a store, driven by real git.
//!
//! Repositories get `https://example.com/o/<name>.git` URLs, rewritten to local paths in the
//! isolated global git configuration, which also allows submodules over `file://`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::*;

const H: &str = env!("CARGO_BIN_EXE_h");

fn url(name: &str) -> String {
    format!("https://example.com/o/{name}.git")
}

/// `app` with submodules `lib` and `other`, where `lib` has a submodule `deep` of its own.
struct Sandbox {
    tmp: tempfile::TempDir,
    store: PathBuf,
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        let tmp = tempfile::tempdir().unwrap();
        let config = global_gitconfig(tmp.path());
        let allow = git_command(tmp.path())
            .args(["config", "--file"])
            .arg(&config)
            .args(["protocol.file.allow", "always"])
            .status()
            .unwrap();
        assert!(allow.success());
        let sb = Sandbox { store: tmp.path().join("store"), root: tmp.path().join("code"), tmp };
        for name in ["deep", "lib", "other", "app"] {
            sb.publish(name);
        }
        sb.add_submodule("lib", "deep", "deep");
        sb.add_submodule("app", "lib", "lib");
        sb.add_submodule("app", "other", "other");
        sb
    }

    fn src(&self, name: &str) -> PathBuf {
        self.tmp.path().join("src").join(name)
    }

    /// Create the repository `name`, with a README saying its name, at [`url`].
    fn publish(&self, name: &str) {
        let src = self.src(name);
        make_git_repo(&src);
        fs::write(src.join("README"), format!("{name}\n")).unwrap();
        self.git(&src, &["commit", "-qam", name]);
        rewrite_url(self.tmp.path(), &url(name), &src);
    }

    fn add_submodule(&self, repo: &str, name: &str, path: &str) {
        let src = self.src(repo);
        self.git(&src, &["submodule", "add", "-q", &url(name), path]);
        self.git(&src, &["commit", "-qm", &format!("add {path}")]);
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = run(git_command(self.tmp.path()).arg("-C").arg(dir).args(args));
        assert_eq!(out.code, Some(0), "git {args:?}: {out:?}");
        out.stdout
    }

    /// `h --root <root> [--store <store>] <args>`, with git isolated.
    fn h(&self, store: bool, args: &[&str]) -> Run {
        let mut cmd = command(H);
        isolate_git(&mut cmd, self.tmp.path());
        cmd.current_dir(self.tmp.path()).arg("--root").arg(&self.root);
        if store {
            cmd.arg("--store").arg(&self.store);
        }
        run(cmd.args(args))
    }

    fn ok(&self, store: bool, args: &[&str]) -> Run {
        let out = self.h(store, args);
        assert_eq!(out.code, Some(0), "h {args:?}: {out:?}");
        out
    }

    /// A store holding every submodule's upstream.
    fn fill_store(&self) {
        self.ok(true, &["store", "add", &url("lib"), &url("deep"), &url("other")]);
    }

    fn app(&self) -> PathBuf {
        self.root.join("example.com/o/app")
    }

    fn config(&self, dir: &Path, key: &str) -> String {
        let out =
            run(git_command(self.tmp.path()).arg("-C").arg(dir).args(["config", "--get-all", key]));
        out.stdout.trim_end().to_string()
    }

    /// The alternates of the repository whose git directory is `git_dir`.
    fn alternates(&self, git_dir: &Path) -> String {
        fs::read_to_string(git_dir.join("objects/info/alternates")).unwrap_or_default()
    }

    /// Whether the checkout at `dir` keeps no objects of its own.
    fn borrows_everything(&self, dir: &Path) -> bool {
        let objects = self.git(dir, &["count-objects", "-v"]);
        objects.contains("count: 0\n") && objects.contains("in-pack: 0\n")
    }
}

fn store_objects(sb: &Sandbox) -> String {
    format!("{}\n", sb.store.join("objects").display())
}

#[test]
fn submodules_borrow_from_the_store() {
    let sb = Sandbox::new();
    sb.fill_store();
    let out = sb.ok(true, &["go", &url("app")]);
    assert_eq!(out.stdout, format!("{}\n", sb.app().display()));
    assert!(!out.stderr.contains("cannot add alternate"), "{out:?}");

    let app = sb.app();
    assert_eq!(fs::read_to_string(app.join("lib/deep/README")).unwrap(), "deep\n");
    assert_eq!(fs::read_to_string(app.join("other/README")).unwrap(), "other\n");
    let modules = app.join(".git/modules");
    for (git_dir, checkout) in [
        (modules.join("lib"), app.join("lib")),
        (modules.join("lib/modules/deep"), app.join("lib/deep")),
        (modules.join("other"), app.join("other")),
    ] {
        assert_eq!(sb.alternates(&git_dir), store_objects(&sb), "{git_dir:?}");
        assert!(sb.borrows_everything(&checkout), "{checkout:?}");
    }
    // What git clone --recurse-submodules records, minus its unusable superproject alternate.
    assert_eq!(sb.config(&app, "submodule.active"), ".");
    assert_eq!(sb.config(&app, "submodule.alternateLocation"), "");
    assert_eq!(sb.config(&app, "submodule.lib.url"), url("lib"));
}

#[test]
fn without_a_store_git_clones_the_submodules() {
    let sb = Sandbox::new();
    sb.ok(false, &["go", &url("app")]);
    let app = sb.app();
    assert_eq!(fs::read_to_string(app.join("lib/deep/README")).unwrap(), "deep\n");
    assert_eq!(sb.alternates(&app.join(".git/modules/lib")), "");
    assert_eq!(sb.config(&app, "submodule.active"), ".");
}

#[test]
fn pathspecs_and_options_choose_the_submodules() {
    let sb = Sandbox::new();
    sb.fill_store();
    sb.ok(true, &["go", &url("app"), "--recurse-submodules=lib"]);
    let app = sb.app();
    assert_eq!(fs::read_to_string(app.join("lib/deep/README")).unwrap(), "deep\n");
    assert!(!app.join("other/README").exists());
    assert_eq!(sb.config(&app, "submodule.active"), "lib");
    assert_eq!(sb.alternates(&app.join(".git/modules/lib")), store_objects(&sb));

    fs::remove_dir_all(&sb.root).unwrap();
    sb.ok(true, &["go", &url("app"), "--no-recurse-submodules"]);
    assert!(!app.join("lib/README").exists());
    assert!(!app.join(".git/modules").exists());
    assert_eq!(sb.config(&app, "submodule.active"), "");
    assert_eq!(sb.alternates(&app.join(".git")), store_objects(&sb));

    // Shallow submodules are shallow, and still borrow.
    sb.git(&sb.src("lib"), &["commit", "-q", "--allow-empty", "-m", "second"]);
    sb.git(&sb.src("app/lib"), &["pull", "-q"]);
    sb.git(&sb.src("app"), &["commit", "-qam", "lib moved on"]);
    sb.ok(true, &["store", "fetch", "-q"]);
    fs::remove_dir_all(&sb.root).unwrap();
    sb.ok(true, &["go", &url("app"), "--shallow-submodules"]);
    let lib = app.join("lib");
    assert_eq!(sb.git(&lib, &["rev-parse", "--is-shallow-repository"]).trim(), "true");
    assert_eq!(sb.alternates(&app.join(".git/modules/lib")), store_objects(&sb));
}

#[test]
fn own_references_leave_recursive_clones_to_git() {
    let sb = Sandbox::new();
    sb.fill_store();
    // A checkout made by git itself, whose `.git/modules` git finds submodules in.
    let other = sb.tmp.path().join("reference");
    let clone = run(git_command(sb.tmp.path())
        .args(["clone", "-q", "--recursive", &url("app")])
        .arg(&other));
    assert_eq!(clone.code, Some(0), "{clone:?}");

    let reference = other.to_str().unwrap();
    sb.ok(true, &["go", &url("app"), "--reference", reference]);
    let app = sb.app();
    let reference_objects =
        |p: &str| format!("{}\n", other.join(".git").join(p).join("objects").display());
    assert_eq!(sb.alternates(&app.join(".git")), reference_objects(""));
    assert_eq!(sb.alternates(&app.join(".git/modules/lib")), reference_objects("modules/lib"));
    assert_eq!(sb.config(&app, "submodule.alternateLocation"), "superproject");
}

#[test]
fn failing_submodules_fail_the_clone_as_git_does() {
    let sb = Sandbox::new();
    sb.fill_store();
    sb.publish("broken");
    sb.add_submodule("broken", "lib", "lib");
    // Point the submodule at a repository that does not exist.
    let src = sb.src("broken");
    sb.git(&src, &["config", "--file", ".gitmodules", "submodule.lib.url", &url("missing")]);
    sb.git(&src, &["commit", "-qam", "break lib"]);
    rewrite_url(sb.tmp.path(), &url("missing"), &sb.tmp.path().join("missing"));

    let without = sb.h(false, &["go", &url("broken")]);
    fs::remove_dir_all(&sb.root).unwrap();
    let with = sb.h(true, &["go", &url("broken")]);
    assert_ne!(with.code, Some(0), "{with:?}");
    assert_eq!(with.code, without.code, "{with:?}\n{without:?}");
    assert_eq!(with.stdout, format!("{}\n", canonical(sb.tmp.path())));
    // As with git clone, the superproject stays.
    assert!(sb.root.join("example.com/o/broken/README").is_file());
}

#[test]
fn empty_repositories_clone_with_a_store() {
    let sb = Sandbox::new();
    sb.fill_store();
    let empty = sb.src("empty");
    let init = git_command(sb.tmp.path()).args(["init", "-q"]).arg(&empty).status().unwrap();
    assert!(init.success());
    rewrite_url(sb.tmp.path(), &url("empty"), &empty);
    sb.ok(true, &["go", &url("empty")]);
    assert!(sb.root.join("example.com/o/empty/.git").is_dir());
}

/// Set `key` in the isolated global configuration of `sb`.
fn set_global(sb: &Sandbox, key: &str, value: &str) {
    let config = git_command(sb.tmp.path())
        .args(["config", "--file"])
        .arg(global_gitconfig(sb.tmp.path()))
        .args([key, value])
        .status()
        .unwrap();
    assert!(config.success());
}

#[test]
fn sticky_recursive_clones_are_honored() {
    let sb = Sandbox::new();
    sb.fill_store();
    set_global(&sb, "submodule.stickyRecursiveClone", "true");
    sb.ok(true, &["go", &url("app")]);
    assert_eq!(sb.config(&sb.app(), "submodule.recurse"), "true");
}

/// Publish `name` with `commits` commits of its own, all newer than anything else here.
fn publish_history(sb: &Sandbox, name: &str, commits: usize) {
    use std::io::Write;
    let src = sb.src(name);
    let init = git_command(sb.tmp.path()).args(["init", "-q", "-b", "main"]).arg(&src).status();
    assert!(init.unwrap().success());
    let mut stream = String::new();
    for i in 0..commits {
        let when = 4_000_000_000 + i * 60;
        stream.push_str(&format!(
            "commit refs/heads/main\ncommitter T <t@e> {when} +0000\ndata 1\nc\n\
             M 644 inline f\ndata {}\n{i}\n\n",
            i.to_string().len() + 1
        ));
    }
    let mut import = git_command(sb.tmp.path())
        .arg("-C")
        .arg(&src)
        .args(["fast-import", "--quiet"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    import.stdin.take().unwrap().write_all(stream.as_bytes()).unwrap();
    assert!(import.wait().unwrap().success());
    rewrite_url(sb.tmp.path(), &url(name), &src);
}

/// How many commits the clones made by `h go <args>` offer the server as ones they have.
fn haves(sb: &Sandbox, args: &[&str]) -> usize {
    let _ = fs::remove_dir_all(&sb.root);
    let trace = sb.tmp.path().join("packets");
    let _ = fs::remove_file(&trace);
    let mut cmd = command(H);
    isolate_git(&mut cmd, sb.tmp.path());
    let out = run(cmd
        .current_dir(sb.tmp.path())
        .env("GIT_TRACE_PACKET", &trace)
        .arg("--root")
        .arg(&sb.root)
        .arg("--store")
        .arg(&sb.store)
        .args(args));
    assert_eq!(out.code, Some(0), "{out:?}");
    let packets = fs::read_to_string(&trace).unwrap();
    packets.lines().filter(|line| line.contains("clone> have ")).count()
}

#[test]
fn submodule_clones_do_not_offer_the_whole_store() {
    let sb = Sandbox::new();
    sb.fill_store();
    // History unrelated to app, all of it newer than the submodules' own commits.
    publish_history(&sb, "big", 1000);
    sb.ok(true, &["store", "add", &url("big")]);
    // A submodule the store has all of is not negotiated at all; one that moved on since the
    // store last fetched it is.
    sb.git(&sb.src("lib"), &["commit", "-q", "--allow-empty", "-m", "newer"]);
    sb.git(&sb.src("app/lib"), &["pull", "-q"]);
    sb.git(&sb.src("app"), &["commit", "-qam", "lib moved on"]);

    let app = url("app");
    let submodule_haves = || {
        let superproject = haves(&sb, &["go", &app, "--no-recurse-submodules"]);
        haves(&sb, &["go", &app]) - superproject
    };
    let skipping = submodule_haves();
    assert!(skipping < 100, "{skipping}");
    assert!(sb.borrows_everything(&sb.app().join("lib/deep")));
    let lib = sb.git(&sb.app().join("lib"), &["count-objects", "-v"]);
    assert!(
        lib.contains("count: 0\n") && lib.contains("in-pack: 1\n"),
        "only the new commit: {lib}"
    );

    // A negotiator the user chose is left alone.
    set_global(&sb, "fetch.negotiationAlgorithm", "consecutive");
    let consecutive = submodule_haves();
    assert!(consecutive >= 1000, "the newer, unrelated commits come first: {consecutive}");
}
