//! Cloning repositories with `git`: optionally borrowing objects from a store, laying the clone
//! out as a bare repository plus worktrees, and wiring up a fork's upstream so it can be pulled
//! from but never pushed to.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::Path;
use std::process::Stdio;

use crate::git::{self, GitError, config_pairs, non_config_opts};
use crate::store::Store;
use crate::submodules::{self, SubmoduleOptions};

/// The name of the bare repository inside a container clone.
pub const BARE_DIR: &str = ".bare";

/// The push URL that makes every push to a remote fail.
pub const NO_PUSH: &str = "no_push";

/// What to clone and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneRequest<'a> {
    pub url: &'a str,
    pub path: &'a Path,
    /// Options the shell function always passes: the identity, as `-c key=value` pairs.
    pub git_opts: &'a [OsString],
    /// Options the user gave for this clone.
    pub extra: &'a [OsString],
    /// An object store to borrow objects from, when it exists.
    pub reference: Option<&'a Path>,
    /// The upstreams in the store that share history with this clone (its own and, for a fork,
    /// its parent's). Only their refs are offered to the server as history the clone already
    /// has, instead of every ref in the store.
    pub reference_names: &'a [String],
    /// Lay the clone out as `<path>/.bare` plus a `.git` file, with no working tree, so that all
    /// work happens in worktrees under `<path>`.
    pub container: bool,
    /// The URL of the repository this one is a fork of.
    pub upstream_url: Option<&'a str>,
}

/// How a plain clone deals with its submodules and the store.
struct Plan<'a> {
    submodules: SubmoduleOptions,
    /// The store to borrow from, if any.
    store: Option<&'a Path>,
    /// Whether the submodules are left out of `git clone` and cloned afterwards by
    /// [`submodules::update`]: so that they borrow from the store too, and so that a fork's
    /// upstream is there before them.
    split: bool,
}

fn plan<'a>(req: &CloneRequest<'a>) -> Plan<'a> {
    let submodules = SubmoduleOptions::parse(req.git_opts.iter().chain(req.extra));
    let recurses = !submodules.pathspecs().is_empty();
    // With its own `--reference`, a recursive clone is git's to finish: git looks for each
    // submodule under `modules/` of every alternate, the store's included, and fails when it
    // is not there, or refuses outright beside `--reference-if-able`.
    let own_references = recurses && submodules.references;
    let store = req.reference.filter(|_| !own_references);
    let split = recurses && !own_references && (store.is_some() || req.upstream_url.is_some());
    Plan { submodules, store, split }
}

/// Arguments to pass to `git` for a plain (non-container) clone.
///
/// Submodules are cloned too unless an option says otherwise, and objects are borrowed from the
/// reference store when there is one. With a store, or for a fork, `git clone` leaves the
/// submodules alone and they are cloned afterwards by [`submodules::update`], so that they borrow
/// from the store as well and come after the fork's upstream.
///
/// The identity in `git_opts` goes to git itself as well as to `git clone`, which writes it only
/// into the new repository: given to git, it reaches the clones of submodules too, nested ones
/// included, through the environment, so they fetch with the same credentials.
pub fn git_clone_args(req: &CloneRequest) -> Vec<OsString> {
    let plan = plan(req);
    let mut args = submodules::config_args(&config_pairs(req.git_opts));
    args.push("clone".into());
    if plan.submodules.recurse.is_none() && !plan.submodules.bare && !plan.split {
        args.push("--recursive".into());
    }
    if let Some(store) = plan.store {
        args.push("--reference-if-able".into());
        args.push(store.into());
        if let Some(prefixes) = alternate_refs_prefixes(req.reference_names) {
            args.push("-c".into());
            args.push(format!("core.alternateRefsPrefixes={prefixes}").into());
        }
    }
    if plan.split {
        // What `git clone --recurse-submodules` itself records.
        for pathspec in plan.submodules.pathspecs() {
            args.push("-c".into());
            args.push(format!("submodule.active={pathspec}").into());
        }
    }
    args.extend(req.git_opts.iter().cloned());
    // Without a filter, `git clone` is left to reject `--also-filter-submodules` itself.
    let only_for_submodules = |opt: &&OsString| {
        plan.split && plan.submodules.filter.is_some() && SubmoduleOptions::is_submodule_only(opt)
    };
    args.extend(req.extra.iter().filter(|opt| !only_for_submodules(opt)).cloned());
    if plan.split {
        args.push("--no-recurse-submodules".into());
    }
    args.push("--".into());
    args.push(req.url.into());
    args.push(req.path.into());
    args
}

/// The `core.alternateRefsPrefixes` that limits the refs a clone borrowing from the store
/// negotiates with to those of the store upstreams `names`, or `None` when there are none.
///
/// Fetching treats every ref of an alternate as history it already has and sends the server
/// all of it until the server recognizes a commit, so a clone of something unrelated to the
/// store would otherwise send every commit in the store first.
pub(crate) fn alternate_refs_prefixes(names: &[String]) -> Option<String> {
    let prefixes: Vec<String> = names
        .iter()
        .flat_map(|name| [format!("refs/remotes/{name}/"), format!("refs/tags/{name}/")])
        .collect();
    (!prefixes.is_empty()).then(|| prefixes.join(" "))
}

/// Clone as `req` asks, creating parent directories. Returns git's exit status, 0 on success.
pub fn clone_repo(req: &CloneRequest) -> u8 {
    if let Some(parent) = req.path.parent() {
        // Any failure here is reported by git itself.
        let _ = std::fs::create_dir_all(parent);
    }
    let result = if req.container {
        // A container's origin has every branch, however much history it has.
        clone_container(req).and_then(|()| match req.upstream_url {
            Some(url) => {
                add_upstream(&req.path.join(BARE_DIR), url, &history_opts(req.extra), false)
            }
            None => Ok(()),
        })
    } else {
        clone_plain(req)
    };
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            err.code()
        }
    }
}

/// Clone as `req` asks: the superproject, then a fork's upstream, then the submodules when h
/// clones them itself, and finally the identity into every submodule. The upstream is added even
/// when git's own clone of the submodules fails, since git leaves the superproject in place then;
/// the first failure is the one returned.
fn clone_plain(req: &CloneRequest) -> Result<(), GitError> {
    let plan = plan(req);
    let cloned = git::run(None, &git_clone_args(req));
    if cloned.is_err() && !req.path.join(".git").exists() {
        return cloned;
    }
    let upstream = match req.upstream_url {
        Some(url) => {
            add_upstream(req.path, url, &history_opts(req.extra), single_branch(req.extra))
        }
        None => Ok(()),
    };
    let identity = config_pairs(req.git_opts);
    let has_submodules = req.path.join(".gitmodules").is_file();
    let cloned_submodules = match &plan {
        Plan { submodules, store, split: true }
            if cloned.is_ok() && has_submodules && submodules.checks_out() =>
        {
            let resolved = match req.upstream_url {
                Some(upstream) => fork_submodule_urls(req, upstream, *store, &identity),
                None => Ok(()),
            };
            resolved.and_then(|()| submodules::update(req.path, *store, submodules, &identity))
        }
        _ => Ok(()),
    };
    let identified =
        if has_submodules { submodules::write_identity(req.path, &identity) } else { Ok(()) };
    cloned.and(upstream).and(cloned_submodules).and(identified)
}

/// Point the relative submodule URLs of the fork cloned as `req` at the fork's copies where it has
/// them and at the upstream's, at `upstream`, where it does not, saying which come from upstream.
fn fork_submodule_urls(
    req: &CloneRequest,
    upstream: &str,
    store: Option<&Path>,
    identity: &[(OsString, OsString)],
) -> Result<(), GitError> {
    let store = store.map(Store::new);
    let from_upstream =
        submodules::resolve_fork_urls(req.path, req.url, upstream, store.as_ref(), identity)?;
    for (name, url) in from_upstream {
        eprintln!("submodule {name}: the fork has no copy of it, so it comes from {url}");
    }
    Ok(())
}

/// Clone options that mean the same to `git fetch`, so a container clone can pass them on.
const FETCH_FLAGS: &[&str] = &["-q", "--quiet", "-v", "--verbose", "--progress", "--no-tags"];
const FETCH_OPTIONS: &[&str] = &["--depth", "--shallow-since", "--shallow-exclude", "--filter"];

/// The `git fetch` arguments for the clone options `extra` of a container clone, or an error
/// naming the first one that `git fetch` does not share.
fn container_fetch_opts(extra: &[OsString]) -> Result<Vec<OsString>, GitError> {
    let opts = non_config_opts(extra);
    let mut out = Vec::new();
    let mut iter = opts.into_iter();
    while let Some(opt) = iter.next() {
        let text = opt.to_string_lossy();
        let name = text.split_once('=').map_or(&*text, |(name, _)| name);
        if FETCH_FLAGS.contains(&&*text) || (FETCH_OPTIONS.contains(&name) && name != text) {
            out.push(opt);
        } else if FETCH_OPTIONS.contains(&name) {
            let Some(value) = iter.next() else {
                return Err(GitError::Invalid(format!("{text} needs a value")));
            };
            out.extend([opt, value]);
        } else {
            return Err(GitError::Invalid(format!("{text} cannot be used with --container")));
        }
    }
    Ok(out)
}

/// The options among a clone's `extra` that limit how much history is fetched, so that the
/// upstream of a shallow or partial clone is fetched the same way instead of in full.
fn history_opts(extra: &[OsString]) -> Vec<OsString> {
    let mut out = Vec::new();
    let mut iter = non_config_opts(extra).into_iter();
    while let Some(opt) = iter.next() {
        let text = opt.to_string_lossy();
        match text.split_once('=') {
            Some((name, _)) if FETCH_OPTIONS.contains(&name) => out.push(opt),
            None if FETCH_OPTIONS.contains(&&*text) => {
                out.push(opt);
                out.extend(iter.next());
            }
            _ => {}
        }
    }
    out
}

/// Whether the clone options `extra` limit how much history the clone gets, by depth, date or
/// filter, as a shallow or partial clone does.
pub fn limits_history(extra: &[OsString]) -> bool {
    !history_opts(extra).is_empty()
}

/// The clone options that limit history by depth, and so imply `--single-branch`.
const DEEPEN_OPTIONS: &[&str] = &["--depth", "--shallow-since", "--shallow-exclude"];

/// Whether a clone with the options `extra` fetches a single branch, as `git clone` decides:
/// `--single-branch` or `--no-single-branch`, whichever comes last, or else whether its history
/// is limited by depth.
fn single_branch(extra: &[OsString]) -> bool {
    let explicit = non_config_opts(extra).iter().rev().find_map(|opt| match opt.to_str() {
        Some("--single-branch") => Some(true),
        Some("--no-single-branch") => Some(false),
        _ => None,
    });
    explicit.unwrap_or_else(|| {
        history_opts(extra).iter().any(|opt| {
            let text = opt.to_string_lossy();
            DEEPEN_OPTIONS.contains(&text.split_once('=').map_or(&*text, |(name, _)| name))
        })
    })
}

/// Create `<path>/.bare` by fetching into a fresh bare repository, then point `<path>/.git` at
/// it. Unlike `git clone --bare`, the result has `origin` with ordinary remote-tracking branches,
/// so worktrees are added from `origin/<branch>` and local branches are only ever the user's.
/// HEAD is detached at `origin/HEAD`, so a new branch made by `git worktree add <dir>` starts
/// from the default branch instead of being an orphan of the branch HEAD names but nobody has.
/// The repository takes the remote's object format, as `git clone` does, rather than the
/// default for new repositories, so that SHA-1 remotes can be fetched when the default is
/// SHA-256. A remote whose HEAD names no commit, such as an empty one, is refused: it has no
/// format to take and no branch to start worktrees from.
///
/// A failure removes `<path>` again, as `git clone` does, so that a later `h` does not mistake
/// the remains for a finished clone.
fn clone_container(req: &CloneRequest) -> Result<(), GitError> {
    let fetch_opts = container_fetch_opts(req.extra)?;
    let existed = req.path.exists();
    let result = fill_container(req, fetch_opts);
    if result.is_err() && !existed {
        let _ = std::fs::remove_dir_all(req.path);
    }
    result
}

fn fill_container(req: &CloneRequest, fetch_opts: Vec<OsString>) -> Result<(), GitError> {
    let bare = req.path.join(BARE_DIR);
    let mut opts = req.git_opts.to_vec();
    opts.extend(req.extra.iter().cloned());
    let config = config_pairs(&opts);
    let head = remote_head(req.url, req.path.parent(), &config)?;
    let format = format!("--object-format={}", head.object_format()?);
    git::run(
        None,
        &[
            OsString::from("init"),
            "--bare".into(),
            "--quiet".into(),
            format.into(),
            bare.clone().into(),
        ],
    )?;
    // Worktrees link to the container by relative paths, so the whole can be moved (git 2.48).
    git::run(Some(&bare), &["config", "worktree.useRelativePaths", "true"])?;
    for (key, value) in config {
        git::run(Some(&bare), &[OsString::from("config"), key, value])?;
    }
    if let Some(reference) = req.reference.filter(|r| r.join("objects").is_dir()) {
        let alternates = bare.join("objects/info/alternates");
        let mut line = reference.join("objects").into_os_string().into_vec();
        line.push(b'\n');
        std::fs::write(&alternates, line).map_err(GitError::Spawn)?;
        if let Some(prefixes) = alternate_refs_prefixes(req.reference_names) {
            git::run(Some(&bare), &["config", "core.alternateRefsPrefixes", &prefixes])?;
        }
    }
    let mut remote_add = vec!["remote", "add"];
    // As with `git clone --no-tags`, later fetches leave tags alone too.
    if fetch_opts.iter().any(|o| o == "--no-tags") {
        remote_add.push("--no-tags");
    }
    remote_add.extend(["origin", req.url]);
    git::run(Some(&bare), &remote_add)?;
    let mut fetch: Vec<OsString> = vec!["fetch".into()];
    fetch.extend(fetch_opts);
    fetch.push("origin".into());
    git::run(Some(&bare), &fetch)?;
    let start = match &head.branch {
        Some(branch) => {
            // Fetching sets origin/HEAD itself since git 2.48; earlier versions do not.
            let target = format!("refs/remotes/origin/{branch}");
            git::run(Some(&bare), &["symbolic-ref", "refs/remotes/origin/HEAD", &target])?;
            "refs/remotes/origin/HEAD"
        }
        None => &head.oid,
    };
    git::run(Some(&bare), &["update-ref", "--no-deref", "HEAD", start])?;
    std::fs::write(req.path.join(".git"), format!("gitdir: ./{BARE_DIR}\n"))
        .map_err(GitError::Spawn)
}

/// Detach the HEAD of the container clone at `path`, if it is one, at `origin/HEAD` again.
///
/// A container's HEAD is where `git worktree add <dir>` starts a new branch. It is detached at
/// `origin/HEAD` when the container is cloned, but fetching moves only `origin/HEAD`, so new
/// branches would start from the commit the container was cloned at. `h` calls this whenever it
/// goes to an existing directory, which keeps HEAD as fresh as the last fetch. Only refs are
/// read and written, without the network, and nothing is printed. A HEAD the user made symbolic
/// is left alone, and so is HEAD when `origin/HEAD` is missing.
pub fn refresh_container_head(path: &Path) {
    let bare = path.join(BARE_DIR);
    if !bare.is_dir() {
        return;
    }
    let dir = Some(bare.as_path());
    if git::output(dir, &["symbolic-ref", "--quiet", "HEAD"]).is_ok() {
        return;
    }
    let commit = |rev: &str| {
        let out = git::output(dir, &["rev-parse", "--verify", "--quiet", rev]).ok()?;
        Some(out.trim().to_string())
    };
    let (Some(new), Some(old)) = (commit("refs/remotes/origin/HEAD^{commit}"), commit("HEAD"))
    else {
        return;
    };
    if new != old {
        let _ = git::output(dir, &["update-ref", "--no-deref", "HEAD", &new, &old]);
    }
}

/// The remote's HEAD: the commit it names and, when the server says, its branch.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RemoteHead {
    oid: String,
    branch: Option<String>,
}

impl RemoteHead {
    /// The object format of the remote, from the length of the commit's name.
    fn object_format(&self) -> Result<&'static str, GitError> {
        match self.oid.len() {
            40 => Ok("sha1"),
            64 => Ok("sha256"),
            _ => Err(GitError::Invalid(format!("Unknown object format for {}", self.oid))),
        }
    }
}

/// Ask the remote at `url` for its HEAD with `git ls-remote --symref`, run in `dir` with the
/// clone's `-c` settings in effect, since they may choose the credentials (as
/// `credential.username` does). Git's errors go to stderr, so that the user sees why it failed.
fn remote_head(
    url: &str,
    dir: Option<&Path>,
    config: &[(OsString, OsString)],
) -> Result<RemoteHead, GitError> {
    let mut args: Vec<OsString> = Vec::new();
    for (key, value) in config {
        let mut setting = key.clone();
        setting.push("=");
        setting.push(value);
        args.extend(["-c".into(), setting]);
    }
    args.extend(["ls-remote".into(), "--symref".into(), url.into(), "HEAD".into()]);
    let out = git::command(dir, &args)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(GitError::Spawn)?;
    if !out.status.success() {
        return Err(GitError::Failed { args, code: git::exit_code(out.status) });
    }
    parse_remote_head(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| {
        GitError::Invalid(format!(
            "{url} has no default branch, so it cannot be cloned with --container"
        ))
    })
}

/// Read `git ls-remote --symref <url> HEAD`, which prints `ref: refs/heads/<branch>\tHEAD` and
/// `<oid>\tHEAD`. `None` when HEAD names no commit, as in an empty repository.
fn parse_remote_head(out: &str) -> Option<RemoteHead> {
    let mut oid = None;
    let mut branch = None;
    for value in out.lines().filter_map(|line| line.strip_suffix("\tHEAD")) {
        if let Some(target) = value.strip_prefix("ref: ") {
            branch = target.strip_prefix("refs/heads/").map(String::from);
        } else if !value.is_empty() && value.bytes().all(|b| b.is_ascii_hexdigit()) {
            oid = Some(value.to_string());
        }
    }
    Some(RemoteHead { oid: oid?, branch })
}

/// Add `url` as the `upstream` remote of the repository at `dir`, fetchable but not pushable,
/// and make the clone's own remote the default push target. That remote is read back from the
/// clone, since `--origin` and `clone.defaultRemoteName` can call it something other than
/// `origin`. `fetch_opts` (such as `--depth 1`) are passed to the first fetch. With
/// `single_branch`, only the upstream's default branch is fetched, then and later, as `git clone`
/// does for its own remote.
pub fn add_upstream(
    dir: &Path,
    url: &str,
    fetch_opts: &[OsString],
    single_branch: bool,
) -> Result<(), GitError> {
    let remotes = git::output(Some(dir), &["remote"])?;
    let [origin] = remotes.lines().collect::<Vec<_>>()[..] else {
        let msg = format!("Cannot tell which remote of {} to push to: {remotes:?}", dir.display());
        return Err(GitError::Invalid(msg));
    };
    let dir = Some(dir);
    let branch = if single_branch { default_branch(dir, url)? } else { None };
    let mut add = vec!["remote", "add"];
    if let Some(branch) = &branch {
        add.extend(["-t", branch]);
    }
    add.extend(["upstream", url]);
    git::run(dir, &add)?;
    git::run(dir, &["config", "remote.upstream.pushurl", NO_PUSH])?;
    git::run(dir, &["config", "remote.upstream.tagOpt", "--no-tags"])?;
    git::run(dir, &["config", "remote.pushDefault", origin])?;
    let mut fetch: Vec<OsString> = vec!["fetch".into(), "--quiet".into()];
    fetch.extend(fetch_opts.iter().cloned());
    fetch.push("upstream".into());
    git::run(dir, &fetch)
}

/// The branch that HEAD names in the repository at `url`, if it names one.
fn default_branch(dir: Option<&Path>, url: &str) -> Result<Option<String>, GitError> {
    let out = git::output(dir, &["ls-remote", "--symref", url, "HEAD"])?;
    let branch =
        out.lines().find_map(|line| line.strip_prefix("ref: refs/heads/")?.strip_suffix("\tHEAD"));
    Ok(branch.map(String::from))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter().map(|a| a.into_string().unwrap()).collect()
    }

    fn opts(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    fn request<'a>(git_opts: &'a [OsString], extra: &'a [OsString]) -> CloneRequest<'a> {
        CloneRequest {
            url: "https://x/y.git",
            path: Path::new("/code/x/y"),
            git_opts,
            extra,
            reference: None,
            reference_names: &[],
            container: false,
            upstream_url: None,
        }
    }

    #[test]
    fn defaults_to_recursive() {
        let args = git_clone_args(&request(&[], &[]));
        assert_eq!(strings(args), ["clone", "--recursive", "--", "https://x/y.git", "/code/x/y"]);
    }

    #[test]
    fn identity_and_extra_options_keep_recursive() {
        let git_opts = opts(&["-c", "user.name=Me"]);
        let extra = opts(&["--depth", "1"]);
        let args = git_clone_args(&request(&git_opts, &extra));
        // The identity goes to git as well, so that submodules' clones have it too.
        assert_eq!(
            strings(args),
            [
                "-c",
                "user.name=Me",
                "clone",
                "--recursive",
                "-c",
                "user.name=Me",
                "--depth",
                "1",
                "--",
                "https://x/y.git",
                "/code/x/y"
            ]
        );
    }

    #[test]
    fn submodule_options_replace_recursive() {
        for opt in ["--bare", "--mirror", "--no-recurse-submodules", "--recurse-submodules=."] {
            let extra = opts(&[opt]);
            let args = git_clone_args(&request(&[], &extra));
            assert_eq!(strings(args), ["clone", opt, "--", "https://x/y.git", "/code/x/y"]);
        }
    }

    #[test]
    fn container_clones_pass_only_fetch_options_on() {
        let extra = opts(&["-c", "a.b=c", "--depth", "1", "--filter=blob:none", "-q"]);
        assert_eq!(
            container_fetch_opts(&extra).unwrap(),
            opts(&["--depth", "1", "--filter=blob:none", "-q"])
        );
        for (bad, msg) in [
            ("--branch", "--branch cannot be used with --container"),
            ("--origin=up", "--origin=up cannot be used with --container"),
            ("--depth", "--depth needs a value"),
        ] {
            let err = container_fetch_opts(&opts(&[bad])).unwrap_err();
            assert_eq!(err.to_string(), msg);
        }
    }

    #[test]
    fn reads_the_remote_head() {
        let sha1 = "03406f3589baf8a123e89b629c1da6abe360a63e";
        let head = parse_remote_head(&format!("ref: refs/heads/main\tHEAD\n{sha1}\tHEAD\n"));
        let head = head.unwrap();
        assert_eq!(head, RemoteHead { oid: sha1.into(), branch: Some("main".into()) });
        assert_eq!(head.object_format().unwrap(), "sha1");
        let sha256 = "0abb7a666d6e37a95a28b984f769f675cdb5ed63fddec81ff5e1c0cc0c45abcf";
        let head = parse_remote_head(&format!("{sha256}\tHEAD\n")).unwrap();
        assert_eq!(head.branch, None);
        assert_eq!(head.object_format().unwrap(), "sha256");
        // An empty repository, or one whose HEAD names a branch it lacks, prints nothing.
        assert_eq!(parse_remote_head(""), None);
        assert_eq!(parse_remote_head("ref: refs/heads/main\tHEAD\n"), None);
    }

    #[test]
    fn upstreams_are_fetched_with_the_same_history_limits() {
        let extra = opts(&[
            "-c",
            "a.b=c",
            "--branch",
            "dev",
            "--depth",
            "1",
            "--filter=blob:none",
            "--shallow-since=2020-01-01",
            "-q",
        ]);
        assert_eq!(
            history_opts(&extra),
            opts(&["--depth", "1", "--filter=blob:none", "--shallow-since=2020-01-01"])
        );
        assert_eq!(history_opts(&opts(&["--recursive"])), opts(&[]));
    }

    #[test]
    fn single_branch_follows_git_clone() {
        for (extra, want) in [
            (&[][..], false),
            (&["--filter=blob:none"], false),
            (&["--depth", "1"], true),
            (&["--depth=1"], true),
            (&["--shallow-since=2020-01-01"], true),
            (&["--shallow-exclude", "v1"], true),
            (&["--single-branch"], true),
            (&["--depth", "1", "--no-single-branch"], false),
            (&["--no-single-branch", "--single-branch"], true),
            (&["-c", "--depth=1"], false),
        ] {
            assert_eq!(single_branch(&opts(extra)), want, "{extra:?}");
        }
    }

    #[test]
    fn borrows_from_the_reference_store() {
        let mut req = request(&[], &[]);
        req.reference = Some(Path::new("/store"));
        // Submodules are cloned afterwards, so that they borrow from the store too.
        assert_eq!(
            strings(git_clone_args(&req)),
            [
                "clone",
                "--reference-if-able",
                "/store",
                "-c",
                "submodule.active=.",
                "--no-recurse-submodules",
                "--",
                "https://x/y.git",
                "/code/x/y"
            ]
        );

        // Only the refs of the upstreams that share the clone's history are negotiated with.
        let names = ["x/y".to_string(), "x/parent".to_string()];
        req.reference_names = &names;
        let args = strings(git_clone_args(&req));
        assert_eq!(
            args[1..5],
            [
                "--reference-if-able",
                "/store",
                "-c",
                "core.alternateRefsPrefixes=refs/remotes/x/y/ refs/tags/x/y/ \
                 refs/remotes/x/parent/ refs/tags/x/parent/"
            ]
        );
        assert!(plan(&req).split);
    }

    #[test]
    fn submodules_of_borrowing_clones_follow_the_options() {
        let store = Some(Path::new("/store"));
        let git_opts = opts(&["-c", "user.name=Me"]);
        let extra = opts(&[
            "--recurse-submodules=lib",
            "--filter=blob:none",
            "--also-filter-submodules",
            "--recursive=b",
        ]);
        let req = CloneRequest { reference: store, ..request(&git_opts, &extra) };
        assert_eq!(
            strings(git_clone_args(&req)),
            [
                "-c",
                "user.name=Me",
                "clone",
                "--reference-if-able",
                "/store",
                "-c",
                "submodule.active=b",
                "-c",
                "submodule.active=lib",
                "-c",
                "user.name=Me",
                "--recurse-submodules=lib",
                "--filter=blob:none",
                "--recursive=b",
                "--no-recurse-submodules",
                "--",
                "https://x/y.git",
                "/code/x/y"
            ]
        );

        // Without a filter, git is left to reject --also-filter-submodules itself.
        let extra = opts(&["--also-filter-submodules"]);
        let req = CloneRequest { reference: store, ..request(&[], &extra) };
        assert!(strings(git_clone_args(&req)).contains(&"--also-filter-submodules".to_string()));

        // No submodules, nothing to split: the store is simply borrowed from.
        for extra in [&["--no-recurse-submodules"][..], &["--bare"]] {
            let extra = opts(extra);
            let req = CloneRequest { reference: store, ..request(&[], &extra) };
            let args = strings(git_clone_args(&req));
            assert_eq!(
                args[..4],
                ["clone", "--reference-if-able", "/store", extra[0].to_str().unwrap()]
            );
            assert!(!plan(&req).split);
        }
    }

    #[test]
    fn forks_clone_their_submodules_after_the_upstream() {
        let mut req = request(&[], &[]);
        req.upstream_url = Some("https://x/parent.git");
        assert_eq!(
            strings(git_clone_args(&req)),
            [
                "clone",
                "-c",
                "submodule.active=.",
                "--no-recurse-submodules",
                "--",
                "https://x/y.git",
                "/code/x/y"
            ]
        );
        assert!(plan(&req).split && plan(&req).store.is_none());
        // With its own reference, git clones the submodules, through that reference.
        let extra = opts(&["--reference", "/other"]);
        let req =
            CloneRequest { upstream_url: Some("https://x/parent.git"), ..request(&[], &extra) };
        assert!(!plan(&req).split);
    }

    #[test]
    fn own_references_leave_recursive_clones_to_git() {
        let extra = opts(&["--reference", "/other"]);
        let req = CloneRequest { reference: Some(Path::new("/store")), ..request(&[], &extra) };
        assert_eq!(
            strings(git_clone_args(&req)),
            ["clone", "--recursive", "--reference", "/other", "--", "https://x/y.git", "/code/x/y"]
        );
        // Without submodules, the store and the user's references go together.
        let extra = opts(&["--reference-if-able=/other", "--no-recurse-submodules"]);
        let req = CloneRequest { reference: Some(Path::new("/store")), ..request(&[], &extra) };
        assert_eq!(strings(git_clone_args(&req))[1..3], ["--reference-if-able", "/store"]);
    }
}
