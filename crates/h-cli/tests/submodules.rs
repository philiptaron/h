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
        self.h_api(store, UNREACHABLE_API, args)
    }

    /// [`Sandbox::h`], with the GitHub API at `api`.
    fn h_api(&self, store: bool, api: &str, args: &[&str]) -> Run {
        run(self.h_command(store, api).args(args))
    }

    /// The command [`Sandbox::h_api`] runs, before its arguments.
    fn h_command(&self, store: bool, api: &str) -> std::process::Command {
        let mut cmd = command(H);
        isolate_git(&mut cmd, self.tmp.path());
        cmd.current_dir(self.tmp.path()).env("H_GITHUB_API", api).arg("--root").arg(&self.root);
        if store {
            cmd.arg("--store").arg(&self.store);
        }
        cmd
    }

    /// Publish `src/<name>` on GitHub as `up/<name>`, and a fork of it as `me/<name>`, with a
    /// mock GitHub API that says so.
    fn publish_fork(&self, name: &str) -> MockGitHub {
        let fork = self.tmp.path().join("forks").join(format!("{name}.git"));
        let clone = run(git_command(self.tmp.path())
            .args(["clone", "-q", "--bare"])
            .arg(self.src(name))
            .arg(&fork));
        assert_eq!(clone.code, Some(0), "{clone:?}");
        rewrite_url(self.tmp.path(), &format!("https://github.com/up/{name}.git"), &self.src(name));
        rewrite_url(self.tmp.path(), &format!("https://github.com/me/{name}.git"), &fork);
        let fork_json = format!(
            r#"{{"name": "{name}", "owner": {{"login": "me"}}, "parent": {{"full_name": "up/{name}"}}}}"#
        );
        let up_json = format!(r#"{{"name": "{name}", "owner": {{"login": "up"}}}}"#);
        let (fork_path, up_path) = (format!("/repos/me/{name}"), format!("/repos/up/{name}"));
        MockGitHub::start(&[(&fork_path, 200, &fork_json), (&up_path, 200, &up_json)])
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

/// Point submodule `name` of `src/<repo>` at `url` and commit that.
fn repoint_submodule(sb: &Sandbox, repo: &str, name: &str, url: &str) {
    let src = sb.src(repo);
    let key = format!("submodule.{name}.url");
    sb.git(&src, &["config", "--file", ".gitmodules", &key, url]);
    sb.git(&src, &["commit", "-qam", &format!("move {name}")]);
}

#[test]
fn forks_keep_their_upstream_when_a_submodule_fails() {
    let sb = Sandbox::new();
    sb.publish("broken");
    sb.add_submodule("broken", "lib", "lib");
    repoint_submodule(&sb, "broken", "lib", &url("missing"));
    rewrite_url(sb.tmp.path(), &url("missing"), &sb.tmp.path().join("missing"));
    let api = sb.publish_fork("broken");
    let clone = sb.root.join("github.com/me/broken");

    for store in [false, true] {
        if store {
            sb.fill_store();
        }
        let out = sb.h_api(store, &api.url, &["go", "me/broken"]);
        assert_ne!(out.code, Some(0), "store {store}: {out:?}");
        assert!(clone.join("README").is_file(), "the superproject stays");
        assert_eq!(sb.config(&clone, "remote.upstream.url"), "https://github.com/up/broken.git");
        assert_eq!(sb.config(&clone, "remote.pushDefault"), "origin");
        fs::remove_dir_all(&sb.root).unwrap();
    }
}

/// `app3` on GitHub as `up/app3`, with a fork `me/app3`, whose submodule `lib3` is given as
/// `../lib3.git`: `up/lib3` beside the upstream, and `me/lib3` beside the fork only when
/// `fork_lib`. Without it, the fork's copy is a local path that does not exist, so asking for
/// it fails at once, without the network.
fn publish_relative(sb: &Sandbox, fork_lib: bool) -> MockGitHub {
    let home = sb.tmp.path();
    sb.publish("lib3");
    sb.publish("app3");
    sb.add_submodule("app3", "lib3", "lib3");
    repoint_submodule(sb, "app3", "lib3", "../lib3.git");
    rewrite_url(home, "https://github.com/up/lib3.git", &sb.src("lib3"));
    let fork = home.join("forks/lib3.git");
    if fork_lib {
        let clone =
            run(git_command(home).args(["clone", "-q", "--bare"]).arg(sb.src("lib3")).arg(&fork));
        assert_eq!(clone.code, Some(0), "{clone:?}");
    }
    rewrite_url(home, "https://github.com/me/lib3.git", &fork);
    sb.publish_fork("app3")
}

#[test]
fn forks_take_relative_submodules_from_the_fork_when_it_has_them() {
    let sb = Sandbox::new();
    let api = publish_relative(&sb, true);
    let out = sb.h_api(false, &api.url, &["go", "me/app3"]);
    assert_eq!(out.code, Some(0), "{out:?}");
    let app = sb.root.join("github.com/me/app3");
    assert_eq!(sb.config(&app, "submodule.lib3.url"), "https://github.com/me/lib3.git");
    assert_eq!(fs::read_to_string(app.join("lib3/README")).unwrap(), "lib3\n");
}

#[test]
fn forks_take_relative_submodules_from_the_upstream_when_the_fork_lacks_them() {
    let sb = Sandbox::new();
    let api = publish_relative(&sb, false);
    for store in [false, true] {
        if store {
            sb.fill_store();
        }
        let out = sb.h_api(store, &api.url, &["go", "me/app3"]);
        assert_eq!(out.code, Some(0), "store {store}: {out:?}");
        let app = sb.root.join("github.com/me/app3");
        let upstream = "https://github.com/up/lib3.git";
        assert_eq!(sb.config(&app, "submodule.lib3.url"), upstream);
        assert_eq!(sb.config(&app.join("lib3"), "remote.origin.url"), upstream);
        assert_eq!(fs::read_to_string(app.join("lib3/README")).unwrap(), "lib3\n");
        assert!(out.stderr.contains(&format!("comes from {upstream}")), "{out:?}");
        fs::remove_dir_all(&sb.root).unwrap();
    }
}

#[test]
fn a_fork_copy_in_the_store_is_taken_without_asking_for_it() {
    let sb = Sandbox::new();
    let api = publish_relative(&sb, true);
    sb.ok(true, &["store", "add", "https://github.com/me/lib3.git"]);
    let trace = sb.tmp.path().join("trace");
    let out = run(sb.h_command(true, &api.url).env("GIT_TRACE", &trace).args(["go", "me/app3"]));
    assert_eq!(out.code, Some(0), "{out:?}");
    let app = sb.root.join("github.com/me/app3");
    assert_eq!(sb.config(&app, "submodule.lib3.url"), "https://github.com/me/lib3.git");
    let traced = fs::read_to_string(&trace).unwrap();
    assert!(!traced.lines().any(|line| line.contains("ls-remote")), "{traced}");
}

#[test]
fn clones_go_into_the_store_and_are_downloaded_once() {
    use std::os::unix::fs::PermissionsExt;
    let sb = Sandbox::new();
    let init = sb.h(true, &["store", "init"]);
    assert_eq!(init.code, Some(0), "{init:?}");
    // Every pack any repository serves is written down with where it was served from.
    let (hook, served) = (sb.tmp.path().join("pack-hook"), sb.tmp.path().join("served"));
    fs::write(&hook, format!("#!/bin/sh\npwd >> '{}'\nexec \"$@\"\n", served.display())).unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    set_global(&sb, "uploadpack.packObjectsHook", hook.to_str().unwrap());
    let out = sb.h(true, &["go", &url("app")]);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(out.stdout, format!("{}\n", sb.app().display()));

    // Everything is in the store, and the clone and its submodules keep none of it themselves.
    let mut listed: Vec<String> =
        sb.ok(true, &["store", "list"]).stdout.lines().map(String::from).collect();
    listed.sort();
    assert_eq!(listed, ["app", "deep", "lib", "other"].map(|n| format!("example.com/o/{n}")));
    let app = sb.app();
    for dir in [app.clone(), app.join("lib"), app.join("lib/deep"), app.join("other")] {
        assert!(sb.borrows_everything(&dir), "{dir:?}");
    }
    // Each published repository serves one pack: app's goes into the store before the clone,
    // which then borrows it all, and each submodule's into its clone, from which the store takes
    // it, so fetching the submodules into the store from their URLs brings nothing more.
    let served = fs::read_to_string(&served).unwrap();
    let src = sb.tmp.path().join("src");
    let mut published: Vec<&str> = served.lines().filter(|dir| dir.contains("/src/")).collect();
    published.sort();
    let want: Vec<String> = ["app", "deep", "lib", "other"]
        .map(|name| src.join(name).join(".git").canonicalize().unwrap().display().to_string())
        .into();
    assert_eq!(published, want, "{served}");
}

#[test]
fn shallow_clones_stay_out_of_the_store() {
    let sb = Sandbox::new();
    sb.ok(true, &["store", "init"]);
    // A shallow clone, asked for among the clone options or among the git options, as the
    // shell function's `--git-opts` passes them.
    for args in [&["--depth", "1"][..], &["--", "-c", "user.name=Me", "--depth", "1"]] {
        let mut go = vec!["go".to_string(), url("app")];
        go.extend(args.iter().map(|arg| arg.to_string()));
        let go: Vec<&str> = go.iter().map(String::as_str).collect();
        sb.ok(true, &go);
        let app = sb.app();
        assert_eq!(sb.git(&app, &["rev-parse", "--is-shallow-repository"]).trim(), "true");
        assert_eq!(sb.ok(true, &["store", "list"]).stdout, "", "{args:?}");
        fs::remove_dir_all(&sb.root).unwrap();
    }
}

#[test]
fn worktrees_made_by_the_hook_get_their_submodules_from_the_store() {
    use std::io::Write;
    let sb = Sandbox::new();
    sb.ok(true, &["store", "init"]);
    sb.ok(true, &["go", &url("app")]);
    let app = sb.app();
    let input = format!(r#"{{"name": "agent-x", "cwd": "{}"}}"#, app.display());
    let mut cmd = sb.h_command(true, UNREACHABLE_API);
    let mut child = cmd
        .args(["hook", "worktree-create"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let worktree = app.join(".claude/worktrees/agent-x");
    assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{}\n", worktree.display()));

    assert_eq!(fs::read_to_string(worktree.join("lib/deep/README")).unwrap(), "deep\n");
    assert_eq!(fs::read_to_string(worktree.join("other/README")).unwrap(), "other\n");
    let modules = app.join(".git/worktrees/agent-x/modules");
    assert_eq!(sb.alternates(&modules.join("lib")), store_objects(&sb));
    for dir in [worktree.join("lib"), worktree.join("lib/deep"), worktree.join("other")] {
        assert!(sb.borrows_everything(&dir), "{dir:?}");
    }
}

#[test]
fn submodules_get_the_identity_the_clone_is_made_with() {
    let sb = Sandbox::new();
    // A submodule behind HTTP authentication, whose clone asks the credential helpers, and a
    // helper that writes down what it is asked and has nothing to give.
    let server = MockGitHub::serve(|_, _| (401, String::new()));
    sb.publish("app2");
    sb.add_submodule("app2", "lib", "lib");
    sb.add_submodule("app2", "other", "private");
    repoint_submodule(&sb, "app2", "private", &format!("{}/private.git", server.url));
    let log = sb.tmp.path().join("helper.log");
    set_global(&sb, "credential.helper", &format!("!f() {{ cat >> '{}'; }}; f", log.display()));
    let identity = ["--", "-c", "credential.username=PhilipTaronQ", "-c", "user.name=Q"];

    for store in [false, true] {
        let _ = fs::remove_file(&log);
        let out = sb.h(store, &[&["go", &url("app2")][..], &identity].concat());
        assert_ne!(out.code, Some(0), "the private submodule cannot be cloned: {out:?}");
        let asked = fs::read_to_string(&log).unwrap_or_default();
        assert!(asked.contains("username=PhilipTaronQ\n"), "store {store}: {asked:?}");
        let lib = sb.root.join("example.com/o/app2/lib");
        assert_eq!(sb.config(&lib, "user.name"), "Q", "store {store}");
        assert_eq!(sb.config(&lib, "credential.username"), "PhilipTaronQ", "store {store}");
        fs::remove_dir_all(&sb.root).unwrap();
        sb.fill_store();
    }
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
    // History unrelated to app, all of it newer than the submodules' own commits. App is in the
    // store too, as every clone made with it puts it there, so the superproject's own clone
    // negotiates the same either way.
    publish_history(&sb, "big", 1000);
    sb.ok(true, &["store", "add", &url("big"), &url("app")]);
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
    // Lib's new commit was downloaded into the submodule, then went into the store with the rest
    // of lib, so the submodule keeps nothing itself.
    assert!(sb.borrows_everything(&sb.app().join("lib")));
    let newer = sb.git(&sb.src("lib"), &["rev-parse", "HEAD"]);
    assert_eq!(sb.git(&sb.store, &["rev-parse", "example.com/o/lib/main"]).trim(), newer.trim());

    // A negotiator the user chose is left alone. The clone above put lib's new commit in the
    // store, so lib moves on again first.
    set_global(&sb, "fetch.negotiationAlgorithm", "consecutive");
    sb.git(&sb.src("lib"), &["commit", "-q", "--allow-empty", "-m", "newer still"]);
    sb.git(&sb.src("app/lib"), &["pull", "-q"]);
    sb.git(&sb.src("app"), &["commit", "-qam", "lib moved on again"]);
    let consecutive = submodule_haves();
    assert!(consecutive >= 1000, "the newer, unrelated commits come first: {consecutive}");
}
