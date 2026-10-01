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
    assert!(out.stdout.contains("resolve <term>"), "{out:?}");
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
    assert_eq!(git(&["branch", "--list"]), "", "no local branches are created");

    // Work happens in worktrees beside the bare repository.
    git(&["worktree", "add", "--quiet", "feature", "origin/main"]);
    assert_eq!(fs::read_to_string(path.join("feature/README")).unwrap(), "hello\n");
}
