//! Cloning repositories with `git`.

use std::ffi::OsString;
use std::path::Path;

use crate::git;

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

/// Clone `url` into `path`, creating parent directories. Returns git's exit status, 0 on success.
pub fn clone_repo(url: &str, path: &Path, extra: &[OsString]) -> u8 {
    if let Some(parent) = path.parent() {
        // Any failure here is reported by git itself.
        let _ = std::fs::create_dir_all(parent);
    }
    match git::run(None, &git_clone_args(url, path, extra)) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            err.code()
        }
    }
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
