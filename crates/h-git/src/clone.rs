//! Cloning repositories with `git`, and wiring up a fork's upstream so it can be pulled from but
//! never pushed to.

use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::git::{self, GitError};

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

/// Arguments to pass to `git` to clone as `req` asks.
///
/// Submodules are cloned too unless an option says otherwise.
pub fn git_clone_args(req: &CloneRequest) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["clone".into()];
    if !decides_submodules(req.git_opts) && !decides_submodules(req.extra) {
        args.push("--recursive".into());
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
    let result = git::run(None, &git_clone_args(req));
    let result = result.and_then(|()| match req.upstream_url {
        Some(url) => add_upstream(req.path, url),
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
}
