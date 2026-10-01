//! End-to-end tests for the `h` binary.

mod common;

use std::fs;
use std::path::Path;

use common::*;

const H: &str = env!("CARGO_BIN_EXE_h");
const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "Usage: h (<name> | <repo>/<name> | <url>) [git opts]\n";

fn h(cwd: &Path, args: &[&str]) -> Run {
    run(command(H).current_dir(cwd).args(args))
}

fn resolve(root: &Path, term: &str) -> Run {
    run(command(H).current_dir(root).arg("--resolve").arg(root).arg(term))
}

fn assert_failed_in(run: &Run, cwd: &Path, stderr: &str) {
    assert_eq!(run.code, Some(1), "{run:?}");
    assert_eq!(run.stdout, format!("{}\n", canonical(cwd)));
    assert_eq!(run.stderr, stderr);
}

fn assert_resolved(run: &Run, path: &Path) {
    assert_eq!(run.code, Some(0), "{run:?}");
    assert_eq!(run.stdout, format!("{}\n", path.display()));
}

#[test]
fn without_arguments_prints_usage() {
    let tmp = tempfile::tempdir().unwrap();
    let run = h(tmp.path(), &[]);
    let usage = format!("h {VERSION}\nUsage: eval \"$(h-shell-init [options] [code-root])\"\n");
    assert_failed_in(&run, tmp.path(), &usage);
}

#[test]
fn prints_version() {
    let tmp = tempfile::tempdir().unwrap();
    for flag in ["-V", "--version"] {
        let run = h(tmp.path(), &[flag]);
        assert_eq!((run.code, run.stdout.as_str()), (Some(0), format!("h {VERSION}\n").as_str()));
        assert_eq!(run.stderr, "");
    }
}

#[test]
fn without_resolve_reports_not_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let run = h(tmp.path(), &["proj"]);
    assert_failed_in(
        &run,
        tmp.path(),
        &format!(
            "h {VERSION}\nh is not installed\n\nUsage: eval \"$(h-shell-init [code-root])\"\n"
        ),
    );
}

#[test]
fn resolve_requires_root_and_term() {
    let tmp = tempfile::tempdir().unwrap();
    let run = h(tmp.path(), &["--resolve"]);
    assert_failed_in(&run, tmp.path(), "Usage: h --resolve <code-root> <term>\n");
    let run = h(tmp.path(), &["--resolve", "/code"]);
    assert_failed_in(&run, tmp.path(), USAGE);
}

#[test]
fn help_prints_usage() {
    let tmp = tempfile::tempdir().unwrap();
    for flag in ["-h", "--help"] {
        let run = h(tmp.path(), &["--resolve", "/code", flag]);
        assert_failed_in(&run, tmp.path(), USAGE);
    }
}

#[test]
fn finds_projects_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["github.com/owner/Proj", "proj", "gitlab.com/proj"]);
    assert_resolved(&resolve(tmp.path(), "proj"), &tmp.path().join("github.com/owner/Proj"));
    assert_resolved(&resolve(tmp.path(), "Proj"), &tmp.path().join("github.com/owner/Proj"));
}

#[test]
fn reports_missing_projects() {
    let tmp = tempfile::tempdir().unwrap();
    assert_failed_in(&resolve(tmp.path(), "nope"), tmp.path(), "nope not found\n");
}

#[test]
fn reports_unknown_patterns() {
    let tmp = tempfile::tempdir().unwrap();
    for term in ["a/b/c", "two words", ""] {
        let msg = format!("Unknown pattern for {term}\n");
        assert_failed_in(&resolve(tmp.path(), term), tmp.path(), &msg);
    }
}

#[test]
fn reports_scp_urls_without_path() {
    let tmp = tempfile::tempdir().unwrap();
    assert_failed_in(&resolve(tmp.path(), "git@host"), tmp.path(), "git@host not found\n");
}

#[test]
fn expands_tilde_in_code_root() {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["code/example.com/proj"]);
    let run = run(command(H).current_dir(tmp.path()).env("HOME", tmp.path()).args([
        "--resolve",
        "~/code",
        "proj",
    ]));
    assert_resolved(&run, &tmp.path().join("code/example.com/proj"));
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
    let run = run(command(H)
        .current_dir(tmp.path())
        .env("H_GITHUB_API", &api.url)
        .args(["--resolve"])
        .arg(tmp.path())
        .arg("zimbatm/h"));
    assert_resolved(&run, &tmp.path().join("github.com/ZimBatm/H"));

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
    let run = run(command(H)
        .current_dir(tmp.path())
        .env("H_GITHUB_API", &api.url)
        .arg("--resolve")
        .arg(tmp.path())
        .arg("zimbatm/h"));
    assert_resolved(&run, &tmp.path().join("github.com/zimbatm/h"));
    assert_eq!(api.requests().len(), 1);

    // An unreachable API behaves the same.
    assert_resolved(&resolve(tmp.path(), "zimbatm/h"), &tmp.path().join("github.com/zimbatm/h"));
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
        assert_resolved(&resolve(tmp.path(), term), &want);
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
        assert_resolved(&resolve(tmp.path(), term), &want);
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
    let run = run(cmd
        .current_dir(tmp.path())
        .env("H_GITHUB_API", &api.url)
        .arg("--resolve")
        .arg(&root)
        .arg("zimbatm/h"));

    let path = root.join("github.com/Zimbatm/h");
    assert_resolved(&run, &path);
    // Git's stdout goes to stderr so it is not mistaken for the directory.
    assert_eq!(run.stderr, "fake git stdout\n");
    assert!(root.join("github.com/Zimbatm").is_dir(), "parent directories are created");
    assert_eq!(
        git.args().unwrap(),
        ["clone", "--recursive", "--", "https://github.com/Zimbatm/h.git", path.to_str().unwrap()]
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
    let run = run(cmd
        .current_dir(tmp.path())
        .env("H_GITHUB_API", &api.url)
        .arg("--resolve")
        .arg(&root)
        .arg("me/nixpkgs"));

    let path = root.join("github.com/me/nixpkgs");
    assert_resolved(&run, &path);
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
    let run = run(cmd.current_dir(tmp.path()).arg("--resolve").arg(tmp.path()).arg(url));

    let path = tmp.path().join("gitlab.com/group/proj");
    assert_resolved(&run, &path);
    assert_eq!(git.args().unwrap(), ["clone", "--recursive", "--", url, path.to_str().unwrap()]);
}

#[test]
fn passes_extra_arguments_to_git() {
    let tmp = tempfile::tempdir().unwrap();
    let git = FakeGit::install(tmp.path());
    let url = "https://example.com/proj";
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let run = run(cmd
        .current_dir(tmp.path())
        .arg("--resolve")
        .arg(tmp.path())
        .args([url, "--depth", "1", "--branch", "dev"]));

    let path = tmp.path().join("example.com/proj");
    assert_resolved(&run, &path);
    assert_eq!(
        git.args().unwrap(),
        ["clone", "--depth", "1", "--branch", "dev", "--", url, path.to_str().unwrap()]
    );
}

#[test]
fn does_not_clone_existing_directories() {
    let tmp = tempfile::tempdir().unwrap();
    mkdirs(tmp.path(), &["example.com/proj"]);
    let git = FakeGit::install(tmp.path());
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let run = run(cmd
        .current_dir(tmp.path())
        .arg("--resolve")
        .arg(tmp.path())
        .arg("https://example.com/proj.git"));
    assert_resolved(&run, &tmp.path().join("example.com/proj"));
    assert_eq!(git.args(), None);
}

#[test]
fn clone_failure_returns_git_status() {
    let tmp = tempfile::tempdir().unwrap();
    let git = FakeGit::install(tmp.path());
    let mut cmd = command(H);
    git.apply(&mut cmd);
    let run = run(cmd
        .current_dir(tmp.path())
        .env("FAKE_GIT_EXIT", "42")
        .arg("--resolve")
        .arg(tmp.path())
        .arg("https://example.com/proj"));
    assert_eq!(run.code, Some(42));
    assert_eq!(run.stdout, format!("{}\n", canonical(tmp.path())));
}

#[test]
fn missing_git_is_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let run = run(command(H)
        .current_dir(tmp.path())
        .env("PATH", tmp.path().join("empty"))
        .arg("--resolve")
        .arg(tmp.path())
        .arg("https://example.com/proj"));
    assert_eq!(run.code, Some(127));
    assert_eq!(run.stdout, format!("{}\n", canonical(tmp.path())));
    assert!(run.stderr.starts_with("failed to run git: "), "{run:?}");
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
    let run = run(cmd.current_dir(tmp.path()).arg("--resolve").arg(&root).arg(&url));

    // `file:///abs/path` has an empty host, giving `<root>//abs/path`.
    let path = format!("{}/{}", root.display(), src.display());
    let path = Path::new(path.strip_suffix(".git").unwrap());
    assert_resolved(&run, path);
    assert_eq!(fs::read_to_string(path.join("README")).unwrap(), "hello\n");

    // A second call finds the clone instead of cloning again.
    assert_resolved(&resolve(&root, &url), path);
}
