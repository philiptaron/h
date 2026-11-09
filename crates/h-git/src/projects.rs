//! The project directories under a code root, and finding them by search term.

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::resolve::is_simple_name;

/// The directories under a code root that a term can name: its repositories, and the directories
/// that hold them.
///
/// Hidden directories are left out, and so are those with no repository in them, such as a
/// Python virtualenv or an empty directory. What is inside a repository is left out too: its own
/// directories are not projects, and not looking keeps the walk small however deep the code root
/// goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projects {
    root: PathBuf,
    /// In sorted order of their paths.
    paths: Vec<Project>,
}

/// A directory a term can name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Project {
    /// Its path relative to the code root, split into segments.
    segments: Vec<OsString>,
    /// Whether it is a repository, rather than a directory that holds them.
    is_repository: bool,
}

impl Projects {
    /// Walk `root` for its projects. Symbolic links are followed, except back into a directory
    /// the walk is already in. A missing `root` has none.
    pub fn walk(root: impl AsRef<Path>) -> Projects {
        let root = root.as_ref().to_path_buf();
        let mut paths = Vec::new();
        if let Ok(meta) = fs::metadata(&root) {
            let mut ancestors = vec![(meta.dev(), meta.ino())];
            walk(&root, &mut Vec::new(), &mut ancestors, &mut paths);
        }
        Projects { root, paths }
    }

    /// Find the directory `term` names.
    ///
    /// A term is a name, or several joined by `/`, and names each directory whose path ends in
    /// those segments, in any case: any directory can be where it starts, so `nixos/nixpkgs`
    /// names `github.com/NixOS/nixpkgs`. With a `/` at the end, it names only the directories
    /// that hold repositories, so `alacritty/` is the owner when `alacritty` is its repository,
    /// as in `github.com/alacritty/alacritty`. When several match, a term with a capital letter in it
    /// prefers those in its own case. Then deeper matches win, since `<domain>/<owner>/<repo>` is
    /// the usual layout; among those as deep, one in the term's case, and then the first in
    /// sorted order.
    pub fn find(&self, term: &str) -> Option<PathBuf> {
        let (term, holders_only) = match term.strip_suffix('/') {
            Some(term) => (term, true),
            None => (term, false),
        };
        let has_capitals = term.bytes().any(|b| b.is_ascii_uppercase());
        let wanted: Vec<&str> = term.split('/').collect();
        // `max_by_key` keeps the last of equals, so going backwards keeps the first in sorted order.
        let best = self.paths.iter().rev().filter_map(|project| {
            if holders_only && project.is_repository {
                return None;
            }
            let segments = &project.segments;
            let tail = &segments[segments.len().checked_sub(wanted.len())?..];
            let pairs = || tail.iter().map(|name| name.as_bytes()).zip(&wanted);
            if !pairs().all(|(name, wanted)| name.eq_ignore_ascii_case(wanted.as_bytes())) {
                return None;
            }
            let exact = pairs().all(|(name, wanted)| name == wanted.as_bytes());
            Some(((has_capitals && exact, segments.len(), exact), segments))
        });
        let (_, segments) = best.max_by_key(|(rank, _)| *rank)?;
        Some(segments.iter().fold(self.root.clone(), |path, s| path.join(s)))
    }

    /// List the terms that complete `word`, best first.
    ///
    /// Each is the end of a project's path, with as many segments as `word` has, spelled as the
    /// directory is: every segment of `word` but the last is a whole segment of it, and the last
    /// starts the last. Case is ignored, so `nixos/n` completes to `NixOS/nixpkgs`. The terms
    /// with the fewest letters whose case differs from `word` come first, then the rest in sorted
    /// order. Only what can be spelled as a term is offered.
    pub fn list_completions(&self, word: &str) -> Vec<String> {
        let wanted = word.split('/').count();
        let mut found: Vec<(usize, String)> = self
            .paths
            .iter()
            .filter_map(|Project { segments, .. }| {
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
}

/// Add the projects below `dir`, which is at `segments` below the root, to `found`, and say
/// whether `dir` is a repository or holds one.
/// What a directory the walk visits turns out to be.
enum Visited {
    Repository,
    /// A directory holding repositories.
    Holder,
    /// Neither, so not a project.
    Nothing,
}

fn walk(
    dir: &Path,
    segments: &mut Vec<OsString>,
    ancestors: &mut Vec<(u64, u64)>,
    found: &mut Vec<Project>,
) -> Visited {
    let Ok(entries) = fs::read_dir(dir) else {
        return Visited::Nothing;
    };
    let mut names: Vec<OsString> = entries.filter_map(|e| e.ok().map(|e| e.file_name())).collect();
    if !segments.is_empty() && is_repository(&names) {
        return Visited::Repository;
    }
    names.sort();

    let mut holds_repository = false;
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
        let at = found.len();
        found.push(Project { segments: segments.clone(), is_repository: false });
        ancestors.push(id);
        match walk(&path, segments, ancestors, found) {
            Visited::Repository => found[at].is_repository = true,
            Visited::Holder => {}
            Visited::Nothing => found.truncate(at),
        }
        holds_repository |= found.len() > at;
        ancestors.pop();
        segments.pop();
    }
    if holds_repository { Visited::Holder } else { Visited::Nothing }
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

    fn search(root: impl AsRef<Path>, term: &str) -> Option<PathBuf> {
        Projects::walk(root).find(term)
    }

    fn list_completions(root: impl AsRef<Path>, word: &str) -> Vec<String> {
        Projects::walk(root).list_completions(word)
    }

    /// A code root with a checkout at each of `checkouts`.
    fn tree(checkouts: &[&str]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        for dir in checkouts {
            fs::create_dir_all(tmp.path().join(dir).join(".git")).unwrap();
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
        let tmp = tree(&["a/repo", "a/repo/sub"]);
        for dir in ["hg/.hg", "hg/sub/.git", "jj/.jj", "jj/sub/.git", "bare.git/objects/x/.git"] {
            fs::create_dir_all(tmp.path().join(dir)).unwrap();
        }
        fs::create_dir_all(tmp.path().join("bare.git/refs")).unwrap();
        fs::write(tmp.path().join("bare.git/HEAD"), "ref: refs/heads/main\n").unwrap();
        assert_eq!(search(tmp.path(), "repo"), Some(tmp.path().join("a/repo")));
        assert_eq!(search(tmp.path(), "sub"), None);
        assert_eq!(search(tmp.path(), "objects"), None);
        // A worktree or submodule has a `.git` file instead of a directory.
        let tmp = tree(&["wt/sub"]);
        fs::write(tmp.path().join("wt/.git"), "gitdir: elsewhere\n").unwrap();
        assert_eq!(search(tmp.path(), "wt"), Some(tmp.path().join("wt")));
        assert_eq!(search(tmp.path(), "sub"), None);
    }

    #[test]
    fn survives_symlink_loops() {
        let tmp = tree(&["a/b"]);
        std::os::unix::fs::symlink(tmp.path().join("a"), tmp.path().join("a/up")).unwrap();
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
        assert_eq!(search(tmp.path(), "NIXOS/nixpkgs"), nixpkgs);
        let linux = Some(tmp.path().join("git.kernel.org/pub/scm/torvalds/linux"));
        assert_eq!(search(tmp.path(), "torvalds/linux"), linux);
        assert_eq!(search(tmp.path(), "scm/torvalds/linux"), linux);
        assert_eq!(search(tmp.path(), "pub/torvalds/linux"), None);
        assert_eq!(search(tmp.path(), "nixos/nixpkgs/more"), None);
    }

    #[test]
    fn a_trailing_slash_names_a_directory_holding_repositories() {
        let tmp = tree(&["github.com/alacritty/alacritty", "gitlab.com/group/alacritty"]);
        let owner = Some(tmp.path().join("github.com/alacritty"));
        assert_eq!(
            search(tmp.path(), "alacritty"),
            Some(tmp.path().join("github.com/alacritty/alacritty"))
        );
        assert_eq!(search(tmp.path(), "alacritty/"), owner);
        assert_eq!(search(tmp.path(), "github.com/alacritty/"), owner);
        assert_eq!(search(tmp.path(), "github.com/"), Some(tmp.path().join("github.com")));
        assert_eq!(search(tmp.path(), "group/alacritty/"), None);
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
    fn terms_with_capitals_prefer_their_case() {
        let tmp = tree(&["a/b/myproj", "a/MyProj"]);
        assert_eq!(search(tmp.path(), "MyProj"), Some(tmp.path().join("a/MyProj")));
        assert_eq!(search(tmp.path(), "MYPROJ"), Some(tmp.path().join("a/b/myproj")));
    }

    #[test]
    fn as_deep_matches_prefer_the_terms_case() {
        let tmp = tree(&["github.com/NixOS/nixpkgs", "github.com/nixos/nixpkgs"]);
        if fs::read_dir(tmp.path().join("github.com")).unwrap().count() < 2 {
            return; // a case-insensitive filesystem cannot hold both
        }
        let nixos = Some(tmp.path().join("github.com/nixos/nixpkgs"));
        let upper = Some(tmp.path().join("github.com/NixOS/nixpkgs"));
        assert_eq!(search(tmp.path(), "nixos/nixpkgs"), nixos);
        assert_eq!(search(tmp.path(), "NixOS/nixpkgs"), upper);
        // In no one's case, the first in sorted order, which puts capitals first.
        assert_eq!(search(tmp.path(), "nixOS/nixpkgs"), upper);
    }

    #[test]
    fn skips_hidden_directories() {
        let tmp = tree(&[".hidden/proj", ".proj", "a/.git"]);
        assert_eq!(search(tmp.path(), "proj"), None);
        assert_eq!(search(tmp.path(), ".proj"), None);
    }

    #[test]
    fn ignores_files() {
        let tmp = tree(&["a/b"]);
        fs::write(tmp.path().join("a/proj"), "").unwrap();
        assert_eq!(search(tmp.path(), "proj"), None);
    }

    #[test]
    fn follows_symlinks() {
        let tmp = tree(&["real/proj"]);
        fs::create_dir(tmp.path().join("a")).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("a/link")).unwrap();
        assert_eq!(search(tmp.path(), "proj"), Some(tmp.path().join("a/link/proj")));
    }

    #[test]
    fn leaves_out_directories_without_repositories() {
        let tmp = tree(&["github.com/owner/proj"]);
        for dir in ["venv/lib/python3/site-packages/pip", "github.com/owner/empty", "scratch"] {
            fs::create_dir_all(tmp.path().join(dir)).unwrap();
        }
        assert_eq!(search(tmp.path(), "pip"), None);
        assert_eq!(search(tmp.path(), "owner/empty"), None);
        assert_eq!(list_completions(tmp.path(), ""), ["github.com", "owner", "proj"]);
        assert_eq!(list_completions(tmp.path(), "owner/"), ["owner/proj"]);
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
            "git.kernel.org/pub/scm/torvalds/linux",
            "git.kernel.org/pub/scm/torvalds/linux/drivers",
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
