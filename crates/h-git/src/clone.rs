//! Cloning repositories with `git`: optionally borrowing objects from a store, laying the clone
//! out as a bare repository plus worktrees, and wiring up a fork's upstream so it can be pulled
//! from but never pushed to.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::process::Stdio;

use crate::git::{self, GitError, config_pairs, non_config_opts};

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
    /// Lay the clone out as `<path>/.bare` plus a `.git` file, with no working tree, so that all
    /// work happens in worktrees under `<path>`.
    pub container: bool,
    /// The URL of the repository this one is a fork of.
    pub upstream_url: Option<&'a str>,
}

/// Whether any option already decides submodule handling or precludes it.
fn decides_submodules(opts: &[OsString]) -> bool {
    opts.iter().any(|opt| {
        let b = opt.as_bytes();
        b == b"--bare"
            || b == b"--mirror"
            || b == b"--recursive"
            || b == b"--no-recursive"
            || b.starts_with(b"--recurse-submodules")
            || b.starts_with(b"--no-recurse-submodules")
    })
}

/// Arguments to pass to `git` for a plain (non-container) clone.
///
/// Submodules are cloned too unless an option says otherwise, and objects are borrowed from the
/// reference store when there is one.
pub fn git_clone_args(req: &CloneRequest) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["clone".into()];
    if !decides_submodules(req.git_opts) && !decides_submodules(req.extra) {
        args.push("--recursive".into());
    }
    if let Some(reference) = req.reference {
        args.push("--reference-if-able".into());
        args.push(reference.into());
    }
    args.extend(req.git_opts.iter().cloned());
    args.extend(req.extra.iter().cloned());
    args.push("--".into());
    args.push(req.url.into());
    args.push(req.path.into());
    args
}

/// Clone as `req` asks, creating parent directories. Returns git's exit status, 0 on success.
pub fn clone_repo(req: &CloneRequest) -> u8 {
    if let Some(parent) = req.path.parent() {
        // Any failure here is reported by git itself.
        let _ = std::fs::create_dir_all(parent);
    }
    let result = if req.container { clone_container(req) } else { clone_plain(req) };
    let result = result.and_then(|()| match req.upstream_url {
        Some(url) => add_upstream(&git_dir(req), url, &history_opts(req.extra)),
        None => Ok(()),
    });
    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            err.code()
        }
    }
}

/// The directory to run `git -C` in once the clone exists.
fn git_dir(req: &CloneRequest) -> std::path::PathBuf {
    if req.container { req.path.join(BARE_DIR) } else { req.path.to_path_buf() }
}

fn clone_plain(req: &CloneRequest) -> Result<(), GitError> {
    git::run(None, &git_clone_args(req))
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
    for (key, value) in config {
        git::run(Some(&bare), &[OsString::from("config"), key, value])?;
    }
    if let Some(reference) = req.reference.filter(|r| r.join("objects").is_dir()) {
        let alternates = bare.join("objects/info/alternates");
        let mut line = reference.join("objects").into_os_string().into_vec();
        line.push(b'\n');
        std::fs::write(&alternates, line).map_err(GitError::Spawn)?;
    }
    git::run(Some(&bare), &["remote", "add", "origin", req.url])?;
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
/// and make `origin` the default push target. `fetch_opts` (such as `--depth 1`) are passed to
/// the first fetch.
pub fn add_upstream(dir: &Path, url: &str, fetch_opts: &[OsString]) -> Result<(), GitError> {
    let dir = Some(dir);
    git::run(dir, &["remote", "add", "upstream", url])?;
    git::run(dir, &["config", "remote.upstream.pushurl", NO_PUSH])?;
    git::run(dir, &["config", "remote.upstream.tagOpt", "--no-tags"])?;
    git::run(dir, &["config", "remote.pushDefault", "origin"])?;
    let mut fetch: Vec<OsString> = vec!["fetch".into(), "--quiet".into()];
    fetch.extend(fetch_opts.iter().cloned());
    fetch.push("upstream".into());
    git::run(dir, &fetch)
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
        assert_eq!(
            strings(args),
            [
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
    fn borrows_from_the_reference_store() {
        let mut req = request(&[], &[]);
        req.reference = Some(Path::new("/store"));
        assert_eq!(
            strings(git_clone_args(&req)),
            [
                "clone",
                "--recursive",
                "--reference-if-able",
                "/store",
                "--",
                "https://x/y.git",
                "/code/x/y"
            ]
        );
    }
}
