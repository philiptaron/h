//! End-to-end tests for `h store`, driven by real git against local repositories.
//!
//! The store names upstreams by URL, so the tests give repositories `https://example.com/...`
//! URLs and rewrite them to local paths in the isolated global git configuration.

mod common;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;

const H: &str = env!("CARGO_BIN_EXE_h");

const PROJ_URL: &str = "https://example.com/owner/proj.git";
const PROJ: &str = "example.com/owner/proj";

/// A sandbox with a source repository published at [`PROJ_URL`] and an empty store path.
struct Sandbox {
    tmp: tempfile::TempDir,
    store: PathBuf,
    src: PathBuf,
}

impl Sandbox {
    fn new() -> Sandbox {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src/proj");
        make_git_repo(&src);
        rewrite_url(tmp.path(), PROJ_URL, &src);
        let store = tmp.path().join("store");
        Sandbox { tmp, store, src }
    }

    fn root(&self) -> &Path {
        self.tmp.path()
    }

    /// `h --root <tmp> --store <store> store <args>`, with git isolated.
    fn h(&self, args: &[&str]) -> Run {
        let mut cmd = command(H);
        isolate_git(&mut cmd, self.root());
        cmd.current_dir(self.root()).arg("--root").arg(self.root()).arg("--store").arg(&self.store);
        run(cmd.arg("store").args(args))
    }

    fn ok(&self, args: &[&str]) -> Run {
        let out = self.h(args);
        assert_eq!(out.code, Some(0), "h store {args:?}: {out:?}");
        out
    }

    fn git(&self, dir: &Path) -> Command {
        let mut cmd = git_command(self.root());
        cmd.arg("-C").arg(dir);
        cmd
    }

    fn store_config(&self, key: &str) -> String {
        run(self.git(&self.store).args(["config", "--get-all", key])).stdout.trim().to_string()
    }

    /// Commit a new file in the source repository.
    fn publish(&self, name: &str) {
        fs::write(self.src.join(name), "new\n").unwrap();
        for args in [&["add", name][..], &["commit", "-q", "-m", name]] {
            assert!(self.git(&self.src).args(args).status().unwrap().success());
        }
    }
}

#[test]
fn store_commands_need_a_store() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(command(H).current_dir(tmp.path()).env_remove("H_STORE").args(["store", "list"]));
    assert_eq!(out.code, Some(1));
    assert_eq!(out.stderr, "No store configured: pass --store DIR or set H_STORE\n");
}

#[test]
fn init_creates_a_bare_store_that_never_prunes() {
    let sb = Sandbox::new();
    let out = sb.h(&["init", "--", "-c", "credential.username=me"]);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(out.stdout, format!("{}\n", sb.store.display()));
    assert!(sb.store.join("HEAD").is_file());
    assert_eq!(sb.store_config("core.bare"), "true");
    assert_eq!(sb.store_config("gc.pruneExpire"), "never");
    assert_eq!(sb.store_config("gc.auto"), "0");
    assert_eq!(sb.store_config("maintenance.gc.enabled"), "false");
    assert_eq!(sb.store_config("maintenance.prefetch.enabled"), "false");
    assert_eq!(sb.store_config("credential.username"), "me");
    assert_eq!(sb.ok(&["path"]).stdout, format!("{}\n", sb.store.display()));
    assert_eq!(sb.ok(&["list"]).stdout, "");

    // Running init again is harmless.
    sb.ok(&["init"]);
    assert_eq!(sb.store_config("gc.pruneExpire"), "never");
}

#[test]
fn stores_hold_sha1_objects_whatever_the_default() {
    let sb = Sandbox::new();
    let mut cmd = command(H);
    isolate_git(&mut cmd, sb.root());
    let out = run(cmd
        .current_dir(sb.root())
        .env("GIT_DEFAULT_HASH", "sha256")
        .arg("--store")
        .arg(&sb.store)
        .args(["store", "add", PROJ_URL]));
    assert_eq!(out.code, Some(0), "{out:?}");
    let format = run(sb.git(&sb.store).args(["rev-parse", "--show-object-format"]));
    assert_eq!(format.stdout, "sha1\n");
    assert_eq!(sb.ok(&["show", "proj", "main:README"]).stdout, "hello\n");
}

#[test]
fn add_names_upstreams_by_path_and_namespaces_their_tags() {
    let sb = Sandbox::new();
    assert!(sb.git(&sb.src).args(["tag", "v1.0"]).status().unwrap().success());

    let out = sb.ok(&["add", PROJ_URL]);
    assert!(out.stderr.contains(&format!("added {PROJ}\n")), "{out:?}");
    assert_eq!(sb.ok(&["list"]).stdout, format!("{PROJ}\n"));
    assert_eq!(sb.store_config(&format!("remote.{PROJ}.url")), PROJ_URL);
    assert_eq!(sb.store_config(&format!("remote.{PROJ}.pushurl")), "no_push");
    assert_eq!(sb.store_config(&format!("remote.{PROJ}.tagOpt")), "--no-tags");
    assert_eq!(
        sb.store_config(&format!("remote.{PROJ}.fetch")),
        format!("+refs/heads/*:refs/remotes/{PROJ}/*\n+refs/tags/*:refs/tags/{PROJ}/*")
    );

    let refs = run(sb.git(&sb.store).args(["for-each-ref", "--format=%(refname)"])).stdout;
    assert_eq!(
        refs,
        format!("refs/remotes/{PROJ}/HEAD\nrefs/remotes/{PROJ}/main\nrefs/tags/{PROJ}/v1.0\n")
    );
    // `<name>/<branch>`, `<name>/<tag>` and `<name>` itself all resolve.
    for rev in [format!("{PROJ}/main"), format!("{PROJ}/v1.0"), PROJ.to_string()] {
        let out = run(sb.git(&sb.store).args(["rev-parse", "--verify", &rev]));
        assert_eq!(out.code, Some(0), "{rev}: {out:?}");
    }

    // Adding it again changes nothing.
    let out = sb.ok(&["add", PROJ_URL]);
    assert!(out.stderr.contains("already in the store"), "{out:?}");
    assert_eq!(sb.ok(&["list"]).stdout, format!("{PROJ}\n"));

    // Pushing to an upstream fails.
    let push = run(sb.git(&sb.store).args(["push", PROJ, &format!("{PROJ}/main:refs/heads/x")]));
    assert_ne!(push.code, Some(0));
    assert!(push.stderr.contains("no_push"), "{push:?}");
}

#[test]
fn fetch_keeps_every_upstreams_tags_when_tags_are_pruned_globally() {
    let sb = Sandbox::new();
    assert!(sb.git(&sb.src).args(["tag", "v1.0"]).status().unwrap().success());
    let other = sb.tmp.path().join("src/other");
    make_git_repo(&other);
    assert!(sb.git(&other).args(["tag", "v2.0"]).status().unwrap().success());
    let other_url = "https://example.org/x/other.git";
    rewrite_url(sb.root(), other_url, &other);
    sb.ok(&["add", PROJ_URL, other_url]);

    let git = |args: &[&str]| {
        let out = run(sb.git(&sb.store).args(args));
        assert_eq!(out.code, Some(0), "git {args:?}: {out:?}");
        out.stdout
    };
    // A global `fetch.pruneTags` would make each upstream's fetch delete the other's tags.
    let global = global_gitconfig(sb.root());
    git(&["config", "--file", global.to_str().unwrap(), "fetch.pruneTags", "true"]);
    let tags = || git(&["for-each-ref", "--format=%(refname)", "refs/tags"]);
    let want = format!("refs/tags/{PROJ}/v1.0\nrefs/tags/example.org/x/other/v2.0\n");

    // A store made before the store configuration said so is safe through `h store fetch`.
    git(&["config", "--unset", "fetch.pruneTags"]);
    sb.ok(&["fetch", "-q"]);
    assert_eq!(tags(), want);

    // Once `h store init` has run again, so is git's own fetch.
    sb.ok(&["init"]);
    assert_eq!(git(&["config", "--local", "fetch.pruneTags"]), "false\n");
    git(&["fetch", "-q", "--prune", "--all"]);
    assert_eq!(tags(), want);
}

#[test]
fn adding_an_unrelated_upstream_offers_few_commits() {
    let sb = Sandbox::new();
    // An upstream with a long history, which the source repository shares nothing with.
    let long = sb.tmp.path().join("src/long");
    assert!(git_command(sb.root()).args(["init", "-q"]).arg(&long).status().unwrap().success());
    let mut stream = String::new();
    for i in 0..500 {
        let msg = format!("c{i}");
        let time = 1_600_000_000 + i * 60;
        stream.push_str(&format!(
            "commit refs/heads/main\ncommitter T <t@e> {time} +0000\ndata {}\n{msg}\n",
            msg.len()
        ));
    }
    let mut import = sb
        .git(&long)
        .args(["fast-import", "--quiet"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    import.stdin.take().unwrap().write_all(stream.as_bytes()).unwrap();
    assert!(import.wait().unwrap().success());
    let long_url = "https://example.org/x/long.git";
    rewrite_url(sb.root(), long_url, &long);
    sb.ok(&["add", long_url]);
    assert_eq!(sb.store_config("fetch.negotiationAlgorithm"), "skipping");

    // Each commit the store offers the server is a `have` line.
    let trace = sb.tmp.path().join("packets");
    let mut cmd = command(H);
    isolate_git(&mut cmd, sb.root());
    let out = run(cmd
        .current_dir(sb.root())
        .env("GIT_TRACE_PACKET", &trace)
        .arg("--store")
        .arg(&sb.store)
        .args(["store", "add", PROJ_URL]));
    assert_eq!(out.code, Some(0), "{out:?}");
    let haves = fs::read_to_string(&trace).unwrap().matches("> have ").count();
    assert!(haves < 50, "offered {haves} of the store's 500 commits");
}

/// A second source repository, published as `<PROJ_URL minus .git>/sub.git`.
fn add_sub_source(sb: &Sandbox) -> &'static str {
    let sub = sb.tmp.path().join("src/sub");
    make_git_repo(&sub);
    fs::write(sub.join("README"), "sub\n").unwrap();
    assert!(sb.git(&sub).args(["commit", "-qam", "sub"]).status().unwrap().success());
    let url = "https://example.com/owner/proj/sub.git";
    rewrite_url(sb.root(), url, &sub);
    url
}

fn refs(sb: &Sandbox) -> String {
    run(sb.git(&sb.store).args(["for-each-ref", "--format=%(refname) %(symref)"])).stdout
}

#[test]
fn adding_a_nested_upstream_nests_the_outer_one() {
    let sb = Sandbox::new();
    let sub_url = add_sub_source(&sb);
    assert!(sb.git(&sb.src).args(["tag", "v1.0"]).status().unwrap().success());
    // A branch of the outer project whose name looks like the inner one's refs.
    assert!(sb.git(&sb.src).args(["branch", "sub/main"]).status().unwrap().success());

    sb.ok(&["add", PROJ_URL]);
    assert!(refs(&sb).contains(&format!("refs/remotes/{PROJ}/sub/main")), "natural at first");
    sb.ok(&["add", sub_url]);
    assert_eq!(sb.ok(&["list"]).stdout, format!("{PROJ}\n{PROJ}/sub\n"));
    // The outer upstream's refs moved under `/-/`; the inner one's are where git puts them.
    assert_eq!(
        refs(&sb),
        format!(
            "refs/remotes/{PROJ}/-/HEAD refs/remotes/{PROJ}/-/main\n\
             refs/remotes/{PROJ}/-/main \n\
             refs/remotes/{PROJ}/-/sub/main \n\
             refs/remotes/{PROJ}/sub/HEAD refs/remotes/{PROJ}/sub/main\n\
             refs/remotes/{PROJ}/sub/main \n\
             refs/tags/{PROJ}/-/v1.0 \n"
        )
    );
    assert_eq!(
        sb.store_config(&format!("remote.{PROJ}.fetch")),
        format!("+refs/heads/*:refs/remotes/{PROJ}/-/*\n+refs/tags/*:refs/tags/{PROJ}/-/*")
    );
    assert_eq!(sb.ok(&["show", PROJ_URL, "sub/main:README"]).stdout, "hello\n");
    assert_eq!(sb.ok(&["show", PROJ_URL, "v1.0:README"]).stdout, "hello\n");
    assert_eq!(sb.ok(&["show", "sub", "main:README"]).stdout, "sub\n");

    // Pruning the outer upstream leaves the inner one's refs alone.
    assert!(sb.git(&sb.src).args(["branch", "-D", "sub/main"]).status().unwrap().success());
    sb.ok(&["fetch", "-q", PROJ_URL]);
    assert_eq!(sb.ok(&["show", "sub", "main:README"]).stdout, "sub\n");
    assert_ne!(sb.h(&["show", PROJ_URL, "sub/main:README"]).code, Some(0));
}

#[test]
fn an_outer_upstream_added_later_is_nested_from_the_start() {
    let sb = Sandbox::new();
    let sub_url = add_sub_source(&sb);
    sb.ok(&["add", sub_url]);
    sb.ok(&["add", PROJ_URL]);
    assert_eq!(
        refs(&sb),
        format!(
            "refs/remotes/{PROJ}/-/HEAD refs/remotes/{PROJ}/-/main\n\
             refs/remotes/{PROJ}/-/main \n\
             refs/remotes/{PROJ}/sub/HEAD refs/remotes/{PROJ}/sub/main\n\
             refs/remotes/{PROJ}/sub/main \n"
        )
    );
    assert_eq!(sb.ok(&["show", "proj", "main:README"]).stdout, "hello\n");
    assert_eq!(sb.ok(&["show", "sub", "main:README"]).stdout, "sub\n");
}

#[test]
fn add_accepts_file_urls() {
    let sb = Sandbox::new();
    // Git rejects remote names with a segment starting with `.`, so the store escapes them.
    let dir = tempfile::Builder::new().prefix(".h-store-").tempdir().unwrap();
    let src = dir.path().join(".proj");
    make_git_repo(&src);
    sb.ok(&["add", &format!("file://{}", src.display())]);
    let name = sb.ok(&["list"]).stdout;
    let dir_name = dir.path().file_name().unwrap().to_str().unwrap();
    assert!(name.ends_with(&format!("/_{dir_name}/_.proj\n")), "{name}");
    assert_eq!(sb.ok(&["show", ".proj", "main:README"]).stdout, "hello\n");
}

#[test]
fn add_rejects_bare_names_and_requires_terms() {
    let sb = Sandbox::new();
    let out = sb.h(&["add"]);
    assert_eq!(out.code, Some(1));
    assert_eq!(out.stderr, "Usage: h store add <term>...\n");
    let out = sb.h(&["add", "proj"]);
    assert_eq!(out.code, Some(1));
    assert_eq!(out.stderr, "proj is not in the store\n");
}

#[test]
fn terms_find_upstreams_already_in_the_store() {
    let sb = Sandbox::new();
    sb.ok(&["add", PROJ_URL]);
    for term in [PROJ_URL, "https://EXAMPLE.com/owner/proj", "proj", "PROJ"] {
        let out = sb.h(&["remote", term]);
        if term == "PROJ" {
            assert_eq!(out.code, Some(1), "uppercase terms are case-sensitive: {out:?}");
            continue;
        }
        assert_eq!(out.code, Some(0), "{term}: {out:?}");
        assert_eq!(out.stdout, format!("{PROJ}\n"), "{term}");
    }
    let out = sb.h(&["remote", "other"]);
    assert_eq!(out.stderr, "other is not in the store\n");

    // GitHub shorthand matches a remote case-insensitively without asking GitHub.
    let fake = sb.tmp.path().join("fake-github");
    make_git_repo(&fake);
    rewrite_url(sb.root(), "https://github.com/Owner/Repo.git", &fake);
    sb.ok(&["add", "https://github.com/Owner/Repo.git"]);
    assert_eq!(sb.ok(&["remote", "owner/repo"]).stdout, "github.com/Owner/Repo\n");

    // A bare name shared by two upstreams is ambiguous.
    let other = sb.tmp.path().join("other-proj");
    make_git_repo(&other);
    rewrite_url(sb.root(), "https://example.org/x/proj.git", &other);
    sb.ok(&["add", "https://example.org/x/proj.git"]);
    let out = sb.h(&["remote", "proj"]);
    assert_eq!(out.code, Some(1));
    assert_eq!(
        out.stderr,
        "proj is ambiguous in the store: example.com/owner/proj, example.org/x/proj\n"
    );
}

#[test]
fn show_prints_files_and_commits_from_the_store() {
    let sb = Sandbox::new();
    sb.ok(&["add", PROJ_URL]);
    assert_eq!(sb.ok(&["show", "proj", "main:README"]).stdout, "hello\n");
    let out = sb.ok(&["show", PROJ_URL, "main"]);
    assert!(out.stdout.contains("init"), "{out:?}");
    // A commit hash works too, in full or abbreviated.
    let hash = run(sb.git(&sb.src).args(["rev-parse", "HEAD"])).stdout.trim().to_string();
    for rev in [&hash[..], &hash[..7]] {
        assert_eq!(sb.ok(&["show", "proj", &format!("{rev}:README")]).stdout, "hello\n");
    }
    let out = sb.h(&["show", "proj", "main:missing"]);
    assert_ne!(out.code, Some(0));
    let out = sb.h(&["show", "proj"]);
    assert_eq!(out.stderr, "Usage: h store show <term> <ref>[:<path>]\n");
}

#[test]
fn fetch_updates_everything_or_the_named_upstreams() {
    let sb = Sandbox::new();
    let out = sb.h(&["fetch"]);
    assert_eq!(out.code, Some(1));
    assert!(out.stderr.starts_with("No store at "), "{out:?}");

    sb.ok(&["add", PROJ_URL]);
    sb.publish("second");
    let out = sb.h(&["show", "proj", "main:second"]);
    assert_ne!(out.code, Some(0), "not fetched yet");
    sb.ok(&["fetch", "--quiet"]);
    assert_eq!(sb.ok(&["show", "proj", "main:second"]).stdout, "new\n");

    sb.publish("third");
    sb.ok(&["fetch", "-q", "proj"]);
    assert_eq!(sb.ok(&["show", "proj", "main:third"]).stdout, "new\n");
}

#[test]
fn worktrees_are_detached_checkouts_from_the_store() {
    let sb = Sandbox::new();
    sb.ok(&["add", PROJ_URL]);

    let dir = sb.tmp.path().join("wt");
    let out = sb.ok(&["worktree", "proj", "main", dir.to_str().unwrap()]);
    assert_eq!(out.stdout, format!("{}\n", dir.display()));
    assert_eq!(fs::read_to_string(dir.join("README")).unwrap(), "hello\n");
    let head = run(sb.git(&dir).args(["symbolic-ref", "-q", "HEAD"]));
    assert_ne!(head.code, Some(0), "HEAD is detached");
    assert_eq!(run(sb.git(&sb.store).args(["branch", "--list"])).stdout, "", "no branch created");

    // Without a directory, a temporary one is made.
    let mut cmd = command(H);
    isolate_git(&mut cmd, sb.root());
    let out = run(cmd
        .current_dir(sb.root())
        .env("TMPDIR", sb.tmp.path())
        .arg("--store")
        .arg(&sb.store)
        .args(["store", "worktree", "proj", "main"]));
    assert_eq!(out.code, Some(0), "{out:?}");
    let temp = PathBuf::from(out.stdout.trim_end());
    assert!(temp.starts_with(sb.tmp.path().join("h-worktrees")), "{temp:?}");
    assert_eq!(fs::read_to_string(temp.join("README")).unwrap(), "hello\n");

    // Relative paths are relative to the current directory, not to the store.
    let mut cmd = command(H);
    isolate_git(&mut cmd, sb.root());
    let out = run(cmd
        .current_dir(sb.root())
        .args(["--store", "store", "store", "worktree", "proj"])
        .args(["main", "rel"]));
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(out.stdout, format!("{}\n", sb.root().join("rel").display()));
    assert!(sb.root().join("rel/README").is_file());

    // So can a commit hash.
    let hash = run(sb.git(&sb.src).args(["rev-parse", "HEAD"])).stdout.trim().to_string();
    let dir = sb.tmp.path().join("by-hash");
    sb.ok(&["worktree", "proj", &hash, dir.to_str().unwrap()]);
    assert_eq!(run(sb.git(&dir).args(["rev-parse", "HEAD"])).stdout.trim(), hash);

    let out = sb.h(&["worktree", "proj"]);
    assert_eq!(out.stderr, "Usage: h store worktree <term> <ref> [DIR]\n");
}

#[test]
fn maintain_runs_only_non_destructive_tasks() {
    let sb = Sandbox::new();
    sb.ok(&["add", PROJ_URL]);
    // Leave a stale worktree behind, which the daily and weekly tasks prune.
    let dir = sb.tmp.path().join("stale");
    sb.ok(&["worktree", "proj", "main", dir.to_str().unwrap()]);
    fs::remove_dir_all(&dir).unwrap();

    sb.ok(&["maintain", "hourly"]);
    assert!(sb.store.join("worktrees/stale").is_dir(), "hourly does not prune worktrees");
    sb.ok(&["maintain"]);
    assert!(!sb.store.join("worktrees/stale").exists(), "daily prunes worktrees");
    sb.ok(&["maintain", "weekly"]);
    let objects = run(sb.git(&sb.store).args(["count-objects", "-v"])).stdout;
    assert!(objects.contains("count: 0"), "weekly packs everything: {objects}");
    let out = sb.h(&["maintain", "monthly"]);
    assert_eq!(out.code, Some(1));
    assert_eq!(out.stderr, "Unknown schedule monthly: use hourly, daily or weekly\n");
}

#[test]
fn clones_borrow_objects_from_the_store() {
    let sb = Sandbox::new();
    sb.ok(&["add", PROJ_URL]);
    let root = sb.tmp.path().join("code");
    let mut cmd = command(H);
    isolate_git(&mut cmd, sb.root());
    let out = run(cmd
        .current_dir(sb.root())
        .arg("--root")
        .arg(&root)
        .arg("--store")
        .arg(&sb.store)
        .args(["go", PROJ_URL]));
    assert_eq!(out.code, Some(0), "{out:?}");
    let clone = root.join(PROJ);
    assert_eq!(fs::read_to_string(clone.join("README")).unwrap(), "hello\n");
    let alternates = fs::read_to_string(clone.join(".git/objects/info/alternates")).unwrap();
    assert_eq!(alternates.trim_end(), sb.store.join("objects").to_str().unwrap());
    let objects = run(sb.git(&clone).args(["count-objects", "-v"])).stdout;
    assert!(objects.contains("count: 0\n") && objects.contains("in-pack: 0\n"), "{objects}");
}
