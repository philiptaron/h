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
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use crate::clone::{NO_PUSH, alternate_refs_prefixes};
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
    // `h store worktree` makes clones, but a worktree of the store made by hand (or by h 0.3.0 and
    // earlier) is throwaway too: forget it as soon as it is gone.
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

/// What [`Store::fetch`] could not do; it did everything else.
#[derive(Debug, Default)]
pub struct FetchReport {
    /// How fetching failed, for one upstream or more. Git names each one as it fails.
    pub failed: Option<GitError>,
    /// The nested upstreams whose `<name>/-/HEAD` could not be updated, and why.
    pub heads_failed: Vec<(String, GitError)>,
    /// The nested upstreams with no default branch, whose `<name>/-/HEAD` is left as it was.
    pub headless: Vec<String>,
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
    ///
    /// The store holds SHA-1 objects whatever the default for new repositories is, since a
    /// repository holds a single object format and nearly every upstream is SHA-1. Its refs are
    /// kept in a reftable (git 2.45), which suits a store of many upstreams' refs: names that
    /// differ only in case (`gitlab.com/GNOME/x` beside `gitlab.com/gnome/y`) coexist on macOS,
    /// pruning a ref does not rewrite every other one, and moving many refs at once, as nesting an
    /// upstream does, is atomic.
    pub fn init(&self, git_opts: &[OsString]) -> Result<(), GitError> {
        if !self.exists() {
            if let Some(parent) = self.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let args: Vec<OsString> = vec![
                "init".into(),
                "--bare".into(),
                "--quiet".into(),
                "--object-format=sha1".into(),
                "--ref-format=reftable".into(),
                self.path.clone().into(),
            ];
            git::run(None, &args)?;
        }
        for (key, value) in STORE_CONFIG {
            git::run(self.dir(), &["config", key, value])?;
        }
        // Upstreams added before HEAD was followed follow it from now on.
        for name in self.remotes()? {
            let nested = self.is_nested(&name)?;
            self.follow_remote_head(&name, nested)?;
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

    /// The upstream called `name`, spelled as the store spells it: exactly, or else, on GitHub,
    /// whose names ignore case, in another casing. `None` when the store has no such upstream.
    pub fn find(&self, name: &str) -> Option<String> {
        let existing = self.remotes().ok()?;
        let exact = existing.iter().find(|r| *r == name);
        let github = name.starts_with("github.com/");
        let other_case = || existing.iter().find(|r| github && r.eq_ignore_ascii_case(name));
        exact.or_else(other_case).cloned()
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
        self.follow_remote_head(name, nested)?;
        Ok(true)
    }

    /// Remove upstream `name`: its configuration, and its branches, tags and HEAD, all in one
    /// transaction. The refs of upstreams whose names extend it are left alone, and an outer
    /// upstream it nested stays nested. Its objects stay too, since clones borrow them.
    ///
    /// `git remote remove` would keep the tags, which are not remote-tracking refs, so the refs
    /// are deleted here.
    pub fn remove_remote(&self, name: &str) -> Result<(), GitError> {
        let dir = self.dir();
        let prefix = self.prefix(name)?;
        let inner: Vec<String> = self.remotes()?.into_iter().filter(|r| extends(r, name)).collect();
        let namespaces = [format!("refs/remotes/{prefix}/"), format!("refs/tags/{prefix}/")];
        let mut args = vec!["for-each-ref".to_string(), "--format=%(refname)".to_string()];
        args.extend(namespaces.iter().cloned());
        let mut refs: Vec<String> = git::output(dir, &args)?.lines().map(String::from).collect();
        // A HEAD whose branch is gone is a broken ref, which for-each-ref leaves out.
        let head = format!("refs/remotes/{prefix}/HEAD");
        if !refs.contains(&head) && git::output(dir, &["symbolic-ref", "--quiet", &head]).is_ok() {
            refs.push(head);
        }
        let inner_ref = |refname: &str| {
            inner.iter().any(|r| {
                refname.starts_with(&format!("refs/remotes/{r}/"))
                    || refname.starts_with(&format!("refs/tags/{r}/"))
            })
        };
        let input: String = refs
            .iter()
            .filter(|refname| !inner_ref(refname))
            .map(|refname| format!("delete {refname}\n"))
            .collect();
        if !input.is_empty() {
            git::run_with_input(dir, &["update-ref", "--no-deref", "--stdin"], input.as_bytes())?;
        }
        git::run(dir, &["config", "--remove-section", &format!("remote.{name}")])
    }

    /// Have each fetch of upstream `name` point `<name>/HEAD` at its default branch, even when
    /// that changes, as git leaves an existing HEAD alone by default. A nested upstream's HEAD is
    /// left to [`Store::set_head`]: git would look for the default branch directly under
    /// `refs/remotes/<name>/`, where only an inner upstream's refs are.
    fn follow_remote_head(&self, name: &str, nested: bool) -> Result<(), GitError> {
        let key = format!("remote.{name}.followRemoteHEAD");
        git::run(self.dir(), &["config", &key, if nested { "never" } else { "always" }])
    }

    /// Whether upstream `name` keeps its refs under `<name>/-/`.
    pub fn is_nested(&self, name: &str) -> Result<bool, GitError> {
        let key = format!("remote.{name}.fetch");
        let configured = git::output(self.dir(), &["config", "--get-all", &key])?;
        let [nested_heads, _] = refspecs(name, true);
        Ok(configured.lines().any(|r| r == nested_heads))
    }

    /// Whether upstream `name` has any branch or tag, as one that was ever fetched does.
    pub fn has_refs(&self, name: &str) -> Result<bool, GitError> {
        let prefix = self.prefix(name)?;
        let (branches, tags) = (format!("refs/remotes/{prefix}/"), format!("refs/tags/{prefix}/"));
        let args = ["for-each-ref", "--count=1", "--format=x", &branches, &tags];
        Ok(!git::output(self.dir(), &args)?.is_empty())
    }

    /// What `name` is called in revisions: `<name>`, or `<name>/-` when it is nested.
    fn prefix(&self, name: &str) -> Result<String, GitError> {
        Ok(if self.is_nested(name)? { format!("{name}/-") } else { name.to_string() })
    }

    /// Point `<name>/-/HEAD` of a nested upstream at the branch the upstream's HEAD names, as
    /// `git clone` does for `origin/HEAD`. Git does this only for refs directly under
    /// `refs/remotes/<name>/`, so [`Store::fetch`] does it here, after every fetch. Returns
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
        self.follow_remote_head(name, true)
    }

    /// Fetch the named upstreams, or all of them when `names` is empty, then point the
    /// `<name>/-/HEAD` of each nested one at its default branch. This is best-effort: an
    /// upstream that cannot be fetched, or whose HEAD cannot be read, keeps none of the others
    /// from being brought up to date, and the report says what failed.
    ///
    /// Tags are never pruned, even in stores made before [`STORE_CONFIG`] said so: git passes
    /// `--no-prune-tags` on to the fetch of each upstream.
    pub fn fetch(&self, names: &[String], quiet: bool) -> Result<FetchReport, GitError> {
        let mut args = vec!["fetch", "--prune", "--no-prune-tags", "--no-write-fetch-head"];
        if quiet {
            args.push("--quiet");
        }
        if names.is_empty() {
            args.push("--all");
        } else {
            args.push("--multiple");
            args.extend(names.iter().map(String::as_str));
        }
        // Git fetches every upstream it can, names those it cannot, and then fails.
        let mut report =
            FetchReport { failed: git::run(self.dir(), &args).err(), ..Default::default() };
        let fetched = if names.is_empty() { self.remotes()? } else { names.to_vec() };
        for name in fetched {
            match self.is_nested(&name).and_then(|nested| Ok(nested && !self.set_head(&name)?)) {
                Ok(true) => report.headless.push(name),
                Ok(false) => {}
                Err(err) => report.heads_failed.push((name, err)),
            }
        }
        Ok(report)
    }

    /// `reference` of upstream `name` as git should be given it: `<name>/<reference>` (or
    /// `<name>/-/<reference>`, when nested) when that names something, and otherwise `reference`
    /// itself if it is a commit hash, as `1f0e2d3` and `1f0e2d3~2` are. Any other reference stays
    /// qualified, so one the upstream lacks fails rather than naming something else in the
    /// store, as `HEAD` would name the store's own.
    fn revision(&self, name: &str, reference: &str) -> Result<String, GitError> {
        let qualified = format!("{}/{reference}", self.prefix(name)?);
        let object = format!("{qualified}^{{object}}");
        let found = git::output(self.dir(), &["rev-parse", "--verify", "--quiet", &object]).is_ok();
        Ok(if found || !is_object_name(reference) { qualified } else { reference.to_string() })
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

    /// Clone upstream `name` from the store into `dir`, or a fresh temporary directory, and check
    /// out `reference` there. Returns the clone's path.
    ///
    /// The clone borrows the store's objects, as `git clone --shared` would, but sees only this
    /// upstream, and under the names an ordinary clone of it has: its branches as `origin/*`,
    /// `origin/HEAD` at its default branch, and its tags as plain tags. So `git describe` and
    /// build tooling behave as in any clone, and `git fetch` refreshes it from the store with no
    /// network. Pushing back fails. A branch (or `HEAD`, the default branch) is checked out as a
    /// local branch tracking `origin/<branch>`, as `git clone --branch` does, so work in the clone
    /// can be committed; a tag or commit is checked out detached. The `-c key=value` pairs in
    /// `git_opts` are written into it, as `h go` writes them into clones.
    ///
    /// `git clone --shared` itself is not used: it would also take the store's own HEAD and
    /// branches, and every upstream's tags. Anything at `dir` but an empty directory is refused,
    /// and a failure removes whatever was made, so `dir` ends up a whole clone or as it was.
    pub fn worktree(
        &self,
        name: &str,
        reference: &str,
        dir: Option<PathBuf>,
        git_opts: &[OsString],
    ) -> Result<PathBuf, GitError> {
        let prefix = self.prefix(name)?;
        let start = self.checkout_start(name, &prefix, reference)?;
        let dir = dir.unwrap_or_else(|| temp_worktree_dir(name, reference));
        let existed = dir.exists();
        if existed && !dir.read_dir().is_ok_and(|mut entries| entries.next().is_none()) {
            let msg = format!("{} already exists and is not an empty directory", dir.display());
            return Err(GitError::Invalid(msg));
        }
        let result = self.fill_checkout(name, &prefix, &start, &dir, git_opts);
        if result.is_err() {
            remove_checkout(&dir, existed);
        }
        result.map(|()| dir)
    }

    /// Where a checkout of `reference` of upstream `name` (called `prefix` in refs) starts:
    /// a branch of the upstream, or else the commit that `reference` names, such as a tag, a
    /// hash or `main~2`. Checked before anything is made, so a bad reference leaves nothing.
    fn checkout_start(&self, name: &str, prefix: &str, reference: &str) -> Result<Start, GitError> {
        let dir = self.dir();
        let branches = format!("refs/remotes/{prefix}/");
        let branch = if reference == "HEAD" {
            let head = format!("{branches}HEAD");
            git::output(dir, &["symbolic-ref", "--quiet", &head])
                .ok()
                .and_then(|target| target.trim().strip_prefix(&branches).map(String::from))
        } else {
            let exact = format!("{branches}{reference}");
            git::output(dir, &["show-ref", "--verify", "--quiet", &exact])
                .ok()
                .map(|_| reference.to_string())
        };
        if let Some(branch) = branch {
            return Ok(Start::Branch(branch));
        }
        let commit = format!("{}^{{commit}}", self.revision(name, reference)?);
        match git::output(dir, &["rev-parse", "--verify", "--quiet", &commit]) {
            Ok(oid) => Ok(Start::Detached(oid.trim().to_string())),
            Err(_) => {
                let msg = format!("{reference} is not a branch, tag or commit of {name}");
                Err(GitError::Invalid(msg))
            }
        }
    }

    /// Make the clone described at [`Store::worktree`] in `dir`.
    fn fill_checkout(
        &self,
        name: &str,
        prefix: &str,
        start: &Start,
        dir: &Path,
        git_opts: &[OsString],
    ) -> Result<(), GitError> {
        let format = git::output(self.dir(), &["rev-parse", "--show-object-format"])?;
        let format = format!("--object-format={}", format.trim());
        std::fs::create_dir_all(dir).map_err(GitError::Spawn)?;
        let init: Vec<OsString> =
            vec!["init".into(), "--quiet".into(), format.into(), dir.as_os_str().into()];
        git::run(None, &init)?;
        let mut alternates = self.path.join("objects").into_os_string().into_vec();
        alternates.push(b'\n');
        std::fs::write(dir.join(".git/objects/info/alternates"), alternates)
            .map_err(GitError::Spawn)?;

        let clone = Some(dir);
        let store: OsString = self.path.clone().into();
        git::run(clone, &[OsString::from("config"), "remote.origin.url".into(), store])?;
        let fetch = [
            format!("+refs/remotes/{prefix}/*:refs/remotes/origin/*"),
            // Leave out the store's `<prefix>/HEAD`: origin/HEAD is pointed at the default
            // branch below, as a symbolic ref, and no fetch should write to it.
            format!("^refs/remotes/{prefix}/HEAD"),
            format!("+refs/tags/{prefix}/*:refs/tags/*"),
        ];
        for refspec in &fetch {
            git::run(clone, &["config", "--add", "remote.origin.fetch", refspec])?;
        }
        for (key, value) in [
            ("remote.origin.pushurl", NO_PUSH),
            // Following tags would bring the store's names for them, `<prefix>/v1.0`.
            ("remote.origin.tagOpt", "--no-tags"),
            // The store's own HEAD says nothing about the upstream's default branch.
            ("remote.origin.followRemoteHEAD", "never"),
        ] {
            git::run(clone, &["config", key, value])?;
        }
        if let Some(prefixes) = alternate_refs_prefixes(&[name.to_string()]) {
            git::run(clone, &["config", "core.alternateRefsPrefixes", &prefixes])?;
        }
        for (key, value) in config_pairs(git_opts) {
            git::run(clone, &[OsString::from("config"), key, value])?;
        }
        git::run(clone, &["fetch", "--quiet", "origin"])?;

        let head = format!("refs/remotes/{prefix}/HEAD");
        if let Ok(target) = git::output(self.dir(), &["symbolic-ref", "--quiet", &head])
            && let Some(branch) = target.trim().strip_prefix(&format!("refs/remotes/{prefix}/"))
        {
            let origin_head = format!("refs/remotes/origin/{branch}");
            git::run(clone, &["symbolic-ref", "refs/remotes/origin/HEAD", &origin_head])?;
        }
        match start {
            Start::Branch(branch) => {
                let upstream = format!("origin/{branch}");
                git::run(clone, &["switch", "--quiet", "--track", "--create", branch, &upstream])
            }
            Start::Detached(oid) => git::run(clone, &["switch", "--quiet", "--detach", oid]),
        }
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

/// Remove what a failed [`Store::worktree`] made in `dir`: all of it, or only what is inside
/// when the (empty) directory was there before.
fn remove_checkout(dir: &Path, existed: bool) {
    if !existed {
        let _ = std::fs::remove_dir_all(dir);
        return;
    }
    for entry in dir.read_dir().into_iter().flatten().flatten() {
        let path = entry.path();
        let _ = match entry.file_type() {
            Ok(kind) if kind.is_dir() => std::fs::remove_dir_all(&path),
            _ => std::fs::remove_file(&path),
        };
    }
}

/// Where [`Store::worktree`] starts the clone it makes.
enum Start {
    /// A local branch of this name, tracking `origin/<name>`.
    Branch(String),
    /// A detached HEAD at this commit.
    Detached(String),
}

/// Whether `reference` is a full or abbreviated object name, perhaps followed by a suffix such
/// as `~2` or `^{tree}`.
fn is_object_name(reference: &str) -> bool {
    let name = reference.split(['~', '^']).next().unwrap_or(reference);
    (4..=64).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_hexdigit())
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
    fn recognizes_object_names() {
        for name in ["1f0e2d3", "1F0E2D3", "cafe", "1f0e2d3~2", "1f0e2d3^{tree}", &"a".repeat(40)] {
            assert!(is_object_name(name), "{name}");
        }
        for name in ["HEAD", "main", "abc", "1f0e2d3x", "v1.0", "", "~1", &"a".repeat(65)] {
            assert!(!is_object_name(name), "{name}");
        }
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
