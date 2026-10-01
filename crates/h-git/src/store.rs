//! A shared object store: one bare repository holding many upstream repositories as remotes.
//!
//! The store is read-only and kept fresh by fetching. Each upstream is a remote named by the
//! path `h` would clone it to (`github.com/NixOS/nixpkgs`), with its branches under
//! `refs/remotes/<name>/` and its tags under `refs/tags/<name>/`, so unrelated projects never
//! collide and `<name>/<branch>` and `<name>/<tag>` both resolve. An upstream whose name another
//! one extends, as `gitlab.com/g/proj` is extended by `gitlab.com/g/proj/sub`, is nested: its
//! refs live under `<name>/-/` instead, so pruning it never touches the other's refs. No name has
//! a `-` segment, so the two layouts never overlap. Pushing to any remote fails, and nothing is
//! ever pruned, so clones that borrow objects from the store stay intact.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::clone::NO_PUSH;
use crate::git::{self, GitError, config_pairs};

/// Configuration every store gets at creation.
pub const STORE_CONFIG: &[(&str, &str)] = &[
    // Never delete objects: clones borrow them through alternates.
    ("gc.auto", "0"),
    ("gc.pruneExpire", "never"),
    ("gc.cruftPacks", "true"),
    ("maintenance.auto", "false"),
    ("maintenance.gc.enabled", "false"),
    ("maintenance.strategy", "incremental"),
    // Fetching is done by `h store fetch`, which also updates tags; prefetch does neither.
    ("maintenance.prefetch.enabled", "false"),
    ("fetch.prune", "true"),
    // Pruning tags adds `refs/tags/*:refs/tags/*` to every fetch, which would delete the other
    // upstreams' tags and write this one's outside its namespace. A global `fetch.pruneTags`
    // must not reach the store.
    ("fetch.pruneTags", "false"),
    ("fetch.parallel", "4"),
    // Every upstream's commits are offered to every fetch, and git offers all of them until the
    // server recognizes one, which an unrelated upstream never does. Skipping offers a few.
    ("fetch.negotiationAlgorithm", "skipping"),
    // Worktrees made from the store are throwaway: forget them as soon as they are gone.
    ("gc.worktreePruneExpire", "now"),
];

/// The maintenance tasks for each schedule. None of them deletes objects.
pub fn maintenance_tasks(schedule: &str) -> Option<&'static [&'static str]> {
    match schedule {
        "hourly" => Some(&["commit-graph"]),
        "daily" => Some(&["commit-graph", "loose-objects", "incremental-repack", "worktree-prune"]),
        "weekly" => Some(&[
            "commit-graph",
            "loose-objects",
            "incremental-repack",
            "worktree-prune",
            "pack-refs",
        ]),
        _ => None,
    }
}

/// A store on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    pub path: PathBuf,
}

impl Store {
    pub fn new(path: impl Into<PathBuf>) -> Store {
        Store { path: path.into() }
    }

    /// Whether the store has been created.
    pub fn exists(&self) -> bool {
        self.path.join("objects").is_dir() && self.path.join("HEAD").is_file()
    }

    fn dir(&self) -> Option<&Path> {
        Some(&self.path)
    }

    /// Create the store, applying [`STORE_CONFIG`] and the `-c key=value` pairs in `git_opts`
    /// (the credentials to fetch private repositories with). Creating an existing store only
    /// reapplies the configuration.
    pub fn init(&self, git_opts: &[OsString]) -> Result<(), GitError> {
        if !self.exists() {
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let args: Vec<OsString> =
                vec!["init".into(), "--bare".into(), "--quiet".into(), self.path.clone().into()];
            git::run(None, &args)?;
        }
        for (key, value) in STORE_CONFIG {
            git::run(self.dir(), &["config", key, value])?;
        }
        for (key, value) in config_pairs(git_opts) {
            git::run(self.dir(), &[OsString::from("config"), key, value])?;
        }
        Ok(())
    }

    /// The names of the upstreams in the store.
    pub fn remotes(&self) -> Result<Vec<String>, GitError> {
        let out = git::output(self.dir(), &["remote"])?;
        Ok(out.lines().map(String::from).collect())
    }

    /// Add the upstream `name` at `url`, or leave it alone if it is already there.
    ///
    /// An upstream whose name another one extends (`g/proj` beside `g/proj/sub`) is nested: its
    /// refs move under `<name>/-/`, out of the way of the other's. Adding the inner one nests the
    /// outer one first. The remote is written into the configuration directly, since `git remote
    /// add` refuses a name that extends another's.
    pub fn add_remote(&self, name: &str, url: &str) -> Result<bool, GitError> {
        let remotes = self.remotes()?;
        if remotes.iter().any(|r| r == name) {
            return Ok(false);
        }
        for outer in remotes.iter().filter(|r| extends(name, r)) {
            if !self.is_nested(outer)? {
                self.nest(outer, &remotes)?;
            }
        }
        let nested = remotes.iter().any(|r| extends(r, name));
        let dir = self.dir();
        let key = |k: &str| format!("remote.{name}.{k}");
        git::run(dir, &["config", &key("url"), url])?;
        git::run(dir, &["config", &key("pushurl"), NO_PUSH])?;
        git::run(dir, &["config", &key("tagOpt"), "--no-tags"])?;
        for refspec in refspecs(name, nested) {
            git::run(dir, &["config", "--add", &key("fetch"), &refspec])?;
        }
        Ok(true)
    }

    /// Whether upstream `name` keeps its refs under `<name>/-/`.
    pub fn is_nested(&self, name: &str) -> Result<bool, GitError> {
        let key = format!("remote.{name}.fetch");
        let configured = git::output(self.dir(), &["config", "--get-all", &key])?;
        let [nested_heads, _] = refspecs(name, true);
        Ok(configured.lines().any(|r| r == nested_heads))
    }

    /// What `name` is called in revisions: `<name>`, or `<name>/-` when it is nested.
    fn prefix(&self, name: &str) -> Result<String, GitError> {
        Ok(if self.is_nested(name)? { format!("{name}/-") } else { name.to_string() })
    }

    /// Point `<name>/-/HEAD` of a nested upstream at the branch the upstream's HEAD names, as
    /// `git clone` does for `origin/HEAD`. Fetching does this only for refs directly under
    /// `refs/remotes/<name>/`, so it is done here, once, when the upstream is added. Returns
    /// whether HEAD was set.
    pub fn set_head(&self, name: &str) -> Result<bool, GitError> {
        let out = git::output(self.dir(), &["ls-remote", "--symref", name, "HEAD"])?;
        let Some(branch) = out
            .lines()
            .find_map(|line| line.strip_prefix("ref: refs/heads/")?.strip_suffix("\tHEAD"))
        else {
            return Ok(false);
        };
        let target = format!("refs/remotes/{name}/-/{branch}");
        if git::output(self.dir(), &["rev-parse", "--verify", "--quiet", &target]).is_err() {
            return Ok(false);
        }
        let head = format!("refs/remotes/{name}/-/HEAD");
        git::run(self.dir(), &["symbolic-ref", &head, &target])?;
        Ok(true)
    }

    /// Move the refs of upstream `name` from directly under `refs/remotes/<name>/` and
    /// `refs/tags/<name>/` to under `<name>/-/`, and fetch it there from now on. The refs of the
    /// other `remotes` under those directories are left alone. No objects are touched, so nothing
    /// needs fetching again.
    fn nest(&self, name: &str, remotes: &[String]) -> Result<(), GitError> {
        let dir = self.dir();
        let branches = format!("refs/remotes/{name}/");
        let tags = format!("refs/tags/{name}/");
        let inner: Vec<String> = remotes.iter().filter(|r| extends(r, name)).cloned().collect();
        let format = "--format=%(refname)%00%(objectname)%00%(symref)";
        let refs = git::output(dir, &["for-each-ref", format, &branches, &tags])?;
        let mut input = String::new();
        let mut heads = Vec::new();
        for line in refs.lines() {
            let mut fields = line.split('\0');
            let (Some(refname), Some(oid), Some(symref)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let Some((prefix, rest)) = [&branches, &tags]
                .into_iter()
                .find_map(|p| Some((p, refname.strip_prefix(p.as_str())?)))
            else {
                continue;
            };
            let inner_ref = |r: &String| {
                refname.starts_with(&format!("refs/remotes/{r}/"))
                    || refname.starts_with(&format!("refs/tags/{r}/"))
            };
            if inner.iter().any(inner_ref) {
                continue;
            }
            if symref.is_empty() {
                input.push_str(&format!("create {prefix}-/{rest} {oid}\ndelete {refname} {oid}\n"));
            } else {
                input.push_str(&format!("delete {refname}\n"));
                if let Some(target) = symref.strip_prefix(prefix.as_str()) {
                    heads.push((format!("{prefix}-/{rest}"), format!("{prefix}-/{target}")));
                }
            }
        }
        if !input.is_empty() {
            git::run_with_input(dir, &["update-ref", "--no-deref", "--stdin"], input.as_bytes())?;
        }
        for (head, target) in heads {
            git::run(dir, &["symbolic-ref", &head, &target])?;
        }
        let key = format!("remote.{name}.fetch");
        git::run(dir, &["config", "--unset-all", &key])?;
        for refspec in refspecs(name, true) {
            git::run(dir, &["config", "--add", &key, &refspec])?;
        }
        Ok(())
    }

    /// Fetch the named upstreams, or all of them when `names` is empty.
    ///
    /// Tags are never pruned, even in stores made before [`STORE_CONFIG`] said so: git passes
    /// `--no-prune-tags` on to the fetch of each upstream.
    pub fn fetch(&self, names: &[String], quiet: bool) -> Result<(), GitError> {
        let mut args = vec!["fetch", "--prune", "--no-prune-tags", "--no-write-fetch-head"];
        if quiet {
            args.push("--quiet");
        }
        if names.is_empty() {
            args.push("--all");
            git::run(self.dir(), &args)
        } else {
            args.push("--multiple");
            args.extend(names.iter().map(String::as_str));
            git::run(self.dir(), &args)
        }
    }

    /// `reference` of upstream `name` as git should be given it: `<name>/<reference>` (or
    /// `<name>/-/<reference>`, when nested) when that names something, and otherwise `reference`
    /// itself, such as a commit hash.
    fn revision(&self, name: &str, reference: &str) -> Result<String, GitError> {
        let qualified = format!("{}/{reference}", self.prefix(name)?);
        let object = format!("{qualified}^{{object}}");
        Ok(match git::output(self.dir(), &["rev-parse", "--verify", "--quiet", &object]) {
            Ok(_) => qualified,
            Err(_) => reference.to_string(),
        })
    }

    /// Show `spec` (a `<ref>` or `<ref>:<path>`) of upstream `name`, as `git show` prints it.
    pub fn show(&self, name: &str, spec: &str) -> Result<(), GitError> {
        let (reference, path) = match spec.find(':') {
            Some(i) if i > 0 => spec.split_at(i),
            _ => (spec, ""),
        };
        let spec = format!("{}{path}", self.revision(name, reference)?);
        git::passthrough(self.dir(), &["show", &spec])
    }

    /// Check out `reference` of upstream `name` into a detached worktree at `dir`, or a fresh
    /// temporary directory. Returns the worktree's path.
    pub fn worktree(
        &self,
        name: &str,
        reference: &str,
        dir: Option<PathBuf>,
    ) -> Result<PathBuf, GitError> {
        let dir = dir.unwrap_or_else(|| temp_worktree_dir(name, reference));
        if let Some(parent) = dir.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let commit = self.revision(name, reference)?;
        let args: Vec<OsString> = vec![
            "worktree".into(),
            "add".into(),
            "--detach".into(),
            "--quiet".into(),
            dir.clone().into(),
            commit.into(),
        ];
        git::run(self.dir(), &args)?;
        Ok(dir)
    }

    /// Run the maintenance tasks for `schedule` (`hourly`, `daily` or `weekly`).
    pub fn maintain(&self, schedule: &str) -> Result<(), GitError> {
        let Some(tasks) = maintenance_tasks(schedule) else {
            let msg = format!("Unknown schedule {schedule}: use hourly, daily or weekly");
            return Err(GitError::Invalid(msg));
        };
        let mut args = vec!["maintenance".to_string(), "run".to_string(), "--quiet".to_string()];
        args.extend(tasks.iter().map(|task| format!("--task={task}")));
        git::run(self.dir(), &args)
    }
}

/// Whether the name `inner` extends the name `outer` by one or more segments.
fn extends(inner: &str, outer: &str) -> bool {
    inner.strip_prefix(outer).is_some_and(|rest| rest.starts_with('/'))
}

/// The fetch refspecs of upstream `name`: its branches and tags, directly under `<name>/`, or
/// under `<name>/-/` when it is nested. No name has a `-` segment, so the two never overlap.
fn refspecs(name: &str, nested: bool) -> [String; 2] {
    let sep = if nested { "/-" } else { "" };
    [
        format!("+refs/heads/*:refs/remotes/{name}{sep}/*"),
        format!("+refs/tags/*:refs/tags/{name}{sep}/*"),
    ]
}

/// A directory for a throwaway worktree of `name` at `reference`, under the temporary directory.
fn temp_worktree_dir(name: &str, reference: &str) -> PathBuf {
    let repo = name.rsplit('/').next().unwrap_or(name);
    let reference: String =
        reference.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let unique = format!("{repo}-{reference}-{}-{nanos:x}", std::process::id());
    std::env::temp_dir().join("h-worktrees").join(unique)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_never_include_gc() {
        for schedule in ["hourly", "daily", "weekly"] {
            let tasks = maintenance_tasks(schedule).unwrap();
            assert!(!tasks.contains(&"gc"), "{schedule}: {tasks:?}");
            assert!(!tasks.contains(&"prefetch"), "{schedule}: {tasks:?}");
        }
        assert_eq!(maintenance_tasks("monthly"), None);
    }

    #[test]
    fn names_extend_by_whole_segments() {
        assert!(extends("g/proj/sub", "g/proj"));
        assert!(extends("g/proj/a/b", "g/proj"));
        assert!(!extends("g/proj2", "g/proj"));
        assert!(!extends("g/proj", "g/proj"));
        assert!(!extends("g/proj", "g/proj/sub"));
    }

    #[test]
    fn temp_worktree_dirs_are_unique_and_safe() {
        let a = temp_worktree_dir("github.com/NixOS/nixpkgs", "release-26.05");
        let b = temp_worktree_dir("github.com/NixOS/nixpkgs", "release-26.05");
        assert_ne!(a, b);
        let name = a.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("nixpkgs-release-26-05-"), "{name}");
        assert_eq!(a.parent().unwrap(), std::env::temp_dir().join("h-worktrees"));
    }
}
