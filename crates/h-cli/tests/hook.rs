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
        (Some(1), "Usage: h hook (worktree-create | worktree-remove) < hook-input.json\n")
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

/// The JSON Claude Code sends the WorktreeRemove hook.
fn remove_input(path: &Path) -> String {
    format!(
        r#"{{"session_id":"s","transcript_path":"/t.jsonl","cwd":"/","permission_mode":"default","hook_event_name":"WorktreeRemove","worktree_path":{:?}}}"#,
        path.to_str().unwrap()
    )
}

impl Sandbox {
    /// `h hook worktree-remove` for the worktree at `path`.
    fn remove(&self, path: &Path) -> Run {
        with_input(self.h(&["hook", "worktree-remove"]), &remove_input(path))
    }

    /// A clone of [`URL`] with the hook's worktree `agent-a1`: the clone, and the worktree.
    fn clone_with_worktree(&self) -> (PathBuf, PathBuf) {
        self.publish();
        self.ok(self.go(false, &[URL]));
        let clone = self.home().join("code/example.com/owner/proj");
        let (path, _) = self.create(&clone, "agent-a1", &clone);
        (clone, path)
    }

    /// Whether the clone still lists the worktree at `path`.
    fn lists(&self, clone: &Path, path: &Path) -> bool {
        let listed = self.git(clone, &["worktree", "list", "--porcelain"]);
        listed.contains(&format!("worktree {}\n", path.display()))
    }
}

#[test]
fn removing_a_worktree_commits_what_was_left_in_it() {
    let sb = Sandbox::new();
    let (clone, path) = sb.clone_with_worktree();
    let base = sb.git(&path, &["rev-parse", "HEAD"]);
    fs::write(path.join("README"), "changed\n").unwrap();
    fs::create_dir_all(path.join("dir")).unwrap();
    fs::write(path.join("dir/new.txt"), "untracked\n").unwrap();
    // Neither a hook that refuses every commit nor a signing key that cannot sign stops it.
    let hooks = sb.home().join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    for hook in ["pre-commit", "commit-msg", "prepare-commit-msg"] {
        fs::write(hooks.join(hook), "#!/bin/sh\nexit 1\n").unwrap();
        let mode = std::os::unix::fs::PermissionsExt::from_mode(0o755);
        fs::set_permissions(hooks.join(hook), mode).unwrap();
    }
    sb.git(&clone, &["config", "core.hooksPath", hooks.to_str().unwrap()]);
    sb.git(&clone, &["config", "commit.gpgsign", "true"]);
    sb.git(&clone, &["config", "gpg.program", "/nonexistent/gpg"]);

    let out = sb.remove(&path);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert_eq!(out.stdout, "");
    assert!(!path.exists());
    assert!(!sb.lists(&clone, &path));
    let branch = "worktree-agent-a1";
    assert!(out.stderr.contains(&format!("uncommitted work is on {branch}")), "{out:?}");
    assert_eq!(sb.git(&clone, &["show", &format!("{branch}:README")]), "changed");
    assert_eq!(sb.git(&clone, &["show", &format!("{branch}:dir/new.txt")]), "untracked");
    assert_eq!(sb.git(&clone, &["rev-parse", &format!("{branch}^")]), base);
    let subject = sb.git(&clone, &["log", "-1", "--format=%s", branch]);
    assert!(subject.starts_with("WIP: what was left uncommitted in "), "{subject}");

    // Asked for again, the worktree comes back with that work.
    let (again, _) = sb.create(&clone, "agent-a1", &clone);
    assert_eq!(fs::read_to_string(again.join("dir/new.txt")).unwrap(), "untracked\n");
}

#[test]
fn removing_a_clean_worktree_keeps_its_branch_as_it_is() {
    let sb = Sandbox::new();
    let (clone, path) = sb.clone_with_worktree();
    let work = sb.commit(&path, "work");
    let out = sb.remove(&path);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert!(out.stderr.contains("its work is on worktree-agent-a1"), "{out:?}");
    assert!(!path.exists());
    assert_eq!(sb.git(&clone, &["rev-parse", "worktree-agent-a1"]), work, "no commit added");
}

#[test]
fn a_detached_head_with_commits_of_its_own_gets_a_branch() {
    let sb = Sandbox::new();
    let (clone, path) = sb.clone_with_worktree();
    let base = sb.git(&path, &["rev-parse", "HEAD"]);
    sb.git(&path, &["switch", "--quiet", "--detach"]);
    let detached = sb.commit(&path, "detached work");
    fs::write(path.join("loose.txt"), "loose\n").unwrap();

    let out = sb.remove(&path);
    assert_eq!(out.code, Some(0), "{out:?}");
    assert!(!path.exists());
    // worktree-agent-a1 is still at the base, so the work gets a branch beside it, named for the
    // commit it ends at: the one that keeps the loose file.
    assert_eq!(sb.git(&clone, &["rev-parse", "worktree-agent-a1"]), base);
    let pattern = "refs/heads/worktree-agent-a1-*";
    let branch = sb.git(&clone, &["for-each-ref", "--format=%(refname:short)", pattern]);
    let tip = sb.git(&clone, &["rev-parse", &branch]);
    assert_eq!(branch, format!("worktree-agent-a1-{}", &tip[..12]));
    assert_eq!(sb.git(&clone, &["rev-parse", &format!("{branch}^")]), detached);
    assert_eq!(sb.git(&clone, &["show", &format!("{branch}:loose.txt")]), "loose");

    // A detached HEAD with nothing of its own gets no branch.
    let (path, _) = sb.create(&clone, "agent-b2", &clone);
    sb.git(&path, &["switch", "--quiet", "--detach"]);
    let before = sb.git(&clone, &["for-each-ref", "--format=%(refname)", "refs/heads"]);
    assert_eq!(sb.remove(&path).code, Some(0));
    let after = sb.git(&clone, &["for-each-ref", "--format=%(refname)", "refs/heads"]);
    assert_eq!(before, after);
}

#[test]
fn what_cannot_be_kept_or_is_not_a_worktree_is_left_alone() {
    let sb = Sandbox::new();
    let (clone, path) = sb.clone_with_worktree();

    // A locked worktree stays, though its work is committed.
    fs::write(path.join("work.txt"), "work\n").unwrap();
    sb.git(&clone, &["worktree", "lock", path.to_str().unwrap()]);
    let out = sb.remove(&path);
    assert_eq!(out.code, Some(1), "{out:?}");
    assert!(path.join("work.txt").is_file());
    assert!(sb.lists(&clone, &path));
    assert_eq!(sb.git(&clone, &["show", "worktree-agent-a1:work.txt"]), "work");
    sb.git(&clone, &["worktree", "unlock", path.to_str().unwrap()]);

    // Work git cannot commit, here a repository inside the worktree with no commits, stays.
    let nested = path.join("nested");
    sb.git(&path, &["init", "-q", "nested"]);
    fs::write(nested.join("f"), "f\n").unwrap();
    let out = sb.remove(&path);
    assert_eq!(out.code, Some(1), "{out:?}");
    assert!(nested.join("f").is_file());
    fs::remove_dir_all(&nested).unwrap();

    for (dir, why) in [
        (clone.clone(), "is a repository's main working tree"),
        (path.join("dir"), "is not the top of a git worktree"),
        (sb.home().join("plain"), "is not the top of a git worktree"),
    ] {
        fs::create_dir_all(&dir).unwrap();
        let out = sb.remove(&dir);
        assert_eq!(out.code, Some(1), "{}: {out:?}", dir.display());
        assert!(out.stderr.contains(why), "{out:?}");
        assert!(out.stderr.contains("so it is left in place"), "{out:?}");
        assert!(dir.is_dir());
    }
    assert!(clone.join("README").is_file());

    // Nothing there is nothing to do.
    let out = sb.remove(&sb.home().join("missing"));
    assert_eq!((out.code, out.stdout.as_str()), (Some(0), ""));
    assert!(sb.lists(&clone, &path));
}
