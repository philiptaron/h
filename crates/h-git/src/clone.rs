//! Cloning repositories with `git`, and wiring up a fork's upstream so it can be pulled from but
//! never pushed to.

use std::ffi::OsString;
use std::path::Path;

use crate::git::{self, GitError};

/// The push URL that makes every push to a remote fail.
pub const NO_PUSH: &str = "no_push";

/// Arguments to pass to `git` to clone `url` into `path`.
///
/// `--recursive` is added only when the caller supplies no options of their own.
pub fn git_clone_args(url: &str, path: &Path, extra: &[OsString]) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["clone".into()];
    if extra.is_empty() {
        args.push("--recursive".into());
    }
    args.extend(extra.iter().cloned());
    args.push("--".into());
    args.push(url.into());
    args.push(path.into());
    args
}

/// Clone `url` into `path`, creating parent directories, and add `upstream_url`, the repository
/// it is a fork of, as its upstream. Returns git's exit status, 0 on success.
pub fn clone_repo(url: &str, path: &Path, extra: &[OsString], upstream_url: Option<&str>) -> u8 {
    if let Some(parent) = path.parent() {
        // Any failure here is reported by git itself.
        let _ = std::fs::create_dir_all(parent);
    }
    let result = git::run(None, &git_clone_args(url, path, extra));
    let result = result.and_then(|()| match upstream_url {
        Some(upstream_url) => add_upstream(path, upstream_url),
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

    #[test]
    fn defaults_to_recursive() {
        let args = git_clone_args("https://x/y.git", Path::new("/code/x/y"), &[]);
        assert_eq!(strings(args), ["clone", "--recursive", "--", "https://x/y.git", "/code/x/y"]);
    }

    #[test]
    fn extra_options_replace_recursive() {
        let extra = ["--depth".into(), "1".into()];
        let args = git_clone_args("https://x/y.git", Path::new("/code/x/y"), &extra);
        assert_eq!(strings(args), ["clone", "--depth", "1", "--", "https://x/y.git", "/code/x/y"]);
    }
}
