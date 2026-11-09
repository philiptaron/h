//! Searching the code root for a project directory by name.

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::resolve::is_simple_name;

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

/// List the terms that complete `word` to a directory below `root`, best first.
///
/// Each is the end of a directory's path, with as many segments as `word` has, spelled as the
/// directory is: every segment of `word` but the last is a whole segment of it, and the last
/// starts the last. Case is ignored, so `nixos/n` completes to `NixOS/nixpkgs`. The terms with
/// the fewest letters whose case differs from `word` come first, then the rest in sorted order.
/// Directories are the ones [`search`] looks through, and only those spelled as a term can be
/// are offered.
pub fn list_completions(root: impl AsRef<Path>, word: &str) -> Vec<String> {
    let wanted = word.split('/').count();
    let mut found: Vec<(usize, String)> = list_projects(root)
        .into_iter()
        .filter_map(|segments| {
            let tail = &segments[segments.len().checked_sub(wanted)?..];
            let tail: Vec<&str> = tail.iter().map(|s| s.to_str()).collect::<Option<_>>()?;
            if !tail.iter().all(|s| is_simple_name(s)) {
                return None;
            }
            let term = tail.join("/");
            let start = term.as_bytes().get(..word.len())?;
            if !start.eq_ignore_ascii_case(word.as_bytes()) {
                return None;
            }
            let differences = start.iter().zip(word.as_bytes()).filter(|(a, b)| a != b).count();
            Some((differences, term))
        })
        .collect();
    found.sort();
    found.dedup();
    found.into_iter().map(|(_, term)| term).collect()
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
        assert!(list_completions("/nonexistent/h-test-root", "").is_empty());
    }

    #[test]
    fn completes_names_and_paths_in_any_case() {
        let tmp = tree(&[
            "github.com/NixOS/nixpkgs",
            "github.com/NixOS/nix",
            "github.com/nixos/mobile",
            "github.com/NixOS/marketing",
            "git.kernel.org/pub/scm/torvalds/linux/.git",
            "gitlab.com/x/nixpkgs",
        ]);
        if fs::read_dir(tmp.path().join("github.com")).unwrap().count() < 2 {
            return; // a case-insensitive filesystem cannot hold both NixOS and nixos
        }
        let complete = |word| list_completions(tmp.path(), word);
        assert_eq!(complete("nixp"), ["nixpkgs"]);
        assert_eq!(complete("NIX"), ["NixOS", "nix", "nixos", "nixpkgs"]);
        assert_eq!(complete("nixos/nixp"), ["NixOS/nixpkgs"]);
        assert_eq!(complete("github.com/nixos/nixp"), ["github.com/NixOS/nixpkgs"]);
        assert_eq!(complete("torvalds/l"), ["torvalds/linux"]);
        assert_eq!(complete("scm/torvalds/"), ["scm/torvalds/linux"]);
        assert_eq!(complete("pub/torvalds/"), Vec::<String>::new());
        // Nothing inside a checkout is offered.
        assert_eq!(complete("linux/"), Vec::<String>::new());
        // The fewest letters in another case first.
        assert_eq!(complete("nixos/m"), ["nixos/mobile", "NixOS/marketing"]);
        assert_eq!(complete("NixOS/m"), ["NixOS/marketing", "nixos/mobile"]);
    }

    #[test]
    fn completes_only_what_a_term_can_spell() {
        let tmp = tree(&["a/with space", "a/caf\u{e9}", "a/fine", "a/.hidden"]);
        assert_eq!(list_completions(tmp.path(), "a/"), ["a/fine"]);
        assert_eq!(list_completions(tmp.path(), ""), ["a", "fine"]);
    }
}
