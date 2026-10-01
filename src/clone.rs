//! Cloning repositories with `git`.

use std::ffi::OsString;
use std::os::fd::AsFd;
use std::path::Path;
use std::process::{Command, Stdio};

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

/// Clone `url` into `path`, creating parent directories. Returns git's exit status.
///
/// Git's stdout is sent to stderr so that stdout carries only the directory to `cd` into.
pub fn clone_repo(url: &str, path: &Path, extra: &[OsString]) -> u8 {
    if let Some(parent) = path.parent() {
        // Any failure here is reported by git itself.
        let _ = std::fs::create_dir_all(parent);
    }

    let stdout = match std::io::stderr().as_fd().try_clone_to_owned() {
        Ok(fd) => Stdio::from(fd),
        Err(_) => Stdio::null(),
    };
    let status = Command::new("git").args(git_clone_args(url, path, extra)).stdout(stdout).status();
    match status {
        Ok(status) => status.code().map_or(1, |code| code as u8),
        Err(err) => {
            eprintln!("failed to run git: {err}");
            127
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
