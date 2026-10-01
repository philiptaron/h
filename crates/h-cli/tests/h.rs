//! End-to-end tests for the `h` binary's `go` and `resolve` commands.

mod common;

use std::fs;
use std::path::Path;

use common::*;

const H: &str = env!("CARGO_BIN_EXE_h");
const VERSION: &str = env!("CARGO_PKG_VERSION");

const GO_USAGE: &str =
    "Usage: h go (<name> | <user>/<repo> | <url>) [clone options] [--container]\n";

fn h(cwd: &Path, args: &[&str]) -> Run {
    run(command(H).current_dir(cwd).args(args))
}

fn go(root: &Path, term: &str) -> Run {
    run(command(H).current_dir(root).arg("--root").arg(root).args(["go", term]))
}

fn assert_failed_in(out: &Run, cwd: &Path, stderr: &str) {
    assert_eq!(out.code, Some(1), "{out:?}");
    assert_eq!(out.stdout, format!("{}\n", canonical(cwd)));
    assert_eq!(out.stderr, stderr);
}

fn assert_resolved(out: &Run, path: &Path) {
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(out.stdout, format!("{}\n", path.display()));
}

#[test]
fn without_arguments_prints_usage() {
    let tmp = tempfile::tempdir().unwrap();
    let out = h(tmp.path(), &[]);
    assert_eq!(out.code, Some(1));
    assert_eq!(out.stdout, "");
    assert!(out.stderr.starts_with(&format!("h {VERSION}\nUsage: h [--root DIR]")), "{out:?}");
}

#[test]
fn prints_version_and_help() {
    let tmp = tempfile::tempdir().unwrap();
    for flag in ["-V", "--version"] {
        let out = h(tmp.path(), &[flag]);
        assert_eq!((out.code, out.stdout.as_str()), (Some(0), format!("h {VERSION}\n").as_str()));
        assert_eq!(out.stderr, "");
    }
    let out = h(tmp.path(), &["--help"]);
    assert_eq!(out.code, Some(0));
    assert!(out.stdout.contains("store worktree <term> <ref> [DIR]"), "{out:?}");
}

#[test]
fn rejects_unknown_options_and_commands() {
    let tmp = tempfile::tempdir().unwrap();
    let out = h(tmp.path(), &["--bogus"]);
    assert_eq!(out.code, Some(1));
    assert!(out.stderr.starts_with("Unknown option: --bogus\n"), "{out:?}");
    let out = h(tmp.path(), &["jump"]);
    assert!(out.stderr.starts_with("Unknown command: jump\n"), "{out:?}");
}

#[test]
fn go_requires_a_term() {
    let tmp = tempfile::tempdir().unwrap();
    let out = h(tmp.path(), &["--root", "/code", "go"]);
    assert_failed_in(&out, tmp.path(), GO_USAGE);
    for flag in ["-h", "--help"] {
        let out = h(tmp.path(), &["--root", "/code", "go", flag]);
        assert_failed_in(&out, tmp.path(), GO_USAGE);
    }
}

#[test]
fn legacy_resolve_flag_still_works() {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["github.com/owner/proj"]);
    let out = run(command(H).current_dir(tmp.path()).arg("--resolve").arg(tmp.path()).arg("proj"));
    assert_resolved(&out, &tmp.path().join("github.com/owner/proj"));
    let out = h(tmp.path(), &["--resolve"]);
    assert_failed_in(&out, tmp.path(), "Usage: h --resolve <code-root> <term>\n");
}

#[test]
fn finds_projects_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["github.com/owner/Proj", "proj", "gitlab.com/proj"]);
    assert_resolved(&go(tmp.path(), "proj"), &tmp.path().join("github.com/owner/Proj"));
    assert_resolved(&go(tmp.path(), "Proj"), &tmp.path().join("github.com/owner/Proj"));
}

#[test]
fn reports_missing_projects() {
    let tmp = tempfile::tempdir().unwrap();
    assert_failed_in(&go(tmp.path(), "nope"), tmp.path(), "nope not found\n");
}

#[test]
fn reports_unknown_patterns() {
    let tmp = tempfile::tempdir().unwrap();
    for term in ["a/b/c", "two words", ""] {
        let msg = format!("Unknown pattern for {term}\n");
        assert_failed_in(&go(tmp.path(), term), tmp.path(), &msg);
    }
}

#[test]
fn reports_scp_urls_without_path() {
    let tmp = tempfile::tempdir().unwrap();
    assert_failed_in(&go(tmp.path(), "git@host"), tmp.path(), "git@host not found\n");
}

#[test]
fn root_comes_from_flag_env_or_default() {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["code/example.com/proj", "src/example.com/proj", "env/example.com/proj"]);
    let home = tmp.path();
    let out = run(command(H)
        .current_dir(home)
        .env("HOME", home)
        .args(["--root", "~/code", "go", "proj"]));
    assert_resolved(&out, &home.join("code/example.com/proj"));
    let out = run(command(H)
        .current_dir(home)
        .env("HOME", home)
        .env("H_CODE_ROOT", "~/env")
        .args(["go", "proj"]));
    assert_resolved(&out, &home.join("env/example.com/proj"));
    let out = run(command(H).current_dir(home).env("HOME", home).args(["go", "proj"]));
    assert_resolved(&out, &home.join("src/example.com/proj"));
}

#[test]
fn resolve_never_clones() {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["github.com/owner/proj"]);
    let git = FakeGit::install(tmp.path());
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let out = run(cmd
        .current_dir(tmp.path())
        .arg("--root")
        .arg(tmp.path())
        .args(["resolve", "owner/proj"]));
    assert_resolved(&out, &tmp.path().join("github.com/owner/proj"));

    let mut cmd = command(H);
    git.apply(&mut cmd);
    let out = run(cmd
        .current_dir(tmp.path())
        .arg("--root")
        .arg(tmp.path())
        .args(["resolve", "owner/other"]));
    assert_eq!(out.code, Some(1));
    assert_eq!(out.stdout, "", "resolve prints nothing on failure");
    assert_eq!(out.stderr, "owner/other not found\n");
    assert_eq!(git.args(), None);
}

#[test]
fn github_shorthand_uses_canonical_casing() {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["github.com/ZimBatm/H"]);
    let api = MockGitHub::start(&[(
        "/repos/zimbatm/h",
        200,
        r#"{"name": "H", "owner": {"login": "ZimBatm"}}"#,
    )]);
    let out = run(command(H)
        .current_dir(tmp.path())
        .env("H_GITHUB_API", &api.url)
        .arg("--root")
        .arg(tmp.path())
        .args(["go", "zimbatm/h"]));
    assert_resolved(&out, &tmp.path().join("github.com/ZimBatm/H"));

    let requests = api.requests();
    assert_eq!(requests.len(), 1);
    let head = requests[0].to_ascii_lowercase();
    assert!(head.starts_with("get /repos/zimbatm/h http/1.1\r\n"), "{head}");
    assert!(head.contains("user-agent: h-cli\r\n"), "{head}");
    assert!(head.contains("accept: application/vnd.github.v3+json\r\n"), "{head}");
}

#[test]
fn github_lookup_failure_keeps_given_casing() {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["github.com/zimbatm/h"]);
    let api = MockGitHub::start(&[("/repos/zimbatm/h", 500, "{}")]);
    let out = run(command(H)
        .current_dir(tmp.path())
        .env("H_GITHUB_API", &api.url)
        .arg("--root")
        .arg(tmp.path())
        .args(["go", "zimbatm/h"]));
    assert_resolved(&out, &tmp.path().join("github.com/zimbatm/h"));
    assert_eq!(api.requests().len(), 1);

    // An unreachable API behaves the same.
    assert_resolved(&go(tmp.path(), "zimbatm/h"), &tmp.path().join("github.com/zimbatm/h"));
}

#[test]
fn github_urls_resolve_to_github_paths() {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["github.com/zimbatm/h"]);
    let want = tmp.path().join("github.com/zimbatm/h");
    for term in [
        "https://github.com/zimbatm/h",
        "https://GitHub.com/zimbatm/h.git",
        "git@github.com:zimbatm/h.git",
        "zimbatm/h.git",
    ] {
        assert_resolved(&go(tmp.path(), term), &want);
    }
}

#[test]
fn other_urls_resolve_under_their_host() {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["gitlab.com/group/sub/proj"]);
    let want = tmp.path().join("gitlab.com/group/sub/proj");
    for term in [
        "https://gitlab.com/group/sub/proj",
        "https://GitLab.com/group/sub/proj.git",
        "git@gitlab.com:group/sub/proj.git",
        "gitea@GITLAB.COM:group/sub/proj",
    ] {
        assert_resolved(&go(tmp.path(), term), &want);
    }
}

#[test]
fn clones_missing_github_repos() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("code");
    let git = FakeGit::install(tmp.path());
    let api = MockGitHub::start(&[(
        "/repos/zimbatm/h",
        200,
        r#"{"name": "h", "owner": {"login": "Zimbatm"}}"#,
    )]);
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let out = run(cmd
        .current_dir(tmp.path())
        .env("H_GITHUB_API", &api.url)
        .arg("--root")
        .arg(&root)
        .args(["go", "zimbatm/h"]));

    let path = root.join("github.com/Zimbatm/h");
    assert_resolved(&out, &path);
    // Git's stdout goes to stderr so it is not mistaken for the directory.
    assert_eq!(out.stderr, "fake git stdout\n");
    assert!(root.join("github.com/Zimbatm").is_dir(), "parent directories are created");
    assert_eq!(
        git.invocations(),
        [[
            "clone",
            "--recursive",
            "--",
            "https://github.com/Zimbatm/h.git",
            path.to_str().unwrap()
        ]]
    );
}

#[test]
fn forks_get_an_unpushable_upstream() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("code");
    let git = FakeGit::install(tmp.path());
    let api = MockGitHub::start(&[(
        "/repos/me/nixpkgs",
        200,
        r#"{"name": "nixpkgs", "owner": {"login": "me"}, "fork": true,
            "parent": {"full_name": "NixOS/nixpkgs"}}"#,
    )]);
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let out = run(cmd
        .current_dir(tmp.path())
        .env("H_GITHUB_API", &api.url)
        .env("FAKE_GIT_STDOUT", "origin")
        .arg("--root")
        .arg(&root)
        .args(["go", "me/nixpkgs"]));

    let path = root.join("github.com/me/nixpkgs");
    assert_resolved(&out, &path);
    let path = path.to_str().unwrap();
    let c = ["-C", path];
    assert_eq!(
        git.invocations(),
        [
            vec!["clone", "--recursive", "--", "https://github.com/me/nixpkgs.git", path],
            [&c[..], &["remote"]].concat(),
            [&c[..], &["remote", "add", "upstream", "https://github.com/NixOS/nixpkgs.git"]]
                .concat(),
            [&c[..], &["config", "remote.upstream.pushurl", "no_push"]].concat(),
            [&c[..], &["config", "remote.upstream.tagOpt", "--no-tags"]].concat(),
            [&c[..], &["config", "remote.pushDefault", "origin"]].concat(),
            [&c[..], &["fetch", "--quiet", "upstream"]].concat(),
        ]
    );
}

#[test]
fn shallow_forks_fetch_a_shallow_upstream() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("code");
    let git = FakeGit::install(tmp.path());
    let api = MockGitHub::start(&[(
        "/repos/me/nixpkgs",
        200,
        r#"{"name": "nixpkgs", "owner": {"login": "me"}, "fork": true,
            "parent": {"full_name": "NixOS/nixpkgs"}}"#,
    )]);
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let out = run(cmd
        .current_dir(tmp.path())
        .env("H_GITHUB_API", &api.url)
        .env("FAKE_GIT_STDOUT", "origin")
        .arg("--root")
        .arg(&root)
        .args(["go", "me/nixpkgs", "--depth", "1", "--branch", "dev"]));

    let path = root.join("github.com/me/nixpkgs");
    assert_resolved(&out, &path);
    let fetch = ["-C", path.to_str().unwrap(), "fetch", "--quiet", "--depth", "1", "upstream"];
    assert_eq!(git.args().unwrap(), fetch);
}

#[test]
fn single_branch_forks_fetch_only_the_upstreams_default_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let fork = Fork::publish(tmp.path(), &["b1", "b2", "b3"]);
    let root = tmp.path().join("code");
    let path = root.join("github.com/me/proj");
    let git = |args: &[&str]| {
        let out = run(git_command(tmp.path()).arg("-C").arg(&path).args(args));
        assert_eq!(out.code, Some(0), "{args:?}: {out:?}");
        out.stdout
    };
    let all = "refs/remotes/upstream/b1\nrefs/remotes/upstream/b2\nrefs/remotes/upstream/b3\n\
               refs/remotes/upstream/main\n";
    for (extra, branches, depth) in [
        // A shallow clone fetches one branch, and so does its upstream, as shallowly.
        (&["--depth", "1"][..], "refs/remotes/upstream/main\n", "1"),
        (&["--single-branch"], "refs/remotes/upstream/main\n", "2"),
        (&["--depth", "1", "--no-single-branch"], all, "1"),
        (&[], all, "2"),
    ] {
        let mut cmd = command(H);
        isolate_git(&mut cmd, tmp.path());
        let out = run(cmd
            .current_dir(tmp.path())
            .env("H_GITHUB_API", &fork.api.url)
            .arg("--root")
            .arg(&root)
            .args(["go", "me/proj"])
            .args(extra));
        assert_resolved(&out, &path);
        let refs = git(&["for-each-ref", "--format=%(refname)", "refs/remotes/upstream/"]);
        let refs = refs.replace("refs/remotes/upstream/HEAD\n", "");
        assert_eq!(refs, branches, "{extra:?}");
        assert_eq!(git(&["rev-list", "--count", "upstream/main"]).trim(), depth, "{extra:?}");
        // Later fetches stay as narrow as the first, as they do for the clone's own remote.
        let refspec = if branches == all { "*" } else { "main" };
        let want = format!("+refs/heads/{refspec}:refs/remotes/upstream/{refspec}\n");
        assert_eq!(git(&["config", "--get-all", "remote.upstream.fetch"]), want, "{extra:?}");
        fs::remove_dir_all(&path).unwrap();
    }
}

#[test]
fn forks_push_to_the_remote_the_clone_has() {
    let tmp = tempfile::tempdir().unwrap();
    let fork = Fork::publish(tmp.path(), &[]);
    let root = tmp.path().join("code");
    let path = root.join("github.com/me/proj");
    let git = |args: &[&str]| run(git_command(tmp.path()).arg("-C").arg(&path).args(args));

    // `clone.defaultRemoteName` and `--origin` both rename the clone's remote.
    let global =
        |args: &[&str]| run(git_command(tmp.path()).args(["config", "--global"]).args(args));
    for (default_name, extra, name) in
        [(Some("mine"), &[][..], "mine"), (None, &["--origin", "theirs"][..], "theirs")]
    {
        match default_name {
            Some(default_name) => global(&["clone.defaultRemoteName", default_name]),
            None => global(&["--unset", "clone.defaultRemoteName"]),
        };
        let mut cmd = command(H);
        isolate_git(&mut cmd, tmp.path());
        let out = run(cmd
            .current_dir(tmp.path())
            .env("H_GITHUB_API", &fork.api.url)
            .arg("--root")
            .arg(&root)
            .args(["go", "me/proj"])
            .args(extra));
        assert_resolved(&out, &path);
        assert_eq!(git(&["remote"]).stdout, format!("{name}\nupstream\n"));
        assert_eq!(git(&["config", "remote.pushDefault"]).stdout.trim(), name);

        git(&["commit", "-q", "--allow-empty", "-m", "mine"]);
        let push = git(&["push", "--dry-run"]);
        assert_eq!(push.code, Some(0), "{name}: {push:?}");
        assert!(push.stderr.contains(fork.fork.to_str().unwrap()), "{name}: {push:?}");
        fs::remove_dir_all(&path).unwrap();
    }
}

#[test]
fn clones_other_urls_verbatim() {
    let tmp = tempfile::tempdir().unwrap();
    let git = FakeGit::install(tmp.path());
    let url = "git@gitlab.com:group/proj.git";
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let out = run(cmd.current_dir(tmp.path()).arg("--root").arg(tmp.path()).args(["go", url]));

    let path = tmp.path().join("gitlab.com/group/proj");
    assert_resolved(&out, &path);
    assert_eq!(git.args().unwrap(), ["clone", "--recursive", "--", url, path.to_str().unwrap()]);
}

#[test]
fn passes_clone_options_and_git_options() {
    let tmp = tempfile::tempdir().unwrap();
    let git = FakeGit::install(tmp.path());
    let url = "https://example.com/proj";
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let out = run(cmd.current_dir(tmp.path()).arg("--root").arg(tmp.path()).args([
        "go",
        url,
        "--depth",
        "1",
        "--branch",
        "dev",
        "--",
        "-c",
        "user.name=Me",
    ]));

    let path = tmp.path().join("example.com/proj");
    assert_resolved(&out, &path);
    assert_eq!(
        git.args().unwrap(),
        [
            "clone",
            "--recursive",
            "-c",
            "user.name=Me",
            "--depth",
            "1",
            "--branch",
            "dev",
            "--",
            url,
            path.to_str().unwrap()
        ]
    );
}

#[test]
fn bare_clones_are_not_recursive() {
    let tmp = tempfile::tempdir().unwrap();
    let git = FakeGit::install(tmp.path());
    let url = "https://example.com/proj";
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let out =
        run(cmd.current_dir(tmp.path()).arg("--root").arg(tmp.path()).args(["go", url, "--bare"]));
    let path = tmp.path().join("example.com/proj");
    assert_resolved(&out, &path);
    assert_eq!(git.args().unwrap(), ["clone", "--bare", "--", url, path.to_str().unwrap()]);
}

#[test]
fn clones_borrow_from_an_existing_store() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("store");
    let init = git_command(tmp.path()).args(["init", "-q", "--bare"]).arg(&store).status().unwrap();
    assert!(init.success());
    let git = FakeGit::install(tmp.path());
    let url = "https://example.com/proj";
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let out = run(cmd
        .current_dir(tmp.path())
        .arg("--root")
        .arg(tmp.path())
        .arg("--store")
        .arg(&store)
        .args(["go", url]));
    let path = tmp.path().join("example.com/proj");
    assert_resolved(&out, &path);
    // The store's upstreams are listed to scope negotiation, and submodules are cloned after the
    // superproject, so that they borrow from the store too.
    let (store, path) = (store.to_str().unwrap(), path.to_str().unwrap());
    let c = ["-C", path];
    let settings = r"^(submodule\.stickyrecursiveclone|clone\.filtersubmodules)$";
    assert_eq!(
        git.invocations(),
        [
            vec!["-C", store, "remote"],
            vec![
                "clone",
                "--reference-if-able",
                store,
                "-c",
                "core.alternateRefsPrefixes=refs/remotes/example.com/proj/ refs/tags/example.com/proj/",
                "-c",
                "submodule.active=.",
                "--no-recurse-submodules",
                "--",
                url,
                path
            ],
            [&c[..], &["config", "--type=bool", "--get-regexp", settings]].concat(),
            // The fake git reports a negotiator, so h leaves it be.
            [&c[..], &["config", "--get", "fetch.negotiationAlgorithm"]].concat(),
            [
                &c[..],
                &["submodule", "update", "--require-init", "--recursive", "--reference", store],
                &["--no-single-branch"]
            ]
            .concat(),
        ]
    );

    // A store that does not exist yet is simply not used.
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let out = run(cmd
        .current_dir(tmp.path())
        .arg("--root")
        .arg(tmp.path().join("other"))
        .env("H_STORE", tmp.path().join("missing"))
        .args(["go", url]));
    assert_eq!(out.code, Some(0), "{out:?}");
    let args = git.args().unwrap();
    assert_eq!(args[..2], ["clone", "--recursive"]);
    assert!(!args.contains(&"--reference-if-able".to_string()), "{args:?}");
}

#[test]
fn does_not_clone_existing_directories() {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["example.com/proj"]);
    let git = FakeGit::install(tmp.path());
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let out = run(cmd
        .current_dir(tmp.path())
        .arg("--root")
        .arg(tmp.path())
        .args(["go", "https://example.com/proj.git"]));
    assert_resolved(&out, &tmp.path().join("example.com/proj"));
    assert_eq!(git.args(), None);
}

#[test]
fn clone_failure_returns_git_status() {
    let tmp = tempfile::tempdir().unwrap();
    let git = FakeGit::install(tmp.path());
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let out = run(cmd
        .current_dir(tmp.path())
        .env("FAKE_GIT_EXIT", "42")
        .arg("--root")
        .arg(tmp.path())
        .args(["go", "https://example.com/proj"]));
    assert_eq!(out.code, Some(42));
    assert_eq!(out.stdout, format!("{}\n", canonical(tmp.path())));
}

#[test]
fn missing_git_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run(command(H)
        .current_dir(tmp.path())
        .env("PATH", tmp.path().join("empty"))
        .arg("--root")
        .arg(tmp.path())
        .args(["go", "https://example.com/proj"]));
    assert_eq!(out.code, Some(127));
    assert_eq!(out.stdout, format!("{}\n", canonical(tmp.path())));
    assert!(out.stderr.starts_with("failed to run git: "), "{out:?}");
}

#[test]
fn clones_with_real_git() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("upstream/proj.git");
    make_git_repo(&src);
    let root = tmp.path().join("code");
    let url = format!("file://{}", src.display());

    let mut cmd = command(H);
    isolate_git(&mut cmd, tmp.path());
    let out = run(cmd.current_dir(tmp.path()).arg("--root").arg(&root).args(["go", &url]));

    // `file:///abs/path` has an empty host, giving `<root>//abs/path`.
    let path = format!("{}/{}", root.display(), src.display());
    let path = Path::new(path.strip_suffix(".git").unwrap());
    assert_resolved(&out, path);
    assert_eq!(fs::read_to_string(path.join("README")).unwrap(), "hello\n");

    // A second call finds the clone instead of cloning again.
    assert_resolved(&go(&root, &url), path);
}

#[test]
fn container_clones_are_bare_with_a_git_file() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    make_git_repo(&src);
    let root = tmp.path().join("code");
    let url = "https://example.com/owner/proj.git";
    rewrite_url(tmp.path(), url, &src);

    let mut cmd = command(H);
    isolate_git(&mut cmd, tmp.path());
    let out = run(cmd.current_dir(tmp.path()).arg("--root").arg(&root).args([
        "go",
        url,
        "--container",
        "--",
        "-c",
        "user.name=Me",
    ]));
    let path = root.join("example.com/owner/proj");
    assert_resolved(&out, &path);

    assert_eq!(fs::read_to_string(path.join(".git")).unwrap(), "gitdir: ./.bare\n");
    assert!(path.join(".bare/HEAD").is_file());
    assert!(!path.join("README").exists(), "no working tree in the container");
    let git = |args: &[&str]| {
        let out = run(git_command(tmp.path()).arg("-C").arg(&path).args(args));
        assert_eq!(out.code, Some(0), "{out:?}");
        out.stdout.trim().to_string()
    };
    assert_eq!(git(&["rev-parse", "--is-bare-repository"]), "true");
    assert_eq!(git(&["config", "user.name"]), "Me");
    assert_eq!(git(&["rev-parse", "--abbrev-ref", "origin/HEAD"]), "origin/main");
    assert_eq!(git(&["for-each-ref", "refs/heads"]), "", "no local branches are created");

    // Work happens in worktrees beside the bare repository.
    git(&["worktree", "add", "--quiet", "feature", "origin/main"]);
    assert_eq!(fs::read_to_string(path.join("feature/README")).unwrap(), "hello\n");
    // HEAD is detached at the default branch, so a new branch starts there, not as an orphan.
    assert_eq!(git(&["rev-parse", "HEAD"]), git(&["rev-parse", "origin/main"]));
    git(&["worktree", "add", "--quiet", "topic"]);
    assert_eq!(fs::read_to_string(path.join("topic/README")).unwrap(), "hello\n");
}

#[test]
fn container_clones_take_the_remote_object_format() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("code");
    let sha1 = tmp.path().join("sha1");
    make_git_repo(&sha1);
    let sha256 = tmp.path().join("sha256");
    for args in [
        &["init", "-q", "-b", "main", "--object-format=sha256"][..],
        &["commit", "-q", "--allow-empty", "-m", "init"],
    ] {
        fs::create_dir_all(&sha256).unwrap();
        let status = git_command(tmp.path()).current_dir(&sha256).args(args).status().unwrap();
        assert!(status.success(), "{args:?}");
    }

    // A SHA-1 remote cloned where new repositories default to SHA-256, and the reverse.
    for (name, src, default) in [("a", &sha1, "sha256"), ("b", &sha256, "sha1")] {
        let url = format!("https://example.com/owner/{name}.git");
        rewrite_url(tmp.path(), &url, src);
        let mut cmd = command(H);
        isolate_git(&mut cmd, tmp.path());
        let out = run(cmd
            .current_dir(tmp.path())
            .env("GIT_DEFAULT_HASH", default)
            .arg("--root")
            .arg(&root)
            .args(["go", &url, "--container"]));
        let path = root.join("example.com/owner").join(name);
        assert_resolved(&out, &path);
        let git = |args: &[&str]| run(git_command(tmp.path()).arg("-C").arg(&path).args(args));
        let format = git(&["rev-parse", "--show-object-format"]).stdout;
        let want = if default == "sha1" { "sha256" } else { "sha1" };
        assert_eq!(format.trim(), want, "{name}");
        let src_head = run(git_command(tmp.path()).arg("-C").arg(src).args(["rev-parse", "HEAD"]));
        assert_eq!(git(&["rev-parse", "HEAD"]).stdout, src_head.stdout, "{name}");
    }

    // An empty remote has no format to take and no branch to start worktrees from.
    let empty = tmp.path().join("empty");
    let status = git_command(tmp.path()).args(["init", "-q", "--bare"]).arg(&empty).status();
    assert!(status.unwrap().success());
    let url = "https://example.com/owner/empty.git";
    rewrite_url(tmp.path(), url, &empty);
    let mut cmd = command(H);
    isolate_git(&mut cmd, tmp.path());
    let out =
        run(cmd.current_dir(tmp.path()).arg("--root").arg(&root).args(["go", url, "--container"]));
    assert_failed_in(
        &out,
        tmp.path(),
        &format!("{url} has no default branch, so it cannot be cloned with --container\n"),
    );
    assert!(!root.join("example.com/owner/empty").exists());
}

#[test]
fn going_to_a_container_brings_its_head_up_to_origin_head() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    make_git_repo(&src);
    let root = tmp.path().join("code");
    let url = "https://example.com/owner/proj.git";
    rewrite_url(tmp.path(), url, &src);
    let path = root.join("example.com/owner/proj");
    let go = || {
        let mut cmd = command(H);
        isolate_git(&mut cmd, tmp.path());
        let args = ["go", url, "--container"];
        assert_resolved(
            &run(cmd.current_dir(tmp.path()).arg("--root").arg(&root).args(args)),
            &path,
        );
    };
    let git = |dir: &Path, args: &[&str]| {
        let out = run(git_command(tmp.path()).arg("-C").arg(dir).args(args));
        assert_eq!(out.code, Some(0), "{out:?}");
        out.stdout.trim().to_string()
    };
    let upstream_moves_on = || {
        git(&src, &["commit", "-q", "--allow-empty", "-m", "upstream moves on"]);
        git(&path, &["fetch", "-q"]);
    };
    go();

    // After a fetch, HEAD is still where the container was cloned, until `h` goes there again.
    upstream_moves_on();
    let tip = git(&path, &["rev-parse", "origin/main"]);
    assert_ne!(git(&path, &["rev-parse", "HEAD"]), tip);
    go();
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), tip);
    git(&path, &["worktree", "add", "--quiet", "topic"]);
    assert_eq!(git(&path, &["rev-parse", "topic"]), tip, "new branches start from the fetch");

    // Without origin/HEAD, HEAD stays put.
    upstream_moves_on();
    git(&path, &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"]);
    go();
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), tip);

    // A HEAD the user made symbolic is left alone.
    git(&path, &["remote", "set-head", "origin", "main"]);
    git(&path, &["symbolic-ref", "HEAD", "refs/heads/topic"]);
    go();
    assert_eq!(git(&path, &["symbolic-ref", "HEAD"]), "refs/heads/topic");
}

#[test]
fn container_clones_can_be_moved_with_their_worktrees() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    make_git_repo(&src);
    let root = tmp.path().join("code");
    let url = "https://example.com/owner/proj.git";
    rewrite_url(tmp.path(), url, &src);
    let mut cmd = command(H);
    isolate_git(&mut cmd, tmp.path());
    let out =
        run(cmd.current_dir(tmp.path()).arg("--root").arg(&root).args(["go", url, "--container"]));
    let path = root.join("example.com/owner/proj");
    assert_resolved(&out, &path);
    let git = |dir: &Path, args: &[&str]| {
        let out = run(git_command(tmp.path()).arg("-C").arg(dir).args(args));
        assert_eq!(out.code, Some(0), "{out:?}");
        out.stdout.trim().to_string()
    };
    git(&path, &["worktree", "add", "--quiet", "feature"]);
    let link = fs::read_to_string(path.join("feature/.git")).unwrap();
    assert_eq!(link, "gitdir: ../.bare/worktrees/feature\n");

    let moved = tmp.path().join("elsewhere");
    fs::rename(&path, &moved).unwrap();
    assert_eq!(git(&moved.join("feature"), &["rev-parse", "--abbrev-ref", "HEAD"]), "feature");
    assert_eq!(git(&moved.join("feature"), &["status", "--porcelain"]), "");
    let list = git(&moved, &["worktree", "list", "--porcelain"]);
    assert!(list.contains(&format!("worktree {}\n", moved.join("feature").display())), "{list}");
    assert!(!list.contains("prunable"), "{list}");
}

#[test]
fn container_clones_keep_no_tags_for_later_fetches() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    make_git_repo(&src);
    let src_git = |args: &[&str]| {
        let out = run(git_command(tmp.path()).arg("-C").arg(&src).args(args));
        assert_eq!(out.code, Some(0), "{out:?}");
    };
    src_git(&["tag", "v1"]);
    let root = tmp.path().join("code");
    let url = "https://example.com/owner/proj.git";
    rewrite_url(tmp.path(), url, &src);

    let mut cmd = command(H);
    isolate_git(&mut cmd, tmp.path());
    let out = run(cmd.current_dir(tmp.path()).arg("--root").arg(&root).args([
        "go",
        url,
        "--container",
        "--no-tags",
    ]));
    let path = root.join("example.com/owner/proj");
    assert_resolved(&out, &path);
    let git = |args: &[&str]| run(git_command(tmp.path()).arg("-C").arg(&path).args(args)).stdout;
    assert_eq!(git(&["config", "remote.origin.tagOpt"]), "--no-tags\n");
    assert_eq!(git(&["tag"]), "");
    src_git(&["tag", "v2"]);
    src_git(&["commit", "-q", "--allow-empty", "-m", "tagged later"]);
    src_git(&["tag", "v3"]);
    git(&["fetch", "-q"]);
    assert_eq!(git(&["tag"]), "", "later fetches bring no tags either");
}

#[test]
fn failed_container_clones_leave_nothing_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("code");
    let url = "https://example.com/owner/proj.git";
    rewrite_url(tmp.path(), url, &tmp.path().join("missing"));
    let path = root.join("example.com/owner/proj");

    for extra in [&[][..], &["--branch", "dev"]] {
        let mut cmd = command(H);
        isolate_git(&mut cmd, tmp.path());
        let out = run(cmd
            .current_dir(tmp.path())
            .arg("--root")
            .arg(&root)
            .args(["go", url, "--container"])
            .args(extra));
        assert_ne!(out.code, Some(0), "{out:?}");
        assert_eq!(out.stdout, format!("{}\n", canonical(tmp.path())));
        assert!(!path.exists(), "{extra:?}: {out:?}");
    }
    let mut cmd = command(H);
    isolate_git(&mut cmd, tmp.path());
    let out = run(cmd.current_dir(tmp.path()).arg("--root").arg(&root).args([
        "go",
        url,
        "--container",
        "--branch",
        "dev",
    ]));
    assert_eq!(out.stderr, "--branch cannot be used with --container\n");
}
