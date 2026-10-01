//! Claude Code's WorktreeCreate and WorktreeRemove hooks: the worktrees that agents and `claude
//! --worktree` work in, made in the user's own clone and started from the store's copy of the
//! upstream's default branch, without the network.
//!
//! Claude Code hands each hook a JSON object on stdin. WorktreeCreate's has `name` (the
//! worktree's slug, already checked by Claude Code) and `cwd`, and Claude Code takes the last line
//! the hook prints on stdout as the worktree's absolute path. WorktreeRemove's has
//! `worktree_path`. Claude Code sends it when an agent finishes and `git status` in its worktree
//! is clean, whatever commits the worktree's branch has, and when the user discards a session
//! outright, changes and all. Either way the work is kept: removing a worktree commits what is
//! left in it to its branch first, and never deletes a branch.

use std::path::{Path, PathBuf};

use crate::clone::BARE_DIR;
use crate::git::{self, GitError};
use crate::ingest;
use crate::resolve::{Target, parse_term, remote_name};
use crate::store::Store;

/// Where Claude Code keeps worktrees, relative to the repository: the same place as its own.
pub const WORKTREES_DIR: &str = ".claude/worktrees";

/// A worktree made by [`create`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub path: PathBuf,
    pub branch: String,
    /// What the branch started from, for people reading the hook's stderr.
    pub base: String,
}

/// What [`remove`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Removed {
    /// There was nothing at the path.
    Missing,
    /// The worktree is gone. Its work is on `branch`, when it had a branch or commits of its own,
    /// and `saved` says whether uncommitted work had to be committed there first.
    Removed { branch: Option<String>, saved: bool },
}

/// One field of the hook's JSON input.
fn field(json: &serde_json::Value, key: &str) -> Result<String, GitError> {
    json.get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(String::from)
        .ok_or_else(|| GitError::Invalid(format!("hook input has no {key}")))
}

fn parse(input: &str) -> Result<serde_json::Value, GitError> {
    serde_json::from_str(input).map_err(|err| GitError::Invalid(format!("hook input: {err}")))
}

/// Check `name` as Claude Code does before it runs the hook: at most 64 characters, in
/// `/`-separated segments of letters, digits, `.`, `_` and `-`, none of them `.`, `..` or `.git`.
fn check_name(name: &str) -> Result<(), GitError> {
    let invalid =
        |why: &str| Err(GitError::Invalid(format!("Invalid worktree name {name}: {why}")));
    if name.len() > 64 {
        return invalid("longer than 64 characters");
    }
    for segment in name.split('/') {
        let ok = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
        if segment.is_empty() || !segment.chars().all(ok) {
            return invalid("segments may only have letters, digits, '.', '_' and '-'");
        }
        if segment == "." || segment == ".." {
            return invalid("'.' and '..' are not allowed");
        }
        if segment.trim_end_matches('.').eq_ignore_ascii_case(".git") {
            return invalid("'.git' is reserved");
        }
    }
    Ok(())
}

/// The worktree's directory name and branch for `name`, as Claude Code makes them itself:
/// `/` becomes `+`, and the branch is `worktree-<that>`.
fn slug(name: &str) -> (String, String) {
    let slug = name.replace('/', "+");
    let branch = format!("worktree-{slug}");
    (slug, branch)
}

/// The output of `git` in `dir`, trimmed, or `None` when it fails.
pub(crate) fn query(dir: &Path, args: &[&str]) -> Option<String> {
    let out = git::output(Some(dir), args).ok()?;
    Some(out.trim().to_string()).filter(|out| !out.is_empty())
}

/// The commit `rev` names in `dir`, if it names one.
fn commit(dir: &Path, rev: &str) -> Option<String> {
    query(dir, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
}

/// The repository `dir` belongs to: its common git directory, which is the main repository's
/// even from inside one of its worktrees.
pub(crate) fn common_dir(dir: &Path) -> Result<PathBuf, GitError> {
    query(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .map(PathBuf::from)
        .ok_or_else(|| GitError::Invalid(format!("{} is not in a git repository", dir.display())))
}

/// The directory that holds the repository whose common git directory is `common`: the clone
/// for `<clone>/.git`, the container for `<container>/.bare`, and a bare repository itself.
pub(crate) fn repository_root(common: &Path) -> PathBuf {
    let parent = common.parent().unwrap_or(common);
    match common.file_name().and_then(|name| name.to_str()) {
        Some(".git") => parent.to_path_buf(),
        Some(BARE_DIR) if parent.join(".git").is_file() => parent.to_path_buf(),
        _ => common.to_path_buf(),
    }
}

/// The store the repository at `common` borrows objects from: the one its
/// `objects/info/alternates` names, which is the store the clone was made with, else `fallback`.
pub(crate) fn borrowed_store(common: &Path, fallback: Option<&Store>) -> Option<Store> {
    let objects = common.join("objects");
    let alternates = std::fs::read_to_string(objects.join("info/alternates")).unwrap_or_default();
    let named = alternates
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let path = objects.join(line);
            let store = path.parent()?;
            (path.file_name()? == "objects").then(|| Store::new(store))
        })
        .find(Store::exists);
    named.or_else(|| fallback.filter(|store| store.exists()).cloned())
}

/// The remotes that matter for a base: `upstream`, for a fork, and the clone's own (`origin`, or
/// its only other remote).
pub(crate) fn remotes(root: &Path) -> (Option<String>, Option<String>) {
    let all: Vec<String> =
        query(root, &["remote"]).unwrap_or_default().lines().map(String::from).collect();
    let upstream = all.iter().find(|r| *r == "upstream").cloned();
    let others: Vec<&String> = all.iter().filter(|r| *r != "upstream").collect();
    let own = match others.as_slice() {
        [one] => Some((*one).clone()),
        _ => others.iter().find(|r| **r == "origin").map(|r| (*r).clone()),
    };
    (upstream, own)
}

/// The store's name for the repository at `url`, spelled as the store spells it.
fn store_name(store: &Store, url: &str) -> Option<String> {
    let name = match parse_term(url).ok()? {
        Target::GitHub { user, repo } => remote_name("github.com", &format!("{user}/{repo}")),
        Target::Remote { host, path, .. } => remote_name(&host, &path),
        Target::Name(_) => return None,
    };
    store.find(&name)
}

/// The commit a new worktree of the repository at `root` starts from, and how it was found.
///
/// That is the store's copy of the upstream's default branch, as of the last fetch into the store,
/// when the clone's upstream (a fork's parent, else its own remote) is in the store and the commit
/// is there for the clone through its alternates. Otherwise it is `upstream/HEAD`, then the
/// clone's own `origin/HEAD`, then HEAD. Nothing is fetched.
fn base(
    root: &Path,
    common: &Path,
    fallback: Option<&Store>,
) -> Result<(String, String), GitError> {
    let (upstream, own) = remotes(root);
    if let Some(store) = borrowed_store(common, fallback) {
        let url = upstream
            .iter()
            .chain(&own)
            .find_map(|remote| query(root, &["config", "--get", &format!("remote.{remote}.url")]));
        if let Some(name) = url.and_then(|url| store_name(&store, &url)) {
            let nested = store.is_nested(&name).unwrap_or(false);
            let head = format!("refs/remotes/{name}{}/HEAD", if nested { "/-" } else { "" });
            if let Some(found) = commit(&store.path, &head)
                && commit(root, &found).is_some()
            {
                let from = format!("{name}/HEAD in the store at {}", store.path.display());
                return Ok((found, from));
            }
        }
    }
    let refs = upstream.iter().chain(&own).map(|remote| format!("refs/remotes/{remote}/HEAD"));
    for rev in refs.chain(["HEAD".to_string()]) {
        if let Some(found) = commit(root, &rev) {
            return Ok((found, rev));
        }
    }
    Err(GitError::Invalid(format!("{} has no commit to start a worktree from", root.display())))
}

/// Whether `path` is a worktree of the repository at `root`.
fn is_worktree_of(root: &Path, path: &Path) -> bool {
    let listed = query(root, &["worktree", "list", "--porcelain"]).unwrap_or_default();
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    listed
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .any(|listed| Path::new(listed).canonicalize().is_ok_and(|listed| listed == path))
}

/// WorktreeCreate: add the worktree `name` to the repository the session is in.
///
/// It goes in `<repository>/.claude/worktrees/<name>` on the branch `worktree-<name>`, the names
/// Claude Code uses itself, in the main repository even when the session is in one of its
/// worktrees. The branch starts at [`base`]. A branch of that name that already exists, as one a
/// removed worktree left behind, is checked out again instead, and a worktree already there is
/// used as it is.
pub fn create(input: &str, fallback: Option<&Store>) -> Result<Created, GitError> {
    let json = parse(input)?;
    let name = field(&json, "name")?;
    let cwd = PathBuf::from(field(&json, "cwd")?);
    check_name(&name)?;
    let (slug, branch) = slug(&name);
    let common = common_dir(&cwd)?;
    let root = repository_root(&common);
    let path = root.join(WORKTREES_DIR).join(&slug);

    if path.exists() {
        if is_worktree_of(&root, &path) {
            return Ok(Created { path, branch, base: "the existing worktree".into() });
        }
        let msg = format!("{} exists and is not a worktree of {}", path.display(), root.display());
        return Err(GitError::Invalid(msg));
    }
    let _ = std::fs::create_dir_all(root.join(WORKTREES_DIR));
    let target = path.to_string_lossy().into_owned();
    let created = if commit(&root, &format!("refs/heads/{branch}")).is_some() {
        git::run(Some(&root), &["worktree", "add", "--quiet", &target, &branch])?;
        Created { path, branch, base: "its existing branch".into() }
    } else {
        let (start, from) = base(&root, &common, fallback)?;
        git::run(Some(&root), &["worktree", "add", "--quiet", "-b", &branch, &target, &start])?;
        Created { path, branch, base: from }
    };
    clone_submodules(&created.path, &common, fallback);
    Ok(created)
}

/// Clone the submodules of the new worktree at `path` from the store the repository (whose
/// common git directory is `common`) borrows from, putting those it lacks there too, so that an
/// agent gets a tree it can build. The worktree is usable without them, so what fails is only
/// said on stderr; stdout stays the worktree's path alone.
fn clone_submodules(path: &Path, common: &Path, fallback: Option<&Store>) {
    if !path.join(".gitmodules").is_file() {
        return;
    }
    let Some(store) = borrowed_store(common, fallback) else {
        return;
    };
    match ingest::ingest(&store, path, &[], ingest::Scope::Submodules) {
        Ok(report) => {
            for line in report.problems() {
                eprintln!("warning: {line}");
            }
        }
        Err(err) => eprintln!("warning: could not clone the submodules from the store: {err}"),
    }
}

/// What `git status` reports in the worktree at `path`, untracked files and changes inside
/// submodules included.
fn status(path: &Path) -> Result<String, GitError> {
    let args = ["status", "--porcelain", "--untracked-files=all", "--ignore-submodules=none"];
    git::output(Some(path), &args)
}

/// Commit everything uncommitted in the worktree at `path`, untracked files included, onto
/// whatever HEAD is. Hooks and signing are turned off for this one commit: it only keeps work
/// from being lost, so it must neither fail because a hook objects nor wait for a signing key's
/// passphrase with nobody there to type it. Returns whether there was anything to commit.
fn save(path: &Path) -> Result<bool, GitError> {
    if status(path)?.is_empty() {
        return Ok(false);
    }
    git::run(Some(path), &["add", "--all"])?;
    if git::output(Some(path), &["diff", "--cached", "--quiet"]).is_ok() {
        return Ok(false);
    }
    let message = format!(
        "WIP: what was left uncommitted in {}\n\n\
         Claude Code removed this worktree, so h committed what was still uncommitted in it, \
         untracked files included, rather than lose it.\n",
        path.display()
    );
    let commit = [
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "--no-verify",
        "-m",
        &message,
    ];
    git::run(Some(path), &commit)?;
    Ok(true)
}

/// The branch the worktree at `path` is on. A detached HEAD with commits that no branch, tag or
/// remote-tracking branch has is given a branch of its own first: `worktree-<directory>`, or
/// `worktree-<directory>-<commit>` when a branch of that name already has other work.
fn branch(path: &Path) -> Result<Option<String>, GitError> {
    if let Some(branch) = query(path, &["symbolic-ref", "--quiet", "--short", "HEAD"]) {
        return Ok(Some(branch));
    }
    let Some(head) = commit(path, "HEAD") else {
        return Ok(None);
    };
    let unique =
        ["rev-list", "--max-count=1", "HEAD", "--not", "--branches", "--tags", "--remotes"];
    if query(path, &unique).is_none() {
        return Ok(None);
    }
    let dir = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut name = format!("worktree-{dir}");
    if commit(path, &format!("refs/heads/{name}")).is_some() {
        name = format!("{name}-{}", &head[..head.len().min(12)]);
    }
    git::run(Some(path), &["branch", &name, &head])?;
    Ok(Some(name))
}

/// WorktreeRemove: remove the worktree at `worktree_path`, keeping its work.
///
/// Whatever is uncommitted there, untracked files included, is committed to the worktree's
/// branch, a detached HEAD with commits of its own gets a branch, and only then is the worktree
/// removed, with `git worktree remove --force`. No branch is ever deleted, so [`create`] picks the
/// work up again under the same name. Anything that is not the top of a linked worktree is left
/// alone, and so is a worktree whose work could not all be committed, such as changes inside a
/// submodule, or one that is locked.
pub fn remove(input: &str) -> Result<Removed, GitError> {
    let json = parse(input)?;
    let path = PathBuf::from(field(&json, "worktree_path")?);
    if !path.exists() {
        return Ok(Removed::Missing);
    }
    let leave = |why: &str| {
        let msg = format!("{} {why}, so it is left in place", path.display());
        Err(GitError::Invalid(msg))
    };
    let top = query(&path, &["rev-parse", "--show-toplevel"]).map(PathBuf::from);
    if top.and_then(|top| top.canonicalize().ok()) != path.canonicalize().ok() {
        return leave("is not the top of a git worktree");
    }
    let dirs = ["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"];
    let dirs = query(&path, &dirs).unwrap_or_default();
    let (git_dir, common) = match dirs.lines().collect::<Vec<_>>().as_slice() {
        [git_dir, common] => (PathBuf::from(git_dir), PathBuf::from(common)),
        _ => return leave("has no git directory that git reports"),
    };
    if git_dir == common {
        return leave("is a repository's main working tree, not one of its worktrees");
    }

    let saved = save(&path)?;
    let branch = branch(&path)?;
    let left = status(&path)?;
    if !left.is_empty() {
        return leave(&format!("still has changes that could not be committed:\n{left}"));
    }
    let target = path.to_string_lossy().into_owned();
    git::run(Some(&repository_root(&common)), &["worktree", "remove", "--force", &target])?;
    Ok(Removed::Removed { branch, saved })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked_as_claude_code_checks_them() {
        for ok in ["agent-a0123456789abcdef", "feature/login", "v1.2_x"] {
            assert!(check_name(ok).is_ok(), "{ok}");
        }
        let long = "a".repeat(65);
        for bad in ["", "a//b", "../x", "x/.", "a b", ".git", "x/.GIT..", &long] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn slugs_match_claude_codes_own() {
        let want = ("feature+login".to_string(), "worktree-feature+login".to_string());
        assert_eq!(slug("feature/login"), want);
    }

    #[test]
    fn finds_the_directory_holding_a_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let clone = tmp.path().join("clone");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        assert_eq!(repository_root(&clone.join(".git")), clone);
        let container = tmp.path().join("container");
        std::fs::create_dir_all(container.join(BARE_DIR)).unwrap();
        std::fs::write(container.join(".git"), "gitdir: ./.bare\n").unwrap();
        assert_eq!(repository_root(&container.join(BARE_DIR)), container);
        let bare = tmp.path().join("bare.git");
        assert_eq!(repository_root(&bare), bare);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(create("not json", None).unwrap_err().to_string().starts_with("hook input: "));
        let err = create(r#"{"cwd": "/"}"#, None).unwrap_err();
        assert_eq!(err.to_string(), "hook input has no name");
        let err = create(r#"{"name": "../x", "cwd": "/"}"#, None).unwrap_err();
        assert!(err.to_string().starts_with("Invalid worktree name ../x: "), "{err}");
        let err = remove("{}").unwrap_err();
        assert_eq!(err.to_string(), "hook input has no worktree_path");
        let missing = r#"{"worktree_path": "/nonexistent/h-test-worktree"}"#;
        assert_eq!(remove(missing).unwrap(), Removed::Missing);
    }
}
