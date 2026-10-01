//! Cloning repositories with `git`: optionally borrowing objects from a store, laying the clone
//! out as a bare repository plus worktrees, and wiring up a fork's upstream so it can be pulled
//! from but never pushed to.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

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
        Some(url) => add_upstream(&git_dir(req), url),
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

/// Create `<path>/.bare` by fetching into a fresh bare repository, then point `<path>/.git` at
/// it. Unlike `git clone --bare`, the result has `origin` with ordinary remote-tracking branches,
/// so worktrees are added from `origin/<branch>` and local branches are only ever the user's.
/// HEAD is detached at `origin/HEAD`, so a new branch made by `git worktree add <dir>` starts
/// from the default branch instead of being an orphan of the branch HEAD names but nobody has.
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
    git::run(
        None,
        &[OsString::from("init"), "--bare".into(), "--quiet".into(), bare.clone().into()],
    )?;
    let mut opts = req.git_opts.to_vec();
    opts.extend(req.extra.iter().cloned());
    for (key, value) in config_pairs(&opts) {
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
    git::run(Some(&bare), &["remote", "set-head", "origin", "--auto"])?;
    git::run(Some(&bare), &["update-ref", "--no-deref", "HEAD", "refs/remotes/origin/HEAD"])?;
    std::fs::write(req.path.join(".git"), format!("gitdir: ./{BARE_DIR}\n"))
        .map_err(GitError::Spawn)
}

/// Add `url` as the `upstream` remote of the repository at `dir`, fetchable but not pushable,
/// and make `origin` the default push target.
pub fn add_upstream(dir: &Path, url: &str) -> Result<(), GitError> {
    let dir = Some(dir);
    git::run(dir, &["remote", "add", "upstream", url])?;
    git::run(dir, &["config", "remote.upstream.pushurl", NO_PUSH])?;
    git::run(dir, &["config", "remote.upstream.tagOpt", "--no-tags"])?;
    git::run(dir, &["config", "remote.pushDefault", "origin"])?;
    git::run(dir, &["fetch", "--quiet", "upstream"])
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
