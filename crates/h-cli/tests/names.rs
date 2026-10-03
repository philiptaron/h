//! End-to-end tests for the names GitHub repositories get in the store: GitHub's own, in its
//! casing and after a rename, whatever form or casing a remote's URL has, with what the store
//! already has under another name renamed to it. Git is real; GitHub's repositories are local
//! ones published at their `https://github.com/...` URLs, and its API is a mock.

mod common;

use std::path::{Path, PathBuf};

use common::*;

const H: &str = env!("CARGO_BIN_EXE_h");

struct Sandbox {
    tmp: tempfile::TempDir,
    store: PathBuf,
    api: MockGitHub,
}

impl Sandbox {
    /// `NixOS/proj`, known to the mock API in any casing, and `NewOrg/newname`, which GitHub
    /// says `old/name` was renamed to.
    fn new() -> Sandbox {
        let tmp = tempfile::tempdir().unwrap();
        let api = MockGitHub::serve(|path, _| {
            let body = |owner: &str, name: &str| {
                format!(r#"{{"name": "{name}", "owner": {{"login": "{owner}"}}}}"#)
            };
            match path.to_ascii_lowercase().as_str() {
                "/repos/nixos/proj" => (200, body("NixOS", "proj")),
                "/repos/old/name" | "/repos/neworg/newname" => (200, body("NewOrg", "newname")),
                _ => (404, r#"{"message": "Not Found"}"#.into()),
            }
        });
        let sb = Sandbox { store: tmp.path().join("store"), tmp, api };
        // Every URL the store may fetch from, in each casing the tests give, is served locally.
        for (src, url) in [
            ("proj", "https://github.com/NixOS/proj.git"),
            ("proj", "https://github.com/nixos/proj.git"),
            ("newname", "https://github.com/NewOrg/newname.git"),
            ("newname", "https://github.com/old/name.git"),
        ] {
            let src = sb.home().join("src").join(src);
            if !src.exists() {
                make_git_repo(&src);
                sb.git(&src, &["tag", "v1"]);
            }
            let key = format!("url.file://{}.insteadOf", src.display());
            let config = global_gitconfig(sb.home());
            let out = run(git_command(sb.home())
                .args(["config", "--add", "--file"])
                .arg(&config)
                .args([key.as_str(), url]));
            assert_eq!(out.code, Some(0), "{out:?}");
        }
        sb
    }

    fn home(&self) -> &Path {
        self.tmp.path()
    }

    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = run(git_command(self.home()).arg("-C").arg(dir).args(args));
        assert_eq!(out.code, Some(0), "git {args:?} in {}: {out:?}", dir.display());
        out.stdout.trim().to_string()
    }

    /// A checkout at `code/<dir>` whose `origin` says `url`, made without the store.
    fn checkout(&self, dir: &str, src: &str, url: &str) -> PathBuf {
        let path = self.home().join("code").join(dir);
        let src = self.home().join("src").join(src);
        let out = run(git_command(self.home()).arg("clone").arg("-q").arg(&src).arg(&path));
        assert_eq!(out.code, Some(0), "{out:?}");
        self.git(&path, &["remote", "set-url", "origin", url]);
        path
    }

    /// `h --store <store> <args>` with the mock API, or none at all without `api`.
    fn h(&self, api: bool, args: &[&str]) -> Run {
        let mut cmd = command(H);
        isolate_git(&mut cmd, self.home());
        if api {
            cmd.env("H_GITHUB_API", &self.api.url);
        }
        let out = run(cmd.current_dir(self.home()).arg("--store").arg(&self.store).args(args));
        assert_eq!(out.code, Some(0), "h {args:?}: {out:?}");
        out
    }

    fn ingest(&self, api: bool, dir: &Path) -> Run {
        self.h(api, &["store", "ingest", dir.to_str().unwrap()])
    }

    fn list(&self) -> Vec<String> {
        self.git(&self.store, &["remote"]).lines().map(String::from).collect()
    }

    fn refs(&self, prefix: &str) -> String {
        self.git(&self.store, &["for-each-ref", "--format=%(refname)", prefix])
    }

    fn prefixes(&self, dir: &Path) -> String {
        self.git(dir, &["config", "core.alternateRefsPrefixes"])
    }
}

#[test]
fn a_remote_in_another_casing_gets_githubs_name() {
    let sb = Sandbox::new();
    let dir = sb.checkout("proj", "proj", "https://github.com/nixos/proj");
    let out = sb.ingest(true, &dir);
    assert!(out.stderr.contains("added github.com/NixOS/proj"), "{out:?}");
    assert_eq!(sb.list(), ["github.com/NixOS/proj"]);
    let url = sb.git(&sb.store, &["config", "remote.github.com/NixOS/proj.url"]);
    assert_eq!(url, "https://github.com/NixOS/proj.git");
    assert!(sb.prefixes(&dir).contains("refs/remotes/github.com/NixOS/proj/"));
}

#[test]
fn a_renamed_repository_gets_its_new_name() {
    let sb = Sandbox::new();
    let dir = sb.checkout("name", "newname", "https://github.com/old/name");
    sb.ingest(true, &dir);
    assert_eq!(sb.list(), ["github.com/NewOrg/newname"]);
    assert!(sb.refs("refs/remotes/github.com/NewOrg/newname/").contains("/main"));
}

#[test]
fn ssh_and_scp_urls_name_the_github_repository() {
    let sb = Sandbox::new();
    let dir = sb.checkout("proj", "proj", "ssh://git@github.com/nixos/proj.git");
    sb.git(&dir, &["remote", "add", "scp", "git@github.com:NixOS/proj.git"]);
    // Without the API, too, the user is no part of the name.
    sb.ingest(false, &dir);
    assert_eq!(sb.list(), ["github.com/nixos/proj"]);
    let url = sb.git(&sb.store, &["config", "remote.github.com/nixos/proj.url"]);
    assert_eq!(url, "https://github.com/nixos/proj.git");
    // The scp remote's casing is not the store's, so GitHub is asked, and the store's is wrong.
    let out = sb.ingest(true, &dir);
    assert!(
        out.stderr.contains("renamed github.com/nixos/proj in the store to github.com/NixOS/proj"),
        "{out:?}"
    );
    assert_eq!(sb.list(), ["github.com/NixOS/proj"]);
}

#[test]
fn an_upstream_in_the_wrong_casing_is_renamed_in_place() {
    let sb = Sandbox::new();
    // Without the API, the remote's casing is all there is to go by.
    let old = sb.checkout("old", "proj", "https://github.com/nixos/proj");
    sb.ingest(false, &old);
    assert_eq!(sb.list(), ["github.com/nixos/proj"]);
    assert!(sb.prefixes(&old).contains("refs/remotes/github.com/nixos/proj/"));
    let head = "refs/remotes/github.com/nixos/proj/HEAD";
    assert_eq!(
        sb.git(&sb.store, &["symbolic-ref", head]),
        "refs/remotes/github.com/nixos/proj/main"
    );
    let main = sb.git(&sb.store, &["rev-parse", "refs/remotes/github.com/nixos/proj/main"]);

    // A checkout naming it in GitHub's casing has it renamed, refs, tags and HEAD.
    let new = sb.checkout("new", "proj", "https://github.com/NixOS/proj");
    let out = sb.ingest(true, &new);
    assert!(out.stderr.contains("renamed github.com/nixos/proj"), "{out:?}");
    assert_eq!(sb.list(), ["github.com/NixOS/proj"]);
    assert_eq!(sb.refs("refs/remotes/github.com/nixos/"), "");
    assert_eq!(sb.refs("refs/tags/github.com/nixos/"), "");
    assert_eq!(sb.refs("refs/h-rename/"), "");
    let renamed = sb.git(&sb.store, &["rev-parse", "refs/remotes/github.com/NixOS/proj/main"]);
    assert_eq!(renamed, main);
    sb.git(&sb.store, &["rev-parse", "--verify", "refs/tags/github.com/NixOS/proj/v1"]);
    let head = sb.git(&sb.store, &["symbolic-ref", "refs/remotes/github.com/NixOS/proj/HEAD"]);
    assert_eq!(head, "refs/remotes/github.com/NixOS/proj/main");
    let fetch = sb.git(&sb.store, &["config", "--get-all", "remote.github.com/NixOS/proj.fetch"]);
    assert_eq!(
        fetch,
        "+refs/heads/*:refs/remotes/github.com/NixOS/proj/*\n\
         +refs/tags/*:refs/tags/github.com/NixOS/proj/*"
    );

    // The clone made before still names the old casing, until it is ingested again.
    assert!(sb.prefixes(&old).contains("github.com/nixos/proj/"));
    sb.ingest(true, &old);
    assert_eq!(
        sb.prefixes(&old),
        "refs/remotes/github.com/NixOS/proj/ refs/tags/github.com/NixOS/proj/"
    );
    sb.h(false, &["store", "fetch"]);
}

#[test]
fn a_clone_renames_the_upstream_it_borrows_from_to_githubs_name() {
    let sb = Sandbox::new();
    let old = sb.checkout("old", "proj", "https://github.com/nixos/proj");
    sb.ingest(false, &old);
    assert_eq!(sb.list(), ["github.com/nixos/proj"]);
    let root = sb.home().join("root");
    let out = sb.h(true, &["--root", root.to_str().unwrap(), "go", "nixos/proj"]);
    assert!(out.stderr.contains("renamed github.com/nixos/proj"), "{out:?}");
    assert_eq!(sb.list(), ["github.com/NixOS/proj"]);
    let clone = root.join("github.com/NixOS/proj");
    assert_eq!(
        sb.prefixes(&clone),
        "refs/remotes/github.com/NixOS/proj/ refs/tags/github.com/NixOS/proj/"
    );
}

#[test]
fn store_add_renames_what_the_store_has_under_another_name() {
    let sb = Sandbox::new();
    let dir = sb.checkout("name", "newname", "https://github.com/old/name");
    sb.ingest(false, &dir);
    assert_eq!(sb.list(), ["github.com/old/name"]);
    let out = sb.h(true, &["store", "add", "old/name"]);
    assert!(
        out.stderr
            .contains("renamed github.com/old/name in the store to github.com/NewOrg/newname"),
        "{out:?}"
    );
    assert_eq!(sb.list(), ["github.com/NewOrg/newname"]);
    sb.git(&sb.store, &["rev-parse", "--verify", "refs/tags/github.com/NewOrg/newname/v1"]);
    // Asked again, it is already there.
    let out = sb.h(true, &["store", "add", "NewOrg/newname"]);
    assert!(out.stderr.contains("github.com/NewOrg/newname is already in the store"), "{out:?}");
}

#[test]
fn the_same_repository_under_two_names_is_merged() {
    let sb = Sandbox::new();
    let old = sb.checkout("old", "newname", "https://github.com/old/name");
    sb.ingest(false, &old);
    let new = sb.checkout("new", "newname", "https://github.com/NewOrg/newname");
    sb.ingest(false, &new);
    assert_eq!(sb.list(), ["github.com/NewOrg/newname", "github.com/old/name"]);
    let main = sb.git(&sb.store, &["rev-parse", "refs/remotes/github.com/NewOrg/newname/main"]);

    // A remote naming the old one in another casing has GitHub asked, and the old one goes.
    let other = sb.checkout("other", "newname", "https://github.com/OLD/name");
    let out = sb.ingest(true, &other);
    assert!(out.stderr.contains("renamed github.com/old/name"), "{out:?}");
    assert_eq!(sb.list(), ["github.com/NewOrg/newname"]);
    assert_eq!(sb.refs("refs/remotes/github.com/old/"), "");
    assert_eq!(sb.refs("refs/tags/github.com/old/"), "");
    let kept = sb.git(&sb.store, &["rev-parse", "refs/remotes/github.com/NewOrg/newname/main"]);
    assert_eq!(kept, main);

    // A clone that borrowed under the old name only now borrows under the new one.
    sb.ingest(true, &old);
    assert_eq!(
        sb.prefixes(&old),
        "refs/remotes/github.com/NewOrg/newname/ refs/tags/github.com/NewOrg/newname/"
    );
}
