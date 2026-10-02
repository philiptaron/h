//! End-to-end tests for `h store ingest`, which puts a checkout under the store, driven by real
//! git against local repositories published at `https://example.com/o/<name>.git`.

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
}

impl Sandbox {
    fn new() -> Sandbox {
        let tmp = tempfile::tempdir().unwrap();
        let sb = Sandbox { store: tmp.path().join("store"), tmp };
        sb.git_global(&["protocol.file.allow", "always"]);
        for name in ["deep", "lib", "other", "app"] {
            sb.publish(name);
        }
        sb.add_submodule("lib", "deep", "deep");
        sb.add_submodule("app", "lib", "lib");
        sb.add_submodule("app", "other", "other");
        sb
    }

    fn home(&self) -> &Path {
        self.tmp.path()
    }

    fn git_global(&self, args: &[&str]) {
        let config = global_gitconfig(self.home());
        let out = run(git_command(self.home()).args(["config", "--file"]).arg(&config).args(args));
        assert_eq!(out.code, Some(0), "{out:?}");
    }

    fn src(&self, name: &str) -> PathBuf {
        self.home().join("src").join(name)
    }

    /// Create the repository `name`, with a README saying its name and a tag, at [`url`].
    fn publish(&self, name: &str) {
        let src = self.src(name);
        make_git_repo(&src);
        fs::write(src.join("README"), format!("{name}\n")).unwrap();
        self.git(&src, &["commit", "-qam", name]);
        self.git(&src, &["tag", &format!("{name}-1.0")]);
        rewrite_url(self.home(), &url(name), &src);
    }

    fn add_submodule(&self, repo: &str, name: &str, path: &str) {
        let src = self.src(repo);
        self.git(&src, &["submodule", "add", "-q", &url(name), path]);
        self.git(&src, &["commit", "-qm", &format!("add {path}")]);
    }

    /// `git -C <dir> <args>`, which must succeed; its stdout, trimmed.
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = run(git_command(self.home()).arg("-C").arg(dir).args(args));
        assert_eq!(out.code, Some(0), "git {args:?} in {}: {out:?}", dir.display());
        out.stdout.trim().to_string()
    }

    fn succeeds(&self, dir: &Path, args: &[&str]) -> bool {
        run(git_command(self.home()).arg("-C").arg(dir).args(args)).code == Some(0)
    }

    /// `h --store <store> <args>`, with git isolated.
    fn h(&self, args: &[&str]) -> Run {
        let mut cmd = command(H);
        isolate_git(&mut cmd, self.home());
        run(cmd.current_dir(self.home()).arg("--store").arg(&self.store).args(args))
    }

    fn ok(&self, args: &[&str]) -> Run {
        let out = self.h(args);
        assert_eq!(out.code, Some(0), "h {args:?}: {out:?}");
        out
    }

    /// A clone of `name` made by git alone, with no store, recursively, at `code/<name>`.
    fn clone_without_store(&self, name: &str) -> PathBuf {
        let path = self.home().join("code").join(name);
        let out = run(git_command(self.home())
            .args(["clone", "-q", "--recursive", &url(name)])
            .arg(&path));
        assert_eq!(out.code, Some(0), "{out:?}");
        path
    }

    /// The objects the repository at `dir` keeps itself, loose and packed.
    fn local_objects(&self, dir: &Path) -> usize {
        let counts = self.git(dir, &["count-objects", "-v"]);
        counts
            .lines()
            .filter_map(|line| line.split_once(": "))
            .filter(|(key, _)| matches!(*key, "count" | "in-pack"))
            .map(|(_, n)| n.parse::<usize>().unwrap())
            .sum()
    }

    fn alternates(&self, git_dir: &Path) -> String {
        fs::read_to_string(git_dir.join("objects/info/alternates")).unwrap_or_default()
    }

    fn store_objects(&self) -> String {
        format!("{}\n", self.store.join("objects").display())
    }
}

#[test]
fn an_existing_clone_is_put_under_the_store() {
    let sb = Sandbox::new();
    let clone = sb.clone_without_store("other");
    // Work of the user's own: a branch the store must never see, and a commit nothing names.
    sb.git(&clone, &["switch", "-qc", "mine"]);
    sb.git(&clone, &["commit", "-q", "--allow-empty", "-m", "mine"]);
    let mine = sb.git(&clone, &["rev-parse", "HEAD"]);
    let dangling = sb.git(&clone, &["commit-tree", "HEAD^{tree}", "-m", "dangling"]);
    // Packed, as it would be after any repack, where a plain `repack -a -d` would drop it.
    sb.git(&clone, &["repack", "-a", "-d", "-k", "-q"]);
    assert!(sb.git(&clone, &["count-objects", "-v"]).contains("count: 0\n"));
    let before = sb.local_objects(&clone);

    let out = sb.ok(&["store", "ingest", clone.to_str().unwrap()]);
    assert!(out.stderr.contains("added example.com/o/other\n"), "{out:?}");
    assert_eq!(sb.ok(&["store", "list"]).stdout, "example.com/o/other\n");
    for rev in ["example.com/o/other/main", "example.com/o/other/other-1.0"] {
        assert!(sb.succeeds(&sb.store, &["rev-parse", "--verify", "--quiet", rev]), "{rev}");
    }
    assert!(!sb.succeeds(&sb.store, &["cat-file", "-e", &mine]), "local branches stay local");

    assert_eq!(sb.alternates(&clone.join(".git")), sb.store_objects());
    let prefixes = sb.git(&clone, &["config", "core.alternateRefsPrefixes"]);
    assert_eq!(prefixes, "refs/remotes/example.com/o/other/ refs/tags/example.com/o/other/");
    let after = sb.local_objects(&clone);
    assert!(after < before, "the store's objects are dropped: {before} -> {after}");
    for object in [&mine, &dangling] {
        assert!(sb.succeeds(&clone, &["cat-file", "-e", object]), "{object} is kept");
    }
    assert!(sb.succeeds(&clone, &["fsck", "--connectivity-only", "--no-dangling"]));
    assert_eq!(sb.git(&clone, &["status", "--porcelain"]), "");

    // Ingesting again changes nothing.
    sb.ok(&["store", "ingest", clone.to_str().unwrap()]);
    assert_eq!(sb.alternates(&clone.join(".git")), sb.store_objects());
}

#[test]
fn history_comes_from_the_checkout_before_the_network() {
    let sb = Sandbox::new();
    let clone = sb.clone_without_store("other");
    // The upstream is out of reach, but the checkout has its history.
    fs::rename(sb.src("other"), sb.home().join("gone")).unwrap();

    let out = sb.h(&["store", "ingest", clone.to_str().unwrap()]);
    assert_eq!(out.code, Some(1), "fetching from the URL fails: {out:?}");
    assert!(out.stderr.contains("added example.com/o/other\n"), "{out:?}");
    let main = sb.git(&clone, &["rev-parse", "origin/main"]);
    let stored = sb.git(&sb.store, &["rev-parse", "example.com/o/other/main"]);
    assert_eq!(stored, main);
    assert!(sb.succeeds(&sb.store, &["rev-parse", "--verify", "example.com/o/other/other-1.0"]));
    assert_eq!(sb.alternates(&clone.join(".git")), sb.store_objects());
}

#[test]
fn submodules_are_put_under_the_store_at_every_level() {
    let sb = Sandbox::new();
    let app = sb.clone_without_store("app");
    sb.git(&app, &["config", "user.name", "Me"]);
    sb.ok(&["store", "ingest", app.to_str().unwrap()]);
    let mut listed: Vec<String> =
        sb.ok(&["store", "list"]).stdout.lines().map(String::from).collect();
    listed.sort();
    assert_eq!(listed, ["app", "deep", "lib", "other"].map(|name| format!("example.com/o/{name}")));
    let modules = app.join(".git/modules");
    for (git_dir, checkout) in [
        (modules.join("lib"), app.join("lib")),
        (modules.join("lib/modules/deep"), app.join("lib/deep")),
        (modules.join("other"), app.join("other")),
    ] {
        assert_eq!(sb.alternates(&git_dir), sb.store_objects(), "{git_dir:?}");
        assert_eq!(sb.local_objects(&checkout), 0, "{checkout:?}");
    }

    // A submodule a pull brings in is cloned from the store by the next ingest.
    sb.publish("extra");
    sb.add_submodule("app", "extra", "extra");
    sb.git(&app, &["pull", "-q", "--no-rebase"]);
    assert!(!app.join("extra/README").exists());
    let out = sb.ok(&["store", "ingest", app.to_str().unwrap()]);
    assert!(out.stderr.contains("added example.com/o/extra\n"), "{out:?}");
    assert_eq!(fs::read_to_string(app.join("extra/README")).unwrap(), "extra\n");
    assert_eq!(sb.alternates(&modules.join("extra")), sb.store_objects());
    assert_eq!(sb.local_objects(&app.join("extra")), 0);
    assert_eq!(sb.git(&app.join("extra"), &["config", "user.name"]), "Me");
}

#[test]
fn submodules_at_paths_with_spaces_are_cloned_from_the_store() {
    let sb = Sandbox::new();
    let app = sb.clone_without_store("app");
    sb.ok(&["store", "ingest", app.to_str().unwrap()]);
    sb.publish("extra");
    sb.add_submodule("app", "extra", "third party/extra");
    sb.git(&app, &["pull", "-q", "--no-rebase"]);

    let out = sb.ok(&["store", "ingest", app.to_str().unwrap()]);
    assert!(out.stderr.contains("added example.com/o/extra\n"), "{out:?}");
    let extra = app.join("third party/extra");
    assert_eq!(fs::read_to_string(extra.join("README")).unwrap(), "extra\n");
    // Its name is its path, spaces and all.
    assert_eq!(sb.alternates(&app.join(".git/modules/third party/extra")), sb.store_objects());
    assert_eq!(sb.local_objects(&extra), 0);
}

#[test]
fn a_fork_brings_its_upstream_into_the_store() {
    let sb = Sandbox::new();
    let fork = Fork::publish(sb.home(), &["dev"]);
    let clone = sb.home().join("code/me/proj");
    let out = run(git_command(sb.home()).args(["clone", "-q", Fork::FORK_URL]).arg(&clone));
    assert_eq!(out.code, Some(0), "{out:?}");
    sb.git(&clone, &["remote", "add", "upstream", Fork::UPSTREAM_URL]);
    sb.git(&clone, &["fetch", "-q", "upstream"]);

    sb.ok(&["store", "ingest", clone.to_str().unwrap()]);
    assert_eq!(sb.ok(&["store", "list"]).stdout, "github.com/me/proj\ngithub.com/up/proj\n");
    let upstream_dev = sb.git(&fork.upstream, &["rev-parse", "dev"]);
    assert_eq!(sb.git(&sb.store, &["rev-parse", "github.com/up/proj/dev"]), upstream_dev);
    let prefixes = sb.git(&clone, &["config", "core.alternateRefsPrefixes"]);
    assert_eq!(
        prefixes,
        "refs/remotes/github.com/me/proj/ refs/tags/github.com/me/proj/ \
         refs/remotes/github.com/up/proj/ refs/tags/github.com/up/proj/"
    );
    assert_eq!(sb.local_objects(&clone), 0, "the fork's own commit is in the store too");
}

#[test]
fn remotes_the_store_cannot_fetch_from_are_named() {
    let sb = Sandbox::new();
    let clone = sb.clone_without_store("other");
    // An SSH host alias, which only ssh's own configuration says is github.com.
    sb.git(&clone, &["remote", "add", "upstream", "me.github.com:up/other"]);

    let out = sb.h(&["store", "ingest", clone.to_str().unwrap()]);
    assert_eq!(out.code, Some(1), "{out:?}");
    let git_dir = fs::canonicalize(clone.join(".git")).unwrap();
    let skipped = format!(
        "could not add remote upstream of {}: the store cannot fetch from me.github.com:up/other\n",
        git_dir.display()
    );
    assert!(out.stderr.contains(&skipped), "{out:?}");
    // Everything else is done all the same.
    assert_eq!(sb.ok(&["store", "list"]).stdout, "example.com/o/other\n");
    assert_eq!(sb.alternates(&clone.join(".git")), sb.store_objects());

    // A submodule's remote is named too.
    let app = sb.clone_without_store("app");
    let lib = app.join("lib");
    sb.git(&lib, &["remote", "set-url", "origin", "/elsewhere/lib"]);
    let out = sb.h(&["store", "ingest", app.to_str().unwrap()]);
    assert_eq!(out.code, Some(1), "{out:?}");
    let git_dir = fs::canonicalize(app.join(".git/modules/lib")).unwrap();
    let skipped = format!(
        "could not add remote origin of {}: the store cannot fetch from /elsewhere/lib\n",
        git_dir.display()
    );
    assert!(out.stderr.contains(&skipped), "{out:?}");
}

#[test]
fn ingest_needs_a_checkout() {
    let sb = Sandbox::new();
    let out = sb.h(&["store", "ingest", sb.home().join("nowhere").to_str().unwrap()]);
    assert_eq!(out.code, Some(1), "{out:?}");
    let out = sb.h(&["store", "ingest", "a", "b"]);
    assert_eq!(out.stderr, "Usage: h store ingest [DIR]\n");
}
