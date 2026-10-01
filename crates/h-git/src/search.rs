//! Searching the code root for a project directory by name.

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// How many directory levels below the code root are searched.
pub const MAX_DEPTH: usize = 3;

/// Find a directory named `term` up to [`MAX_DEPTH`] levels below `root`.
///
/// The match is case-insensitive unless `term` contains an uppercase letter. Hidden directories
/// are skipped. Deeper matches win, since `<domain>/<owner>/<repo>` is the usual layout; among
/// matches at the same depth, the first in sorted order wins.
pub fn search(root: &Path, term: &str) -> Option<PathBuf> {
    let case_sensitive = term.bytes().any(|b| b.is_ascii_uppercase());
    let mut best = None;
    walk(root, term.as_bytes(), case_sensitive, 1, &mut best);
    best.map(|(_, path)| path)
}

fn walk(
    dir: &Path,
    term: &[u8],
    case_sensitive: bool,
    depth: usize,
    best: &mut Option<(usize, PathBuf)>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.file_name())).collect();
    names.sort();

    for name in names {
        let name_bytes = OsStr::as_bytes(&name);
        if name_bytes.starts_with(b".") {
            continue;
        }
        let path = dir.join(&name);
        // Follows symlinks, so linked projects are found too.
        if !path.is_dir() {
            continue;
        }
        let matches =
            if case_sensitive { name_bytes == term } else { name_bytes.eq_ignore_ascii_case(term) };
        if matches && best.as_ref().is_none_or(|(d, _)| depth > *d) {
            *best = Some((depth, path.clone()));
        }
        walk(&path, term, case_sensitive, depth + 1, best);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(dirs: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for dir in dirs {
            fs::create_dir_all(tmp.path().join(dir)).unwrap();
        }
        tmp
    }

    #[test]
    fn finds_at_each_depth() {
        let tmp = tree(&["one", "a/two", "a/b/three"]);
        assert_eq!(search(tmp.path(), "one"), Some(tmp.path().join("one")));
        assert_eq!(search(tmp.path(), "two"), Some(tmp.path().join("a/two")));
        assert_eq!(search(tmp.path(), "three"), Some(tmp.path().join("a/b/three")));
    }

    #[test]
    fn does_not_search_past_max_depth() {
        let tmp = tree(&["a/b/c/four"]);
        assert_eq!(search(tmp.path(), "four"), None);
    }

    #[test]
    fn prefers_deeper_matches() {
        let tmp = tree(&["proj", "x/proj", "y/z/proj"]);
        assert_eq!(search(tmp.path(), "proj"), Some(tmp.path().join("y/z/proj")));
    }

    #[test]
    fn ties_are_broken_by_sorted_order() {
        let tmp = tree(&["b/c/proj", "a/c/proj", "c/c/proj"]);
        assert_eq!(search(tmp.path(), "proj"), Some(tmp.path().join("a/c/proj")));
    }

    #[test]
    fn lowercase_terms_match_any_case() {
        let tmp = tree(&["github.com/owner/MyProj"]);
        let want = Some(tmp.path().join("github.com/owner/MyProj"));
        assert_eq!(search(tmp.path(), "myproj"), want);
    }

    #[test]
    fn uppercase_terms_match_exactly() {
        let tmp = tree(&["a/b/myproj", "a/MyProj"]);
        assert_eq!(search(tmp.path(), "MyProj"), Some(tmp.path().join("a/MyProj")));
        assert_eq!(search(tmp.path(), "MYPROJ"), None);
    }

    #[test]
    fn skips_hidden_directories() {
        let tmp = tree(&[".hidden/proj", ".proj", "a/.git"]);
        assert_eq!(search(tmp.path(), "proj"), None);
        assert_eq!(search(tmp.path(), ".proj"), None);
    }

    #[test]
    fn ignores_files() {
        let tmp = tree(&["a"]);
        fs::write(tmp.path().join("a/proj"), "").unwrap();
        assert_eq!(search(tmp.path(), "proj"), None);
    }

    #[test]
    fn follows_symlinks() {
        let tmp = tree(&["real/proj", "a"]);
        std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("a/link")).unwrap();
        assert_eq!(search(tmp.path(), "proj"), Some(tmp.path().join("a/link/proj")));
    }

    #[test]
    fn missing_root_finds_nothing() {
        assert_eq!(search(Path::new("/nonexistent/h-test-root"), "proj"), None);
    }
}
