//! End-to-end tests for `h hook`, the commands behind Claude Code's worktree hooks, fed hook JSON
//! on stdin as Claude Code does, against real git.

mod common;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::*;

const H: &str = env!("CARGO_BIN_EXE_h");

const URL: &str = "https://example.com/owner/proj.git";

/// A temporary home with git isolated in it.
struct Sandbox {
    tmp: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Sandbox {
        Sandbox { tmp: tempfile::tempdir().unwrap() }
    }

    fn home(&self) -> &Path {
        self.tmp.path()
    }

    /// `h <args>`, run in the home directory with git isolated.
    fn h(&self, args: &[&str]) -> Command {
        let mut cmd = command(H);
        isolate_git(&mut cmd, self.home());
        cmd.current_dir(self.home()).args(args);
        cmd
    }

    fn ok(&self, mut cmd: Command) -> Run {
        let out = run(&mut cmd);
        assert_eq!(out.code, Some(0), "{cmd:?}: {out:?}");
        out
    }

    /// `git -C <dir> <args>`, which must succeed; its stdout, trimmed.
    fn git(&self, dir: &Path, args: &[&str]) -> String {
        let out = run(git_command(self.home()).arg("-C").arg(dir).args(args));
        assert_eq!(out.code, Some(0), "git {args:?} in {}: {out:?}", dir.display());
        out.stdout.trim().to_string()
    }

    fn store(&self) -> PathBuf {
        self.home().join("store")
    }

    /// `h --root <home>/code [--store <home>/store] go <args>`.
    fn go(&self, store: bool, args: &[&str]) -> Command {
        let root = self.home().join("code");
        let mut cmd = self.h(&["--root", root.to_str().unwrap()]);
        if store {
            cmd.arg("--store").arg(self.store());
        }
        cmd.arg("go").args(args);
        cmd
    }

    /// `h --store <home>/store store <args>`.
    fn store_cmd(&self, args: &[&str]) -> Command {
        let mut cmd = self.h(&["--store", self.store().to_str().unwrap(), "store"]);
        cmd.args(args);
        cmd
    }

    /// Publish a one-commit repository at [`URL`]; returns it.
    fn publish(&self) -> PathBuf {
        let src = self.home().join("src/proj");
        make_git_repo(&src);
        rewrite_url(self.home(), URL, &src);
        src
    }

    /// Commit an empty commit named `message` in the repository at `dir`; returns it.
    fn commit(&self, dir: &Path, message: &str) -> String {
        self.git(dir, &["commit", "-q", "--allow-empty", "-m", message]);
        self.git(dir, &["rev-parse", "HEAD"])
    }
}

/// Run `cmd`, feeding it `input` on stdin, as Claude Code runs a command hook.
fn with_input(mut cmd: Command, input: &str) -> Run {
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    Run {
        code: out.status.code(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

/// The JSON Claude Code sends the WorktreeCreate hook.
fn create_input(name: &str, cwd: &Path) -> String {
    format!(
        r#"{{"session_id":"s","transcript_path":"/t.jsonl","cwd":{:?},"permission_mode":"default","hook_event_name":"WorktreeCreate","name":{name:?}}}"#,
        cwd.to_str().unwrap()
    )
}

impl Sandbox {
    /// `h hook worktree-create` for `name` from `cwd`, which must succeed and print exactly the
    /// worktree's path: `<repository>/.claude/worktrees/<name>`.
    fn create(&self, repository: &Path, name: &str, cwd: &Path) -> (PathBuf, Run) {
        let out = with_input(self.h(&["hook", "worktree-create"]), &create_input(name, cwd));
        assert_eq!(out.code, Some(0), "{out:?}");
        let path = PathBuf::from(canonical(repository)).join(".claude/worktrees").join(name);
        assert_eq!(out.stdout, format!("{}\n", path.display()), "only the path, on stdout");
        assert!(path.join(".git").is_file(), "{out:?}");
        (path, out)
    }
}

#[test]
fn fork_worktrees_start_from_the_stores_copy_of_the_parent() {
    let sb = Sandbox::new();
    let fork = Fork::publish(sb.home(), &[]);
    sb.ok(sb.store_cmd(&["add", Fork::UPSTREAM_URL]));
    let mut go = sb.go(true, &["me/proj"]);
    go.env("H_GITHUB_API", &fork.api.url);
    sb.ok(go);
    let clone = sb.home().join("code/github.com/me/proj");

    // The parent moves on and the store fetches it; the clone's own refs stay behind.
    let newer = sb.commit(&fork.upstream, "newer");
    sb.ok(sb.store_cmd(&["fetch", "-q"]));
    assert_ne!(sb.git(&clone, &["rev-parse", "upstream/HEAD"]), newer);
    assert_ne!(sb.git(&clone, &["rev-parse", "HEAD"]), newer);
    // Nothing can be fetched any more, so the hook must not need to.
    fs::rename(&fork.upstream, sb.home().join("gone-upstream")).unwrap();
    fs::rename(&fork.fork, sb.home().join("gone-fork")).unwrap();

    // The store comes from the clone's alternates, not from --store or $H_STORE.
    let (path, out) = sb.create(&clone, "agent-a1", &clone);
    assert_eq!(sb.git(&path, &["rev-parse", "HEAD"]), newer);
    assert_eq!(sb.git(&path, &["symbolic-ref", "--short", "HEAD"]), "worktree-agent-a1");
    assert!(out.stderr.contains("github.com/up/proj/HEAD in the store"), "{out:?}");

    // From inside a worktree, the next one still goes in the clone, beside it.
    let (inner, _) = sb.create(&clone, "agent-b2", &path);
    assert_eq!(sb.git(&inner, &["rev-parse", "HEAD"]), newer);
    // Slashes become `+`, as in Claude Code's own worktrees.
    let input = create_input("feature/login", &clone);
    let out = with_input(sb.h(&["hook", "worktree-create"]), &input);
    assert_eq!(out.code, Some(0), "{out:?}");
    let slashed = format!("{}/.claude/worktrees/feature+login\n", canonical(&clone));
    assert_eq!(out.stdout, slashed);
    let branch = sb.git(Path::new(slashed.trim_end()), &["symbolic-ref", "--short", "HEAD"]);
    assert_eq!(branch, "worktree-feature+login");
}

#[test]
fn without_the_store_worktrees_start_from_the_clones_refs() {
    let sb = Sandbox::new();

    // A plain clone starts from origin/HEAD, not from local work on its HEAD, even with a store
    // configured that does not have it.
    let src = sb.publish();
    sb.ok(sb.go(false, &[URL]));
    let clone = sb.home().join("code/example.com/owner/proj");
    sb.commit(&clone, "local work");
    sb.ok(sb.store_cmd(&["init"]));
    let input = create_input("agent-a1", &clone);
    let mut cmd = sb.h(&["--store", sb.store().to_str().unwrap(), "hook", "worktree-create"]);
    cmd.current_dir(&clone);
    let out = with_input(cmd, &input);
    assert_eq!(out.code, Some(0), "{out:?}");
    let path = PathBuf::from(out.stdout.trim_end());
    assert_eq!(sb.git(&path, &["rev-parse", "HEAD"]), sb.git(&src, &["rev-parse", "HEAD"]));
    assert!(out.stderr.contains("from refs/remotes/origin/HEAD"), "{out:?}");

    // A fork starts from its parent's branch, not its own.
    let fork = Fork::publish(sb.home(), &[]);
    let mut go = sb.go(false, &["me/proj"]);
    go.env("H_GITHUB_API", &fork.api.url);
    sb.ok(go);
    let clone = sb.home().join("code/github.com/me/proj");
    let (path, _) = sb.create(&clone, "agent-b2", &clone);
    let parent = sb.git(&fork.upstream, &["rev-parse", "HEAD"]);
    assert_eq!(sb.git(&path, &["rev-parse", "HEAD"]), parent);
    assert_ne!(sb.git(&clone, &["rev-parse", "origin/HEAD"]), parent);

    // A repository with no remotes starts from HEAD.
    let lonely = sb.home().join("lonely");
    make_git_repo(&lonely);
    let head = sb.commit(&lonely, "second");
    let (path, _) = sb.create(&lonely, "agent-c3", &lonely);
    assert_eq!(sb.git(&path, &["rev-parse", "HEAD"]), head);
}

#[test]
fn container_worktrees_go_in_the_container() {
    let sb = Sandbox::new();
    let src = sb.publish();
    sb.ok(sb.store_cmd(&["add", URL]));
    sb.ok(sb.go(true, &[URL, "--container"]));
    let container = sb.home().join("code/example.com/owner/proj");
    let newer = sb.commit(&src, "newer");
    sb.ok(sb.store_cmd(&["fetch", "-q"]));
    assert_ne!(sb.git(&container, &["rev-parse", "origin/HEAD"]), newer);

    let (path, _) = sb.create(&container, "agent-a1", &container);
    assert_eq!(sb.git(&path, &["rev-parse", "HEAD"]), newer);
    let listed = sb.git(&container, &["worktree", "list", "--porcelain"]);
    assert!(listed.contains(&format!("worktree {}\n", path.display())), "{listed}");
}

#[test]
fn existing_worktrees_and_branches_are_used_again() {
    let sb = Sandbox::new();
    sb.publish();
    sb.ok(sb.go(false, &[URL]));
    let clone = sb.home().join("code/example.com/owner/proj");
    let (path, _) = sb.create(&clone, "agent-a1", &clone);
    let work = sb.commit(&path, "work");

    // Asked again, the hook hands back the same worktree.
    let (again, _) = sb.create(&clone, "agent-a1", &clone);
    assert_eq!(again, path);
    // Once the worktree is gone but its branch is not, the branch is checked out again.
    sb.git(&clone, &["worktree", "remove", path.to_str().unwrap()]);
    let (path, _) = sb.create(&clone, "agent-a1", &clone);
    assert_eq!(sb.git(&path, &["rev-parse", "HEAD"]), work);
}

#[test]
fn bad_input_is_refused() {
    let sb = Sandbox::new();
    let out = run(sb.h(&["hook"]));
    assert_eq!(
        (out.code, out.stderr.as_str()),
        (Some(1), "Usage: h hook worktree-create < hook-input.json\n")
    );

    let out = with_input(sb.h(&["hook", "worktree-create"]), "not json");
    assert_eq!(out.code, Some(1), "{out:?}");
    assert!(out.stderr.starts_with("h hook worktree-create: hook input: "), "{out:?}");
    assert_eq!(out.stdout, "");

    let out = with_input(sb.h(&["hook", "worktree-create"]), &create_input("x", sb.home()));
    assert_eq!(out.code, Some(1), "{out:?}");
    assert!(out.stderr.contains("is not in a git repository"), "{out:?}");

    // Something that is not a worktree where the worktree would go is left alone.
    let repo = sb.home().join("repo");
    make_git_repo(&repo);
    fs::create_dir_all(repo.join(".claude/worktrees/taken")).unwrap();
    let out = with_input(sb.h(&["hook", "worktree-create"]), &create_input("taken", &repo));
    assert_eq!(out.code, Some(1), "{out:?}");
    assert!(out.stderr.contains("exists and is not a worktree of"), "{out:?}");
}
