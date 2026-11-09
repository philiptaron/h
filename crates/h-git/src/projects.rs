//! The project directories under a code root, and finding them by search term.

use std::cell::{Cell, Ref, RefCell};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
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
///
/// They can be kept in an index between runs, which stands in for the walk. An index can be out
/// of date, since checkouts come and go without h; so when what it has does not answer a
/// question, the code root is walked again, and the index rewritten, before the answer is given.
#[derive(Debug)]
pub struct Projects {
    root: PathBuf,
    /// The file they are kept in between runs, if any.
    index: Option<PathBuf>,
    /// In sorted order of their paths; read or walked for when first needed.
    paths: RefCell<Option<Vec<Project>>>,
    /// Whether `paths` came from the index, and so may be out of date.
    from_index: Cell<bool>,
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
    /// The projects under `root`, which are walked for when first needed. Symbolic links are
    /// followed, except back into a directory the walk is already in. A missing `root` has none.
    pub fn new(root: impl AsRef<Path>) -> Projects {
        Projects {
            root: root.as_ref().to_path_buf(),
            index: None,
            paths: RefCell::new(None),
            from_index: Cell::new(false),
        }
    }

    /// The projects under `root`, kept in `index` between runs: read from it when first needed,
    /// if it has them for `root`, and otherwise walked for and written to it.
    pub fn with_index(root: impl AsRef<Path>, index: impl Into<PathBuf>) -> Projects {
        Projects { index: Some(index.into()), ..Projects::new(root) }
    }

    /// Walk the code root again, and write what is found to the index. This is for after
    /// something that changes what is under the code root, such as a clone.
    pub fn refresh(&self) {
        let paths = walk_root(&self.root);
        if let Some(index) = &self.index {
            // The index only saves a walk, so failing to write it costs no more than that.
            let _ = write_index(index, &self.root, &paths);
        }
        *self.paths.borrow_mut() = Some(paths);
        self.from_index.set(false);
    }

    /// The code root they are under.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn paths(&self) -> Ref<'_, [Project]> {
        if self.paths.borrow().is_none() {
            let read = self.index.as_deref().and_then(|index| read_index(index, &self.root));
            match read {
                Some(paths) => {
                    *self.paths.borrow_mut() = Some(paths);
                    self.from_index.set(true);
                }
                None => self.refresh(),
            }
        }
        Ref::map(self.paths.borrow(), |paths| paths.as_deref().unwrap_or_default())
    }

    /// Ask `question` of the projects, and when what came from the index does not answer it, as
    /// `answers` decides, ask again after walking the code root.
    fn ask<T>(&self, question: impl Fn(&[Project]) -> T, answers: impl Fn(&T) -> bool) -> T {
        let answer = question(&self.paths());
        if answers(&answer) || !self.from_index.get() {
            return answer;
        }
        self.refresh();
        question(&self.paths())
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
        // A directory the index has may have gone since.
        let found = |paths: &[Project]| self.find_among(paths, term).filter(|path| path.is_dir());
        self.ask(found, Option::is_some)
    }

    fn find_among(&self, paths: &[Project], term: &str) -> Option<PathBuf> {
        let (term, holders_only) = match term.strip_suffix('/') {
            Some(term) => (term, true),
            None => (term, false),
        };
        let has_capitals = term.bytes().any(|b| b.is_ascii_uppercase());
        let wanted: Vec<&str> = term.split('/').collect();
        // `max_by_key` keeps the last of equals, so going backwards keeps the first in sorted order.
        let best = paths.iter().rev().filter_map(|project| {
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
    /// order. A directory that holds repositories is offered with a `/` at the end, as a term
    /// for it is spelled, which also leaves it ready for the next segment; one whose name is also
    /// a repository's is offered both ways. Only what can be spelled as a term is offered.
    pub fn list_completions(&self, word: &str) -> Vec<String> {
        self.ask(|paths| list_completions(paths, word), |found| !found.is_empty())
    }
}

fn list_completions(paths: &[Project], word: &str) -> Vec<String> {
    let wanted = word.split('/').count();
    let mut found: Vec<(usize, String)> = paths
        .iter()
        .filter_map(|Project { segments, is_repository }| {
            let tail = &segments[segments.len().checked_sub(wanted)?..];
            let tail: Vec<&str> = tail.iter().map(|s| s.to_str()).collect::<Option<_>>()?;
            if !tail.iter().all(|s| is_simple_name(s)) {
                return None;
            }
            let mut term = tail.join("/");
            if !is_repository {
                term.push('/');
            }
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

/// The first line of an index, which says what it holds and how.
const INDEX_HEADER: &[u8] = b"h projects 1\0";

/// Read the projects under `root` from the index at `path`. `None` when it cannot be read, or
/// holds another code root's.
///
/// After [`INDEX_HEADER`] and the code root, each project is a `/` for a repository or a `+` for
/// a directory holding them, then its path relative to the code root; everything ends in a NUL,
/// which no path can hold.
fn read_index(path: &Path, root: &Path) -> Option<Vec<Project>> {
    let bytes = fs::read(path).ok()?;
    let rest = bytes.strip_prefix(INDEX_HEADER)?.strip_suffix(b"\0")?;
    let mut records = rest.split(|&b| b == 0);
    if records.next()? != root.as_os_str().as_bytes() {
        return None;
    }
    records
        .map(|record| {
            let (&kind, path) = record.split_first()?;
            let segments = path.split(|&b| b == b'/').map(|s| OsStr::from_bytes(s).into());
            let segments: Vec<OsString> = segments.collect();
            match kind {
                _ if path.is_empty() => None,
                b'/' => Some(Project { segments, is_repository: true }),
                b'+' => Some(Project { segments, is_repository: false }),
                _ => None,
            }
        })
        .collect()
}

/// Write `paths`, the projects under `root`, to the index at `path`, replacing it whole.
fn write_index(path: &Path, root: &Path, paths: &[Project]) -> std::io::Result<()> {
    let mut bytes = INDEX_HEADER.to_vec();
    bytes.extend_from_slice(root.as_os_str().as_bytes());
    bytes.push(0);
    for project in paths {
        bytes.push(if project.is_repository { b'/' } else { b'+' });
        let segments: Vec<&[u8]> = project.segments.iter().map(|s| s.as_bytes()).collect();
        bytes.extend_from_slice(&segments.join(&b'/'));
        bytes.push(0);
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // Write beside it and rename over it, so no reader ever sees half an index.
    let mut temp = path.as_os_str().to_owned().into_vec();
    temp.extend_from_slice(format!(".{}", std::process::id()).as_bytes());
    let temp = PathBuf::from(OsString::from_vec(temp));
    let written = fs::File::create(&temp).and_then(|mut file| file.write_all(&bytes));
    match written.and_then(|()| fs::rename(&temp, path)) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
}

/// Walk `root` for its projects.
fn walk_root(root: &Path) -> Vec<Project> {
    let mut paths = Vec::new();
    if let Ok(meta) = fs::metadata(root) {
        let mut ancestors = vec![(meta.dev(), meta.ino())];
        walk(root, &mut Vec::new(), &mut ancestors, &mut paths);
    }
    paths
}

/// What a directory the walk visits turns out to be.
enum Visited {
    Repository,
    /// A directory holding repositories.
    Holder,
    /// Neither, so not a project.
    Nothing,
}

/// Add the projects below `dir`, which is at `segments` below the root, to `found`, and say
/// whether `dir` is a repository or holds one.
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
        Projects::new(root).find(term)
    }

    fn list_completions(root: impl AsRef<Path>, word: &str) -> Vec<String> {
        Projects::new(root).list_completions(word)
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
        assert_eq!(list_completions(tmp.path(), ""), ["github.com/", "owner/", "proj"]);
        assert_eq!(list_completions(tmp.path(), "owner/"), ["owner/proj"]);
    }

    #[test]
    fn the_index_stands_in_for_the_walk() {
        let tmp = tree(&["code/github.com/owner/one"]);
        let (root, index) = (tmp.path().join("code"), tmp.path().join("store/h/projects"));
        assert_eq!(Projects::with_index(&root, &index).list_completions("o"), ["one", "owner/"]);
        assert!(index.is_file(), "the first walk writes the index");

        // A checkout made since is not in the index, so it is not offered while something else is.
        fs::create_dir_all(root.join("github.com/owner/other/.git")).unwrap();
        let projects = Projects::with_index(&root, &index);
        assert_eq!(projects.list_completions("o"), ["one", "owner/"]);
        // Nothing in the index answers this, so the code root is walked again.
        assert_eq!(projects.list_completions("oth"), ["other"]);
        let projects = Projects::with_index(&root, &index);
        assert_eq!(projects.list_completions("o"), ["one", "other", "owner/"]);
    }

    #[test]
    fn a_term_the_index_does_not_answer_walks_again() {
        let tmp = tree(&["code/a/one", "code/b/two"]);
        let (root, index) = (tmp.path().join("code"), tmp.path().join("index"));
        Projects::with_index(&root, &index).refresh();

        // Made since the index was written.
        fs::create_dir_all(root.join("c/three/.git")).unwrap();
        let three = Some(root.join("c/three"));
        assert_eq!(Projects::with_index(&root, &index).find("three"), three);
        // Gone since: the index has it, but the walk does not.
        fs::remove_dir_all(root.join("a")).unwrap();
        let projects = Projects::with_index(&root, &index);
        assert_eq!(projects.find("one"), None);
        assert_eq!(projects.find("two"), Some(root.join("b/two")));
    }

    #[test]
    fn an_index_of_another_root_or_unreadable_is_walked_over() {
        let tmp = tree(&["code/proj", "other/elsewhere"]);
        let (root, index) = (tmp.path().join("code"), tmp.path().join("index"));
        Projects::with_index(tmp.path().join("other"), &index).refresh();
        assert_eq!(Projects::with_index(&root, &index).list_completions(""), ["proj"]);
        assert_eq!(read_index(&index, &root).map(|p| p.len()), Some(1), "rewritten for this root");
        for junk in [&b""[..], b"h projects 1\0", b"h projects 2\0/x\0", b"garbage"] {
            fs::write(&index, junk).unwrap();
            assert_eq!(read_index(&index, &root), None, "{junk:?}");
            assert_eq!(Projects::with_index(&root, &index).find("proj"), Some(root.join("proj")));
        }
    }

    #[test]
    fn the_index_keeps_any_path() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = tempfile::tempdir().unwrap();
        let odd = OsStr::from_bytes(b"caf\xe9 dir");
        fs::create_dir_all(tmp.path().join(odd).join("proj/.git")).unwrap();
        let index = tmp.path().join("index");
        let walked = walk_root(tmp.path());
        write_index(&index, tmp.path(), &walked).unwrap();
        assert_eq!(read_index(&index, tmp.path()), Some(walked));
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
        assert_eq!(complete("NIX"), ["NixOS/", "nix", "nixos/", "nixpkgs"]);
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
    fn completes_holders_with_a_slash() {
        let tmp = tree(&["github.com/alacritty/alacritty", "github.com/owner/proj"]);
        let complete = |word| list_completions(tmp.path(), word);
        assert_eq!(complete("alac"), ["alacritty", "alacritty/"]);
        assert_eq!(complete("alacritty/"), ["alacritty/alacritty"]);
        assert_eq!(complete("git"), ["github.com/"]);
        assert_eq!(complete("github.com/"), ["github.com/alacritty/", "github.com/owner/"]);
    }

    #[test]
    fn completes_only_what_a_term_can_spell() {
        let tmp = tree(&["a/with space", "a/caf\u{e9}", "a/fine", "a/.hidden"]);
        assert_eq!(list_completions(tmp.path(), "a/"), ["a/fine"]);
        assert_eq!(list_completions(tmp.path(), ""), ["a/", "fine"]);
    }
}
