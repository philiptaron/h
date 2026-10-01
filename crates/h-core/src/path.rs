//! Interpreting paths given on the command line.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

/// Expand a leading `~` (alone or followed by `/`) to `$HOME`.
pub fn expand_tilde(path: &OsStr) -> PathBuf {
    expand_tilde_with(path, std::env::var_os("HOME").as_deref())
}

/// Like [`expand_tilde`], with an explicit home directory.
///
/// `~user` forms are left untouched, as is everything when `home` is `None`.
pub fn expand_tilde_with(path: &OsStr, home: Option<&OsStr>) -> PathBuf {
    let bytes = path.as_bytes();
    match (bytes, home) {
        ([b'~'], Some(home)) => PathBuf::from(home),
        ([b'~', b'/', rest @ ..], Some(home)) => {
            let mut out = home.as_bytes().to_vec();
            out.push(b'/');
            out.extend_from_slice(rest);
            PathBuf::from(OsString::from_vec(out))
        }
        _ => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(path: &str, home: Option<&str>) -> PathBuf {
        expand_tilde_with(OsStr::new(path), home.map(OsStr::new))
    }

    #[test]
    fn expands_bare_tilde() {
        assert_eq!(expand("~", Some("/home/me")), PathBuf::from("/home/me"));
    }

    #[test]
    fn expands_tilde_slash() {
        assert_eq!(expand("~/src", Some("/home/me")), PathBuf::from("/home/me/src"));
        assert_eq!(expand("~/", Some("/home/me")), PathBuf::from("/home/me/"));
    }

    #[test]
    fn leaves_other_paths_alone() {
        assert_eq!(expand("/abs/~/x", Some("/home/me")), PathBuf::from("/abs/~/x"));
        assert_eq!(expand("rel", Some("/home/me")), PathBuf::from("rel"));
        assert_eq!(expand("~other/src", Some("/home/me")), PathBuf::from("~other/src"));
    }

    #[test]
    fn leaves_tilde_without_home() {
        assert_eq!(expand("~/src", None), PathBuf::from("~/src"));
    }
}
