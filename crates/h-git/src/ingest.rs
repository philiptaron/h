//! Putting a checkout under the store: `h store ingest`.
//!
//! Ingesting a checkout puts the history of its upstreams, and of its submodules' upstreams at
//! every nesting level, into the store, points it and each of its submodules at the store for
//! their objects, clones any submodules that are missing from the store, and drops the local
//! copies of objects the store now has. A clone made before the store, or a submodule a pull
//! brought in, ends up as if it had been cloned with the store in the first place.
//!
//! History the checkout already has is not downloaded again: an upstream new to the store is
//! fetched first from the checkout itself, its remote-tracking branches and tags (never its local
//! branches), and only then from its URL, which then sends little more than what changed since.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use crate::clone::alternate_refs_prefixes;
use crate::git::{self, GitError};
use crate::hook::{borrowed_store, common_dir, query, remotes, repository_root};
use crate::resolve::store_upstream;
use crate::store::{FetchReport, Store};
use crate::submodules::{self, config_args, negotiation_args};

/// What [`ingest`] covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The repository itself, its upstreams and objects, and its working tree's submodules,
    /// cloning those that are missing: `h store ingest`.
    Everything,
    /// The working tree's submodules, cloning those that are missing, as for a new worktree of
    /// a repository whose own history is in the store already.
    Submodules,
    /// The submodules that are checked out, and no others, as for a new clone, whose options
    /// decided which submodules it has.
    CheckedOut,
}

/// What [`ingest`] did, and what it could not do; it did everything else.
#[derive(Debug, Default)]
pub struct Report {
    /// The upstreams it put in the store.
    pub added: Vec<String>,
    /// The upstreams it put in the store and took out again, because nothing could be fetched
    /// for them, from the checkout or from their URL.
    pub removed_again: Vec<String>,
    /// How fetching into the store went.
    pub fetch: FetchReport,
    /// Everything else that failed, said for people.
    pub failures: Vec<String>,
}

/// One remote of a repository, as an upstream in the store.
#[derive(Debug, Clone)]
struct Source {
    /// The upstream's name, as the store spells it.
    name: String,
    /// The URL the store fetches it from.
    url: String,
    /// The repository's name for the remote, under which its remote-tracking branches are.
    remote: String,
    /// Whether the repository's tags came from this remote, as a clone's come from its own.
    tags: bool,
}

/// A repository whose objects are to come from the store.
#[derive(Debug, Clone)]
struct Repo {
    /// Its common git directory, which holds its objects.
    git_dir: PathBuf,
    /// Whether it has all the history it names, which a shallow or partial clone does not: only
    /// a complete repository is fetched from, and only a complete one gives up objects.
    complete: bool,
    sources: Vec<Source>,
    /// Its remotes the store cannot fetch from, as `(remote, URL)`: a local path, say, or an SSH
    /// host alias such as `me.github.com:owner/repo`.
    unusable: Vec<(String, String)>,
}

/// The repository's own configuration settings for the identity it was cloned with, `user.*`
/// and `credential.*`, as `-c` pairs: what the shell function passes `h`, when nothing does.
pub fn local_identity(dir: &Path) -> Vec<(OsString, OsString)> {
    let pattern = r"^(user|credential)\.";
    let out = git::output(Some(dir), &["config", "--local", "-z", "--get-regexp", pattern]);
    out.unwrap_or_default()
        .split('\0')
        .filter_map(|entry| entry.split_once('\n'))
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
        .collect()
}

/// The store the checkout at `dir` borrows from, else `fallback`, which need not exist yet.
pub fn store_for(dir: &Path, fallback: Option<&Store>) -> Option<Store> {
    let common = common_dir(dir).ok();
    common.and_then(|common| borrowed_store(&common, None)).or_else(|| fallback.cloned())
}

/// Put the checkout at `dir` under `store`, as far as `scope` goes, with `identity` (`-c` pairs
/// for credentials and commits; the repository's own when empty) for everything that fetches.
///
/// `dir` may be anywhere in the checkout: the repository is the main one even from one of its
/// worktrees, and a container's from inside its worktrees, while the submodules are those of the
/// working tree that holds `dir`. This is best-effort: whatever can be done is, and the report
/// says what could not.
pub fn ingest(
    store: &Store,
    dir: &Path,
    identity: &[(OsString, OsString)],
    scope: Scope,
) -> Result<Report, GitError> {
    let common = common_dir(dir)?;
    let root = repository_root(&common);
    let identity = if identity.is_empty() { local_identity(&root) } else { identity.to_vec() };
    let mut report = Report::default();
    let mut repos = Vec::new();
    if scope == Scope::Everything {
        repos.push(repository(&root, &common));
    }
    if let Some(worktree) = query(dir, &["rev-parse", "--show-toplevel"]).map(PathBuf::from) {
        let fork = fork_urls(&root);
        if scope != Scope::CheckedOut
            && let Err(err) = clone_missing(store, &worktree, &identity, fork.as_ref())
        {
            report.failures.push(format!("could not clone every submodule: {err}"));
        }
        repos.extend(submodule_repos(&worktree));
    }
    let mut repos: Vec<Repo> = repos.into_iter().flatten().collect();
    for repo in &repos {
        for (remote, url) in &repo.unusable {
            let msg = format!(
                "could not add remote {remote} of {}: the store cannot fetch from {url}",
                display(repo)
            );
            report.failures.push(msg);
        }
    }

    // The upstreams, in the store or put there.
    let mut names = Vec::new();
    for repo in &mut repos {
        for source in &mut repo.sources {
            match upstream(store, source, &mut report.added) {
                Ok(name) if !names.contains(&name) => {
                    source.name = name.clone();
                    names.push(name);
                }
                Ok(name) => source.name = name,
                Err(err) => report.failures.push(format!("could not add {}: {err}", source.name)),
            }
        }
    }
    // An upstream the store has no history of yet gets the checkout's first.
    for repo in repos.iter().filter(|repo| repo.complete) {
        for source in &repo.sources {
            let new =
                store.find(&source.name).is_some() && !store.has_refs(&source.name).unwrap_or(true);
            if new && let Err(err) = fetch_local(store, &repo.git_dir, source) {
                let msg = format!("could not fetch {} from {}: {err}", source.name, display(repo));
                report.failures.push(msg);
            }
        }
    }
    if !names.is_empty() {
        report.fetch = store.fetch(&names, true)?;
    }
    take_back_unfetched(store, &mut report);

    // Borrowing, then giving up what the store has.
    for repo in &repos {
        let names: Vec<String> = repo
            .sources
            .iter()
            .map(|s| s.name.clone())
            .filter(|n| store.find(n).is_some())
            .collect();
        if names.is_empty() {
            continue;
        }
        if let Err(err) = borrow(store, &repo.git_dir, &names) {
            report.failures.push(format!("could not borrow for {}: {err}", display(repo)));
        } else if repo.complete
            && let Err(err) = drop_borrowed(&repo.git_dir)
        {
            report.failures.push(format!("could not repack {}: {err}", display(repo)));
        }
    }
    Ok(report)
}

/// Put the repositories at `urls` in the store if they are not there, and bring them up to date
/// there, so that a clone of them made next borrows nearly everything instead of downloading it.
/// One that cannot be fetched is taken out of the store again, and the clone simply gets less
/// from the store.
pub fn prefetch(store: &Store, urls: &[&str]) -> Report {
    let mut report = Report::default();
    let mut names = Vec::new();
    for url in urls {
        let Some((name, url)) = store_upstream(url) else {
            continue;
        };
        let source = Source { name, url, remote: String::new(), tags: false };
        match upstream(store, &source, &mut report.added) {
            Ok(name) if !names.contains(&name) => names.push(name),
            Ok(_) => {}
            Err(err) => report.failures.push(format!("could not add {}: {err}", source.name)),
        }
    }
    if !names.is_empty() {
        match store.fetch(&names, false) {
            Ok(fetched) => report.fetch = fetched,
            Err(err) => report.failures.push(err.to_string()),
        }
    }
    take_back_unfetched(store, &mut report);
    report
}

/// Take the upstreams `report` added out of the store again when nothing was fetched for them,
/// so that the store never holds an upstream it has no history of.
fn take_back_unfetched(store: &Store, report: &mut Report) {
    for name in std::mem::take(&mut report.added) {
        if store.has_refs(&name).unwrap_or(true) {
            report.added.push(name);
        } else if store.remove_remote(&name).is_ok() {
            report.removed_again.push(name);
        }
    }
}

impl Report {
    /// Whether everything was done.
    pub fn complete(&self) -> bool {
        self.problems().is_empty()
    }

    /// What went wrong, one line each, for people: upstreams taken out again, failed fetches,
    /// and everything else.
    pub fn problems(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .removed_again
            .iter()
            .map(|name| {
                format!("removed {name} from the store again, since it could not be fetched")
            })
            .collect();
        if let Some(err) = &self.fetch.failed {
            lines.push(format!("fetching into the store failed: {err}"));
        }
        for (name, err) in &self.fetch.heads_failed {
            lines.push(format!("could not update {name}/-/HEAD: {err}"));
        }
        lines.extend(self.failures.iter().cloned());
        lines
    }
}

fn display(repo: &Repo) -> String {
    repo.git_dir.display().to_string()
}

/// The repository whose common git directory is `common`, held in `root`, with its own remote
/// and, for a fork, its upstream.
fn repository(root: &Path, common: &Path) -> Option<Repo> {
    let (upstream, own) = remotes(root);
    let mut repo = Repo {
        git_dir: common.to_path_buf(),
        complete: is_complete(common),
        sources: Vec::new(),
        unusable: Vec::new(),
    };
    for (remote, tags) in [(own, true), (upstream, false)] {
        if let Some(remote) = remote {
            add_source(&mut repo, root, &remote, tags);
        }
    }
    Some(repo)
}

/// Add the remote `remote` of the repository at `dir` to `repo`'s sources as an upstream, named
/// as the store would add it, or to its unusable remotes when the store cannot fetch from its
/// URL. Its URL is read from the configuration as it is written, before any `insteadOf`.
fn add_source(repo: &mut Repo, dir: &Path, remote: &str, tags: bool) {
    let Some(url) = query(dir, &["config", "--get", &format!("remote.{remote}.url")]) else {
        return;
    };
    match store_upstream(&url) {
        Some((name, url)) => {
            repo.sources.push(Source { name, url, remote: remote.to_string(), tags });
        }
        None => repo.unusable.push((remote.to_string(), url)),
    }
}

/// The URLs of the fork's own remote and its upstream, for a fork's repository at `root`.
fn fork_urls(root: &Path) -> Option<(String, String)> {
    let (upstream, own) = remotes(root);
    let url = |remote: String| query(root, &["config", "--get", &format!("remote.{remote}.url")]);
    Some((url(own?)?, url(upstream?)?))
}

/// Whether the repository whose common git directory is `common` has all its history, unlike a
/// shallow or partial clone.
fn is_complete(common: &Path) -> bool {
    let partial = query(common, &["config", "--get", "extensions.partialClone"]).is_some()
        || query(common, &["config", "--get-regexp", r"^remote\..*\.promisor$"])
            .is_some_and(|out| out.lines().any(|line| line.ends_with(" true")));
    !common.join("shallow").exists() && !partial
}

/// The submodules checked out in the working tree `worktree`, nested ones included, each with its
/// own remote.
fn submodule_repos(worktree: &Path) -> Vec<Option<Repo>> {
    let args = ["submodule", "foreach", "--quiet", "--recursive", "pwd"];
    let dirs = git::output(Some(worktree), &args).unwrap_or_default();
    dirs.lines()
        .filter(|dir| !dir.is_empty())
        .map(|dir| {
            let dir = Path::new(dir);
            let common = common_dir(dir).ok()?;
            let complete = is_complete(&common);
            let mut repo =
                Repo { git_dir: common, complete, sources: Vec::new(), unusable: Vec::new() };
            if let (_, Some(own)) = remotes(dir) {
                add_source(&mut repo, dir, &own, true);
            }
            Some(repo)
        })
        .collect()
}

/// The name `source` has in the store, putting it there if it is not, from its URL.
fn upstream(store: &Store, source: &Source, added: &mut Vec<String>) -> Result<String, GitError> {
    if let Some(name) = store.find(&source.name) {
        return Ok(name);
    }
    if store.add_remote(&source.name, &source.url)? {
        added.push(source.name.clone());
    }
    Ok(source.name.clone())
}

/// Fetch upstream `source` into `store` from the repository whose git directory is `git_dir`:
/// its remote-tracking branches for that remote and, when they came from it, its tags, under the
/// upstream's names in the store. Never its local branches, and never its `<remote>/HEAD`, which
/// fetching from the upstream itself sets.
fn fetch_local(store: &Store, git_dir: &Path, source: &Source) -> Result<(), GitError> {
    let prefix = store.prefix(&source.name)?;
    let remote = &source.remote;
    let mut args: Vec<OsString> =
        ["fetch", "--quiet", "--no-tags", "--no-prune", "--no-write-fetch-head"]
            .map(OsString::from)
            .into();
    args.push(git_dir.into());
    args.push(format!("+refs/remotes/{remote}/*:refs/remotes/{prefix}/*").into());
    args.push(format!("^refs/remotes/{remote}/HEAD").into());
    if source.tags {
        args.push(format!("+refs/tags/*:refs/tags/{prefix}/*").into());
    }
    git::run(Some(&store.path), &args)
}

/// Clone the submodules of the working tree `worktree` that are not there yet, at every level,
/// borrowing from `store`; those already there are left as they are, checkout and all. Which
/// submodules belong is git's decision, as for `git submodule init`. For a fork, `fork` has the
/// URLs of its own remote and its upstream, which the top level's relative submodule URLs are
/// resolved against first.
fn clone_missing(
    store: &Store,
    worktree: &Path,
    identity: &[(OsString, OsString)],
    fork: Option<&(String, String)>,
) -> Result<(), GitError> {
    if !worktree.join(".gitmodules").is_file() {
        return Ok(());
    }
    if let Some((own, upstream)) = fork {
        let from_upstream =
            submodules::resolve_fork_urls(worktree, own, upstream, Some(store), identity)?;
        for (name, url) in from_upstream {
            eprintln!("submodule {name}: the fork has no copy of it, so it comes from {url}");
        }
    }
    git::run(Some(worktree), &["submodule", "init"])?;
    let paths: Vec<(String, String)> = submodule_paths(worktree);
    let status = git::output(Some(worktree), &["submodule", "status"])?;
    let mut missing = Vec::new();
    let mut present = Vec::new();
    for line in status.lines() {
        // `<state><hash> <path>[ (<describe>)]`, where the path may itself contain spaces, so it
        // is matched against the paths `.gitmodules` names, the longest first.
        let Some((_, rest)) = line.get(1..).and_then(|rest| rest.split_once(' ')) else {
            continue;
        };
        let Some((name, path)) = paths
            .iter()
            .filter(|(_, p)| {
                rest == p || rest.strip_prefix(p.as_str()).is_some_and(|r| r.starts_with(" ("))
            })
            .max_by_key(|(_, p)| p.len())
        else {
            continue;
        };
        let initialized =
            query(worktree, &["config", "--get", &format!("submodule.{name}.url")]).is_some();
        match line.as_bytes().first() {
            Some(b'-') if initialized => missing.push(path.to_string()),
            Some(b'-') => {}
            _ => present.push(worktree.join(path)),
        }
    }
    let mut result = Ok(());
    if !missing.is_empty() {
        let mut args = config_args(identity);
        args.extend(negotiation_args(worktree));
        let update = ["submodule", "update", "--init", "--recursive", "--reference"];
        args.extend(update.map(OsString::from));
        args.push(store.path.clone().into());
        args.push("--".into());
        args.extend(missing.iter().map(OsString::from));
        let updated = git::run(Some(worktree), &args);
        result = updated.and(submodules::write_identity(worktree, identity));
    }
    for dir in present {
        result = result.and(clone_missing(store, &dir, identity, None));
    }
    result
}

/// The submodules `.gitmodules` in `worktree` names, as `(name, path)`.
fn submodule_paths(worktree: &Path) -> Vec<(String, String)> {
    let file = worktree.join(".gitmodules");
    let args = [OsString::from("config"), "--file".into(), file.into(), "-z".into()];
    let args = [&args[..], &["--get-regexp".into(), r"^submodule\..*\.path$".into()]].concat();
    let out = git::output(Some(worktree), &args).unwrap_or_default();
    // NUL-terminated `<key>\n<value>` entries, since names and paths may contain spaces.
    out.split('\0')
        .filter_map(|entry| {
            let (key, path) = entry.split_once('\n')?;
            let name = key.strip_prefix("submodule.")?.strip_suffix(".path")?;
            Some((name.to_string(), path.to_string()))
        })
        .collect()
}

/// Point the repository whose git directory is `git_dir` at `store` for objects, beside any
/// alternates it already has, and have it offer servers only the store history of `names`.
fn borrow(store: &Store, git_dir: &Path, names: &[String]) -> Result<(), GitError> {
    let objects = git_dir.join("objects");
    let file = objects.join("info/alternates");
    let theirs = store.path.join("objects");
    let existing = std::fs::read_to_string(&file).unwrap_or_default();
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let has =
        existing.lines().any(|line| canonical(&objects.join(line.trim())) == canonical(&theirs));
    if !has {
        let mut content = existing.into_bytes();
        if !content.is_empty() && !content.ends_with(b"\n") {
            content.push(b'\n');
        }
        content.extend(theirs.into_os_string().into_vec());
        content.push(b'\n');
        std::fs::create_dir_all(objects.join("info")).map_err(GitError::Spawn)?;
        std::fs::write(&file, content).map_err(GitError::Spawn)?;
    }
    let key = "core.alternateRefsPrefixes";
    let configured = query(git_dir, &["config", "--get", key]).unwrap_or_default();
    let mut prefixes: Vec<String> = configured.split_whitespace().map(String::from).collect();
    for prefix in alternate_refs_prefixes(names).unwrap_or_default().split_whitespace() {
        if !prefixes.iter().any(|p| p == prefix) {
            prefixes.push(prefix.to_string());
        }
    }
    git::run(Some(git_dir), &["config", key, &prefixes.join(" ")])
}

/// Give up the local copies of objects the repository whose git directory is `git_dir` can now
/// borrow from the store, keeping every other object, reachable or not.
///
/// `git repack -a -d -l` packs what is reachable and not in an alternate into a new pack and
/// deletes the old ones, which on its own would drop unreachable objects too. With `--cruft`
/// those go into a cruft pack instead, and `-l` holds for that one as well, so only objects the
/// store has are dropped; with no `--cruft-expiration`, nothing expires. Nothing happens when
/// the repository keeps no objects of its own.
fn drop_borrowed(git_dir: &Path) -> Result<(), GitError> {
    let counts = git::output(Some(git_dir), &["count-objects", "-v"])?;
    let local = counts
        .lines()
        .any(|line| matches!(line.split_once(": "), Some(("count" | "in-pack", n)) if n != "0"));
    if !local {
        return Ok(());
    }
    let args = ["repack", "-a", "-d", "-l", "--cruft", "-q", "--no-write-bitmap-index"];
    git::run(Some(git_dir), &args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_identity_reads_user_and_credential_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert!(git::run(None, &[OsString::from("init"), "-q".into(), dir.into()]).is_ok());
        for (key, value) in
            [("user.name", "Me Too"), ("credential.username", "me"), ("core.x", "y")]
        {
            git::run(Some(dir), &["config", key, value]).unwrap();
        }
        let got = local_identity(dir);
        let want: Vec<(OsString, OsString)> =
            [("user.name", "Me Too"), ("credential.username", "me")]
                .iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v)))
                .collect();
        assert_eq!(got, want);
    }
}
