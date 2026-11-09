//! Searching the code root for a project directory by name.

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Find the directory `term` names below `root`, at any depth.
///
/// A term is a name, or several joined by `/`, and names each directory whose path ends in those
/// segments: any directory can be where it starts, so `nixos/nixpkgs` names
/// `github.com/NixOS/nixpkgs`. The match is case-insensitive unless `term` contains an uppercase
/// letter. Hidden directories are skipped, and so is everything inside a repository. Deeper
/// matches win, since `<domain>/<owner>/<repo>` is the usual layout; among matches at the same
/// depth, the first in sorted order wins.
pub fn search(root: impl AsRef<Path>, term: &str) -> Option<PathBuf> {
    let root = root.as_ref();
    let case_sensitive = term.bytes().any(|b| b.is_ascii_uppercase());
    let same = |name: &[u8], wanted: &str| match case_sensitive {
        true => name == wanted.as_bytes(),
        false => name.eq_ignore_ascii_case(wanted.as_bytes()),
    };
    let wanted: Vec<&str> = term.split('/').collect();
    let mut best: Option<&Vec<OsString>> = None;
    let projects = list_projects(root);
    for segments in &projects {
        let Some(tail) = segments.len().checked_sub(wanted.len()).map(|i| &segments[i..]) else {
            continue;
        };
        let matches = tail.iter().zip(&wanted).all(|(name, wanted)| same(name.as_bytes(), wanted));
        if matches && best.is_none_or(|b| segments.len() > b.len()) {
            best = Some(segments);
        }
    }
    best.map(|segments| segments.iter().fold(root.to_path_buf(), |path, s| path.join(s)))
}

/// List the directories below `root` a term can name, each as its path relative to `root` split
/// into segments, in sorted order.
///
/// Hidden directories are skipped. A repository is listed, but not what is inside it: its own
/// directories are not projects, and skipping them keeps the walk small however deep the code
/// root goes. Symbolic links are followed, except back into a directory the walk is already in.
fn list_projects(root: impl AsRef<Path>) -> Vec<Vec<OsString>> {
    let root = root.as_ref();
    let mut found = Vec::new();
    let Ok(meta) = fs::metadata(root) else {
        return found;
    };
    let mut ancestors = vec![(meta.dev(), meta.ino())];
    walk(root, &mut Vec::new(), &mut ancestors, &mut found);
    found
}

fn walk(
    dir: &Path,
    segments: &mut Vec<OsString>,
    ancestors: &mut Vec<(u64, u64)>,
    found: &mut Vec<Vec<OsString>>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<OsString> = entries.filter_map(|e| e.ok().map(|e| e.file_name())).collect();
    if !segments.is_empty() && is_repository(&names) {
        return;
    }
    names.sort();

    for name in names {
        if name.as_bytes().starts_with(b".") {
            continue;
        }
        let path = dir.join(&name);
        // Follows symlinks, so linked projects are found too.
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };
        let id = (meta.dev(), meta.ino());
        if !meta.is_dir() || ancestors.contains(&id) {
            continue;
        }
        segments.push(name);
        found.push(segments.clone());
        ancestors.push(id);
        walk(&path, segments, ancestors, found);
        ancestors.pop();
        segments.pop();
    }
}

/// Whether a directory holding `names` is a repository: a git, Mercurial or Jujutsu checkout, or
/// a bare git repository.
fn is_repository(names: &[OsString]) -> bool {
    let has = |name: &str| names.iter().any(|n| n == name);
    has(".git") || has(".hg") || has(".jj") || (has("HEAD") && has("objects") && has("refs"))
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
    fn searches_at_any_depth() {
        let tmp = tree(&["a/b/c/d/e/six"]);
        assert_eq!(search(tmp.path(), "six"), Some(tmp.path().join("a/b/c/d/e/six")));
    }

    #[test]
    fn does_not_search_inside_repositories() {
        let tmp = tree(&["a/repo/.git", "a/repo/sub", "hg/.hg", "hg/sub", "jj/.jj", "jj/sub"]);
        fs::create_dir_all(tmp.path().join("bare.git/objects")).unwrap();
        fs::create_dir_all(tmp.path().join("bare.git/refs")).unwrap();
        fs::write(tmp.path().join("bare.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(search(tmp.path(), "repo"), Some(tmp.path().join("a/repo")));
        assert_eq!(search(tmp.path(), "sub"), None);
        assert_eq!(search(tmp.path(), "objects"), None);
        // A worktree or submodule has a `.git` file instead of a directory.
        let tmp = tree(&["wt/sub"]);
        fs::write(tmp.path().join("wt/.git"), "gitdir: elsewhere\n").unwrap();
        assert_eq!(search(tmp.path(), "sub"), None);
    }

    #[test]
    fn survives_symlink_loops() {
        let tmp = tree(&["a/b"]);
        std::os::unix::fs::symlink(tmp.path().join("a"), tmp.path().join("a/b/up")).unwrap();
        assert_eq!(search(tmp.path(), "b"), Some(tmp.path().join("a/b")));
        assert_eq!(search(tmp.path(), "nope"), None);
    }

    #[test]
    fn finds_paths_starting_at_any_directory() {
        let tmp = tree(&["github.com/NixOS/nixpkgs", "git.kernel.org/pub/scm/torvalds/linux"]);
        let nixpkgs = Some(tmp.path().join("github.com/NixOS/nixpkgs"));
        assert_eq!(search(tmp.path(), "nixos/nixpkgs"), nixpkgs);
        assert_eq!(search(tmp.path(), "github.com/nixos/nixpkgs"), nixpkgs);
        assert_eq!(search(tmp.path(), "NixOS/nixpkgs"), nixpkgs);
        assert_eq!(search(tmp.path(), "NIXOS/nixpkgs"), None);
        let linux = Some(tmp.path().join("git.kernel.org/pub/scm/torvalds/linux"));
        assert_eq!(search(tmp.path(), "torvalds/linux"), linux);
        assert_eq!(search(tmp.path(), "scm/torvalds/linux"), linux);
        assert_eq!(search(tmp.path(), "pub/torvalds/linux"), None);
        assert_eq!(search(tmp.path(), "nixos/nixpkgs/more"), None);
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
        assert_eq!(search("/nonexistent/h-test-root", "proj"), None);
    }
}
