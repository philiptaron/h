//! Putting a checkout under the store: `h store ingest`.
//!
//! Ingesting a checkout puts the history of all its remotes, and of its submodules' remotes at
//! every nesting level, into the store as upstreams, clones any submodules that are missing, and
//! makes the checkout and each of its submodules a [`Tenant`] of the store: a repository that
//! gets its objects from the store and keeps only those the store lacks. A clone made before the
//! store, or a submodule a pull brought in, ends up as if it had been cloned with the store in the
//! first place.
//!
//! History the checkout already has is not downloaded again: an upstream new to the store is
//! fetched first from the checkout itself, its remote-tracking branches and tags (never its local
//! branches), and only then from its URL, which then sends little more than what changed since.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use crate::clone::alternate_refs_prefixes;
use crate::git::{self, Git, GitError, config_args};
use crate::github::Lookup;
use crate::hook::{find_common_dir, find_store, read_remotes, repository_root};
use crate::resolve::{Target, parse_term, remote_name, store_upstream};
use crate::store::{FetchReport, Store};
use crate::submodules::{self, negotiation_args};

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
    /// The upstreams it renamed, as `(old, new)`: a GitHub repository the store had under
    /// another casing, or under the name it had before GitHub renamed or transferred it. When
    /// the store had the new name too, the old one was dropped instead.
    pub renamed: Vec<(String, String)>,
    /// The upstreams it put in the store and took out again, because nothing could be fetched
    /// for them, from the checkout or from their URL.
    pub removed_again: Vec<String>,
    /// The upstreams of shallow or partial repositories that it left out of the store, since
    /// fetching them there would download all the history those repositories were made without.
    pub left_out: Vec<String>,
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

/// A tenant of the store: a repository that gets its objects from the store, which its
/// `objects/info/alternates` names, and offers servers only the store history of its own
/// upstreams. Ingesting makes the checkout and each of its submodules one, as cloning with the
/// store does from the start.
#[derive(Debug, Clone)]
struct Tenant {
    /// Its common git directory, which holds its objects.
    git_dir: PathBuf,
    /// Whether it has all the history it names, which a shallow or partial clone does not: only
    /// a complete tenant is fetched from, and only a complete one gives up objects.
    complete: bool,
    /// Its remotes the store can fetch, as upstreams.
    sources: Vec<Source>,
    /// Its remotes the store cannot fetch from, as `(remote, URL)`: a local path, say, or an SSH
    /// host alias such as `me.github.com:owner/repo`.
    unusable: Vec<(String, String)>,
}

/// Read the repository's own configuration settings for the identity it was cloned with, `user.*`
/// and `credential.*`, as `-c` pairs: what the shell function passes `h`, when nothing does.
pub fn read_local_identity(dir: &Path) -> Vec<(OsString, OsString)> {
    let pattern = r"^(user|credential)\.";
    let out = git::at(dir).capture_output(&["config", "--local", "-z", "--get-regexp", pattern]);
    out.unwrap_or_default()
        .split('\0')
        .filter_map(|entry| entry.split_once('\n'))
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
        .collect()
}

/// Find the store the checkout at `dir` is a tenant of, else `fallback`, which need not exist yet.
pub fn find_checkout_store(dir: &Path, fallback: Option<&Store>) -> Option<Store> {
    let common = find_common_dir(dir).ok();
    common.and_then(|common| find_store(&common, None)).or_else(|| fallback.cloned())
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
    let common = find_common_dir(dir)?;
    let root = repository_root(&common);
    let identity = if identity.is_empty() { read_local_identity(&root) } else { identity.to_vec() };
    let lookup = Lookup::new(&identity);
    let mut report = Report::default();
    let mut tenants = Vec::new();
    if scope == Scope::Everything {
        tenants.push(find_tenant(&root, &common));
    }
    if let Some(worktree) = git::at(dir).query(&["rev-parse", "--show-toplevel"]).map(PathBuf::from)
    {
        let fork = read_fork_urls(&root);
        if scope != Scope::CheckedOut
            && let Err(err) = clone_missing(store, &worktree, &identity, fork.as_ref())
        {
            report.failures.push(format!("could not clone every submodule: {err}"));
        }
        tenants.extend(find_submodule_tenants(&worktree));
    }
    let mut tenants: Vec<Tenant> = tenants.into_iter().flatten().collect();
    for tenant in &tenants {
        for (remote, url) in &tenant.unusable {
            let msg = format!(
                "could not add remote {remote} of {}: the store cannot fetch from {url}",
                display(tenant)
            );
            report.failures.push(msg);
        }
    }

    // The upstreams, in the store or put there. A shallow or partial tenant's upstream is only
    // used when the store has it already, or a complete tenant brings it there.
    let mut names = Vec::new();
    let mut partial_only = Vec::new();
    for tenant in &mut tenants {
        for source in &mut tenant.sources {
            if !tenant.complete && store.find_upstream(&source.name).is_none() {
                partial_only.push(source.name.clone());
                continue;
            }
            match ensure_upstream(store, &lookup, source, &mut report) {
                Ok(name) if !names.contains(&name) => {
                    source.name = name.clone();
                    names.push(name);
                }
                Ok(name) => source.name = name,
                Err(err) => report.failures.push(format!("could not add {}: {err}", source.name)),
            }
        }
    }
    // A name taken as the store spelled it may have been renamed for a later remote since.
    names = follow_renames(&report.renamed, names);
    for source in tenants.iter_mut().flat_map(|tenant| tenant.sources.iter_mut()) {
        source.name = follow_renames(&report.renamed, vec![source.name.clone()]).remove(0);
    }
    for name in partial_only {
        if !names.contains(&name) && !report.left_out.contains(&name) {
            report.left_out.push(name);
        }
    }
    // An upstream the store has no history of yet gets the tenant's first.
    for tenant in tenants.iter().filter(|tenant| tenant.complete) {
        for source in &tenant.sources {
            let new = store.find_upstream(&source.name).is_some()
                && !store.has_refs(&source.name).unwrap_or(true);
            if new && let Err(err) = fetch_local(store, &tenant.git_dir, source) {
                let msg =
                    format!("could not fetch {} from {}: {err}", source.name, display(tenant));
                report.failures.push(msg);
            }
        }
    }
    if !names.is_empty() {
        report.fetch = store.fetch(&names, true)?;
    }
    take_back_unfetched(store, &mut report);

    // A clone made before an upstream was renamed still names it the old way. The tenants below
    // are repaired as they move in; this is for the repository itself in every scope.
    if let Err(err) = repair_prefixes(store, &common) {
        report.failures.push(format!("could not update {}: {err}", common.display()));
    }
    // Each tenant moves in, then gives up what the store has.
    for tenant in &tenants {
        let names: Vec<String> = tenant
            .sources
            .iter()
            .map(|s| s.name.clone())
            .filter(|n| store.find_upstream(n).is_some())
            .collect();
        if names.is_empty() {
            continue;
        }
        if let Err(err) = make_tenant(store, &tenant.git_dir, &names) {
            let msg = format!("could not make {} a tenant of the store: {err}", display(tenant));
            report.failures.push(msg);
        } else if tenant.complete
            && let Err(err) = drop_borrowed(&tenant.git_dir)
        {
            report.failures.push(format!("could not repack {}: {err}", display(tenant)));
        }
    }
    Ok(report)
}

/// Put the repositories at `urls` in the store if they are not there, and bring them up to date
/// there, so that a clone of them made next borrows nearly everything instead of downloading it.
/// One that cannot be fetched is taken out of the store again, and the clone simply gets less
/// from the store. GitHub is asked for their names through `lookup`, as [`ingest`] asks it.
pub fn prefetch(store: &Store, urls: &[&str], lookup: &Lookup) -> Report {
    let mut report = Report::default();
    let mut names = Vec::new();
    for url in urls {
        let Some((name, url)) = store_upstream(url) else {
            continue;
        };
        let source = Source { name, url, remote: String::new(), tags: false };
        match ensure_upstream(store, lookup, &source, &mut report) {
            Ok(name) if !names.contains(&name) => names.push(name),
            Ok(_) => {}
            Err(err) => report.failures.push(format!("could not add {}: {err}", source.name)),
        }
    }
    let names = follow_renames(&report.renamed, names);
    if !names.is_empty() {
        match store.fetch(&names, false) {
            Ok(fetched) => report.fetch = fetched,
            Err(err) => {
                report.failures.push(err.context("could not fetch into the store").to_string())
            }
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
    pub fn is_complete(&self) -> bool {
        self.problems().is_empty()
    }

    /// What went wrong, one line each, for people: upstreams taken out again, failed fetches,
    /// and everything else. Git names each upstream it could not fetch as it fails, so a failed
    /// fetch is said only when no upstream taken out again says it already.
    pub fn problems(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .removed_again
            .iter()
            .map(|name| {
                format!("removed {name} from the store again, since it could not be fetched")
            })
            .collect();
        if self.fetch.failed.is_some() && self.removed_again.is_empty() {
            lines.push("could not fetch every upstream into the store".into());
        }
        for (name, err) in &self.fetch.heads_failed {
            lines.push(format!("could not update {name}/-/HEAD: {err}"));
        }
        lines.extend(self.failures.iter().cloned());
        lines
    }
}

fn display(tenant: &Tenant) -> String {
    tenant.git_dir.display().to_string()
}

/// Find the repository whose common git directory is `common`, held in `root`, as a tenant, with
/// every one of its remotes.
fn find_tenant(root: &Path, common: &Path) -> Option<Tenant> {
    let mut tenant = Tenant {
        git_dir: common.to_path_buf(),
        complete: is_complete(common),
        sources: Vec::new(),
        unusable: Vec::new(),
    };
    add_sources(&mut tenant, root);
    Some(tenant)
}

/// Add every remote of the repository at `dir` to `tenant`'s sources: its own first (`origin`, or
/// its only remote besides `upstream`), whose tags are the repository's, then `upstream`, for a
/// fork, then the rest, in the order the configuration has them. Tags fetched from any of the
/// others share one namespace with the own remote's in the repository, so they are not counted
/// as theirs; the store gets each one's tags from its URL.
fn add_sources(tenant: &mut Tenant, dir: &Path) {
    let repo = git::at(dir);
    let (upstream, own) = read_remotes(repo);
    let all: Vec<String> =
        repo.query(&["remote"]).unwrap_or_default().lines().map(String::from).collect();
    let others = all.iter().filter(|r| Some(*r) != own.as_ref() && Some(*r) != upstream.as_ref());
    if let Some(own) = &own {
        add_source(tenant, repo, own, true);
    }
    for remote in upstream.iter().chain(others) {
        add_source(tenant, repo, remote, false);
    }
}

/// Add the remote `remote` of `repo` to `tenant`'s sources as an upstream, named
/// as the store would add it, or to its unusable remotes when the store cannot fetch from its
/// URL. Its URL is read from the configuration as it is written, before any `insteadOf`.
fn add_source(tenant: &mut Tenant, repo: Git<'_>, remote: &str, tags: bool) {
    let Some(url) = repo.read_remote_url(remote) else {
        return;
    };
    match store_upstream(&url) {
        Some((name, url)) => {
            tenant.sources.push(Source { name, url, remote: remote.to_string(), tags });
        }
        None => tenant.unusable.push((remote.to_string(), url)),
    }
}

/// Read the URLs of the fork's own remote and its upstream, for a fork's repository at `root`.
fn read_fork_urls(root: &Path) -> Option<(String, String)> {
    let repo = git::at(root);
    let (upstream, own) = read_remotes(repo);
    Some((repo.read_remote_url(&own?)?, repo.read_remote_url(&upstream?)?))
}

/// Whether the repository whose common git directory is `common` has all its history, unlike a
/// shallow or partial clone.
fn is_complete(common: &Path) -> bool {
    let repo = git::at(common);
    let partial = repo.query(&["config", "--get", "extensions.partialClone"]).is_some()
        || repo
            .query(&["config", "--get-regexp", r"^remote\..*\.promisor$"])
            .is_some_and(|out| out.lines().any(|line| line.ends_with(" true")));
    !common.join("shallow").exists() && !partial
}

/// Find the submodules checked out in the working tree `worktree`, nested ones included, as
/// tenants, each with every one of its remotes.
fn find_submodule_tenants(worktree: &Path) -> Vec<Option<Tenant>> {
    submodules::find_checked_out(worktree)
        .into_iter()
        .map(|dir| {
            let dir = dir.as_path();
            let common = find_common_dir(dir).ok()?;
            let complete = is_complete(&common);
            let mut tenant =
                Tenant { git_dir: common, complete, sources: Vec::new(), unusable: Vec::new() };
            add_sources(&mut tenant, dir);
            Some(tenant)
        })
        .collect()
}

/// Make sure `source` is in the store, putting it there from its URL if it is not, and say
/// what the store calls it.
///
/// A name the store has spelled exactly so is taken as it is, without the network. Otherwise a
/// GitHub repository is named as GitHub names it, in its casing and under its current owner and
/// name, and whatever the store has under another casing of that name, or of the name the remote
/// gives, is renamed to it, or dropped when the store has both. When GitHub cannot be asked, a
/// casing the store has already will do, and nothing is renamed.
fn ensure_upstream(
    store: &Store,
    lookup: &Lookup,
    source: &Source,
    report: &mut Report,
) -> Result<String, GitError> {
    if store.list_remotes()?.contains(&source.name) {
        return Ok(source.name.clone());
    }
    let (name, url) = match ask_canonical(lookup, &source.url) {
        Some(canonical) => canonical,
        None => match store.find_upstream(&source.name) {
            Some(name) => return Ok(name),
            None => (source.name.clone(), source.url.clone()),
        },
    };
    let same = |r: &String| {
        *r != name && (r.eq_ignore_ascii_case(&name) || r.eq_ignore_ascii_case(&source.name))
    };
    for other in store.list_remotes()?.into_iter().filter(same) {
        store.rename_remote(&other, &name, &url)?;
        report.renamed.push((other, name.clone()));
    }
    if store.add_remote(&name, &url)? {
        report.added.push(name.clone());
    }
    Ok(name)
}

/// `names` as they are after the renames `renamed`, each once.
fn follow_renames(renamed: &[(String, String)], names: Vec<String>) -> Vec<String> {
    let mut followed: Vec<String> = Vec::new();
    for mut name in names {
        while let Some((_, new)) = renamed.iter().find(|(old, _)| *old == name) {
            name = new.clone();
        }
        if !followed.contains(&name) {
            followed.push(name);
        }
    }
    followed
}

/// Ask GitHub for the store's name for the repository a remote fetches from at `url`, and the
/// URL to fetch it from, as GitHub names it now. `None` for another host, or when GitHub cannot
/// say.
fn ask_canonical(lookup: &Lookup, url: &str) -> Option<(String, String)> {
    let Ok(Target::GitHub { user, repo }) = parse_term(url) else {
        return None;
    };
    let info = lookup.ask(&user, &repo).found()?;
    let full = format!("{}/{}", info.owner, info.name);
    Some((remote_name("github.com", &full), format!("https://github.com/{full}.git")))
}

/// Point the `core.alternateRefsPrefixes` of the repository whose git directory is `git_dir` at
/// the store's names for its upstreams as they are now: a prefix naming an upstream the store
/// has only under another casing, as one renamed since does, is spelled as the store spells it,
/// and one naming an upstream the store no longer has is dropped. Prefixes for anything but a
/// store upstream are left alone.
pub fn repair_prefixes(store: &Store, git_dir: &Path) -> Result<(), GitError> {
    let key = "core.alternateRefsPrefixes";
    let repo = git::at(git_dir);
    let Some(configured) = repo.query(&["config", "--get", key]) else {
        return Ok(());
    };
    let existing = store.list_remotes()?;
    let mut prefixes: Vec<String> = Vec::new();
    for prefix in configured.split_whitespace() {
        let parsed = ["refs/remotes/", "refs/tags/"].into_iter().find_map(|namespace| {
            let name = prefix.strip_prefix(namespace)?.strip_suffix('/')?;
            Some((namespace, name))
        });
        let repaired = match parsed {
            None => Some(prefix.to_string()),
            Some((_, name)) if existing.iter().any(|r| r == name) => Some(prefix.to_string()),
            Some((namespace, name)) => {
                store.find_upstream(name).map(|found| format!("{namespace}{found}/"))
            }
        };
        if let Some(repaired) = repaired.filter(|p| !prefixes.contains(p)) {
            prefixes.push(repaired);
        }
    }
    if prefixes.join(" ") == configured.trim() {
        return Ok(());
    }
    if prefixes.is_empty() {
        return repo.run(&["config", "--unset", key]);
    }
    repo.run(&["config", key, &prefixes.join(" ")])
}

/// Fetch upstream `source` into `store` from the repository whose git directory is `git_dir`:
/// its remote-tracking branches for that remote and, when they came from it, its tags, under the
/// upstream's names in the store. Never its local branches, and never its `<remote>/HEAD`, which
/// fetching from the upstream itself sets.
fn fetch_local(store: &Store, git_dir: &Path, source: &Source) -> Result<(), GitError> {
    let prefix = store.find_prefix(&source.name)?;
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
    git::at(&store.path).run(&args)
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
    // A gitlink `.gitmodules` does not map is no submodule, to git or to h, but `git submodule`
    // stops at one unless given only the others.
    let (paths, unmapped) = submodules::read_gitlinks(worktree);
    for path in &unmapped {
        eprintln!(
            "left {path} in {} as it is, since .gitmodules names no submodule there",
            worktree.display()
        );
    }
    if paths.is_empty() {
        return Ok(());
    }
    let mapped = || paths.iter().map(|(_, path)| OsString::from(path));
    // Without a pathspec, `git submodule init` initializes every submodule, or only the active
    // ones when `submodule.active` is set, which leaves the unmapped out too; given paths, it
    // initializes them active or not, so they are given only when nothing else leaves those out.
    let repo = git::at(worktree);
    let active = repo.query(&["config", "--get-all", "submodule.active"]).is_some();
    let mut init: Vec<OsString> = ["submodule", "init", "--"].map(OsString::from).into();
    if !unmapped.is_empty() && !active {
        init.extend(mapped());
    }
    repo.run(&init)?;
    let mut args: Vec<OsString> = ["submodule", "status", "--"].map(OsString::from).into();
    args.extend(mapped());
    let status = repo.capture_output(&args)?;
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
        let initialized = git::at(worktree)
            .query(&["config", "--get", &format!("submodule.{name}.url")])
            .is_some();
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
        let updated = repo.run(&args);
        result = updated.and(submodules::write_identity(worktree, identity));
    }
    for dir in present {
        result = result.and(clone_missing(store, &dir, identity, None));
    }
    result
}

/// Make the repository whose git directory is `git_dir` a tenant of `store`: point it at the store
/// for objects, beside any alternates it already has, and have it offer servers only the store
/// history of `names`.
fn make_tenant(store: &Store, git_dir: &Path, names: &[String]) -> Result<(), GitError> {
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
        let info = objects.join("info");
        std::fs::create_dir_all(&info).map_err(git::file_error("create", &info))?;
        std::fs::write(&file, content).map_err(git::file_error("write", &file))?;
    }
    repair_prefixes(store, git_dir)?;
    let key = "core.alternateRefsPrefixes";
    let repo = git::at(git_dir);
    let configured = repo.query(&["config", "--get", key]).unwrap_or_default();
    let mut prefixes: Vec<String> = configured.split_whitespace().map(String::from).collect();
    for prefix in alternate_refs_prefixes(names).unwrap_or_default().split_whitespace() {
        if !prefixes.iter().any(|p| p == prefix) {
            prefixes.push(prefix.to_string());
        }
    }
    repo.run(&["config", key, &prefixes.join(" ")])
}

/// Give up the local copies of objects the tenant whose git directory is `git_dir` can now get
/// from the store, keeping every other object, reachable or not.
///
/// `git repack -a -d -l` packs what is reachable and not in an alternate into a new pack and
/// deletes the old ones, which on its own would drop unreachable objects too. With `--cruft`
/// those go into a cruft pack instead, and `-l` holds for that one as well, so only objects the
/// store has are dropped; with no `--cruft-expiration`, nothing expires. Nothing happens when
/// the repository keeps no objects of its own.
fn drop_borrowed(git_dir: &Path) -> Result<(), GitError> {
    let repo = git::at(git_dir);
    let counts = repo.capture_output(&["count-objects", "-v"])?;
    let local = counts
        .lines()
        .any(|line| matches!(line.split_once(": "), Some(("count" | "in-pack", n)) if n != "0"));
    if !local {
        return Ok(());
    }
    let args = ["repack", "-a", "-d", "-l", "--cruft", "-q", "--no-write-bitmap-index"];
    repo.run(&args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::pairs;

    #[test]
    fn local_identity_reads_user_and_credential_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        assert!(git::run(&git::args!["init", "-q", dir]).is_ok());
        for (key, value) in
            [("user.name", "Me Too"), ("credential.username", "me"), ("core.x", "y")]
        {
            git::at(dir).run(&["config", key, value]).unwrap();
        }
        let want = pairs(&[("user.name", "Me Too"), ("credential.username", "me")]);
        assert_eq!(read_local_identity(dir), want);
    }

    #[test]
    fn failed_fetches_are_said_once() {
        let failed = || GitError::Failed { args: vec!["fetch".into()], code: 1 };
        let mut report = Report::default();
        report.fetch.failed = Some(failed());
        assert_eq!(report.problems(), ["could not fetch every upstream into the store"]);
        report.removed_again.push("github.com/me/gone".into());
        let removed =
            "removed github.com/me/gone from the store again, since it could not be fetched";
        assert_eq!(report.problems(), [removed]);
    }
}
