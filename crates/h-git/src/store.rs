//! A shared object store: one bare repository holding many upstream repositories as remotes.
//!
//! The store is read-only and kept fresh by fetching. Each upstream is a remote named by the
//! path `h` would clone it to (`github.com/NixOS/nixpkgs`), with its branches under
//! `refs/remotes/<name>/` and its tags under `refs/tags/<name>/`, so unrelated projects never
//! collide and `<name>/<branch>` and `<name>/<tag>` both resolve. Pushing to any remote fails,
//! and nothing is ever pruned, so clones that borrow objects from the store stay intact.

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
    ("fetch.parallel", "4"),
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
    pub fn add_remote(&self, name: &str, url: &str) -> Result<bool, GitError> {
        if self.remotes()?.iter().any(|r| r == name) {
            return Ok(false);
        }
        let dir = self.dir();
        git::run(dir, &["remote", "add", name, url])?;
        git::run(dir, &["config", &format!("remote.{name}.pushurl"), NO_PUSH])?;
        git::run(dir, &["config", &format!("remote.{name}.tagOpt"), "--no-tags"])?;
        let tags = format!("+refs/tags/*:refs/tags/{name}/*");
        git::run(dir, &["config", "--add", &format!("remote.{name}.fetch"), &tags])?;
        Ok(true)
    }

    /// Fetch the named upstreams, or all of them when `names` is empty.
    pub fn fetch(&self, names: &[String], quiet: bool) -> Result<(), GitError> {
        let mut args = vec!["fetch", "--prune", "--no-write-fetch-head"];
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

    /// `reference` of upstream `name` as git should be given it: `<name>/<reference>` when that
    /// names something, and otherwise `reference` itself, such as a commit hash.
    fn revision(&self, name: &str, reference: &str) -> String {
        let qualified = format!("{name}/{reference}");
        let object = format!("{qualified}^{{object}}");
        match git::output(self.dir(), &["rev-parse", "--verify", "--quiet", &object]) {
            Ok(_) => qualified,
            Err(_) => reference.to_string(),
        }
    }

    /// Show `spec` (a `<ref>` or `<ref>:<path>`) of upstream `name`, as `git show` prints it.
    pub fn show(&self, name: &str, spec: &str) -> Result<(), GitError> {
        let (reference, path) = match spec.find(':') {
            Some(i) if i > 0 => spec.split_at(i),
            _ => (spec, ""),
        };
        let spec = format!("{}{path}", self.revision(name, reference));
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
        let commit = self.revision(name, reference);
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
    fn temp_worktree_dirs_are_unique_and_safe() {
        let a = temp_worktree_dir("github.com/NixOS/nixpkgs", "release-26.05");
        let b = temp_worktree_dir("github.com/NixOS/nixpkgs", "release-26.05");
        assert_ne!(a, b);
        let name = a.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("nixpkgs-release-26-05-"), "{name}");
        assert_eq!(a.parent().unwrap(), std::env::temp_dir().join("h-worktrees"));
    }
}
