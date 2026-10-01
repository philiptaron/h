//! Finding the root of the project that contains a directory.

use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Whether `dir` looks like the root of a project.
///
/// A project root contains a `.git` directory or gitfile (as worktrees, submodules and container
/// clones have), an `.hg` directory, or an `.envrc` or `Gemfile` file, or is the directory direnv
/// has loaded (`DIRENV_DIR` holds that directory prefixed with `-`).
pub fn is_project_root(dir: &Path, direnv_dir: Option<&OsStr>) -> bool {
    let git = dir.join(".git");
    git.is_dir()
        || is_gitfile(&git)
        || dir.join(".hg").is_dir()
        || dir.join(".envrc").is_file()
        || dir.join("Gemfile").is_file()
        || direnv_dir
            .and_then(|d| d.as_bytes().strip_prefix(b"-"))
            .is_some_and(|d| Path::new(OsStr::from_bytes(d)) == dir)
}

/// Whether `path` is a gitfile: a file starting `gitdir: `, which is how git itself tells one
/// from any other file named `.git`.
fn is_gitfile(path: &Path) -> bool {
    let mut start = [0u8; 8];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut start)).is_ok()
        && &start == b"gitdir: "
}

/// Find the project root to move to from `cwd`.
///
/// When `cwd` is itself a project root, the search starts from its parent, so repeated calls
/// climb through nested projects. The search stops at `/` and at `home`; if no root is found,
/// `cwd` is returned.
pub fn find_project_root(cwd: &Path, home: Option<&Path>, direnv_dir: Option<&OsStr>) -> PathBuf {
    let mut dir = cwd;
    if is_project_root(dir, direnv_dir) {
        dir = parent(dir).unwrap_or(dir);
    }
    while dir != Path::new("/") && Some(dir) != home {
        if is_project_root(dir, direnv_dir) {
            return dir.to_path_buf();
        }
        match parent(dir) {
            Some(p) => dir = p,
            None => break,
        }
    }
    cwd.to_path_buf()
}

fn parent(dir: &Path) -> Option<&Path> {
    dir.parent().filter(|p| !p.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn detects_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for dir in ["git", "hg", "envrc", "gemfile", "none", "gitfile", "otherfile", "envrcdir"] {
            fs::create_dir(root.join(dir)).unwrap();
        }
        fs::create_dir(root.join("git/.git")).unwrap();
        fs::create_dir(root.join("hg/.hg")).unwrap();
        fs::write(root.join("envrc/.envrc"), "").unwrap();
        fs::write(root.join("gemfile/Gemfile"), "").unwrap();
        // Worktrees, submodules and container clones have a gitfile in place of a directory.
        fs::write(root.join("gitfile/.git"), "gitdir: ../x\n").unwrap();
        // Any other file named `.git` is not one.
        fs::write(root.join("otherfile/.git"), "not a gitfile\n").unwrap();
        fs::create_dir(root.join("envrcdir/.envrc")).unwrap();

        for dir in ["git", "hg", "envrc", "gemfile", "gitfile"] {
            assert!(is_project_root(&root.join(dir), None), "{dir}");
        }
        for dir in ["none", "otherfile", "envrcdir"] {
            assert!(!is_project_root(&root.join(dir), None), "{dir}");
        }
    }

    #[test]
    fn detects_direnv_dir() {
        let dir = Path::new("/some/where");
        assert!(is_project_root(dir, Some(OsStr::new("-/some/where"))));
        assert!(!is_project_root(dir, Some(OsStr::new("/some/where"))));
        assert!(!is_project_root(dir, Some(OsStr::new("-/some"))));
        assert!(!is_project_root(dir, Some(OsStr::new(""))));
    }

    fn project_tree() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("outer/.git")).unwrap();
        fs::create_dir_all(tmp.path().join("outer/inner/.git")).unwrap();
        fs::create_dir_all(tmp.path().join("outer/inner/src/deep")).unwrap();
        fs::create_dir_all(tmp.path().join("loose/dir")).unwrap();
        tmp
    }

    #[test]
    fn climbs_to_nearest_root() {
        let tmp = project_tree();
        let cwd = tmp.path().join("outer/inner/src/deep");
        assert_eq!(find_project_root(&cwd, None, None), tmp.path().join("outer/inner"));
    }

    #[test]
    fn from_a_root_climbs_to_the_enclosing_root() {
        let tmp = project_tree();
        let cwd = tmp.path().join("outer/inner");
        assert_eq!(find_project_root(&cwd, None, None), tmp.path().join("outer"));
    }

    #[test]
    fn climbs_through_submodules_worktrees_and_containers() {
        // A container clone, a worktree of it, and a submodule inside the worktree, each with
        // a gitfile as git writes them.
        let tmp = tempfile::tempdir().unwrap();
        let container = tmp.path().join("p");
        let worktree = container.join("topic");
        let submodule = worktree.join("vendor/lib");
        let deep = submodule.join("src/deep");
        fs::create_dir_all(container.join(".bare/worktrees/topic/modules/lib")).unwrap();
        fs::create_dir_all(&deep).unwrap();
        fs::write(container.join(".git"), "gitdir: ./.bare\n").unwrap();
        let admin = container.join(".bare/worktrees/topic");
        fs::write(worktree.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        fs::write(submodule.join(".git"), "gitdir: ../../../.bare/worktrees/topic/modules/lib\n")
            .unwrap();

        let mut cwd = deep;
        for want in [&submodule, &worktree, &container, &container] {
            cwd = find_project_root(&cwd, None, None);
            assert_eq!(&cwd, want);
        }
    }

    #[test]
    fn stays_put_without_a_root() {
        let tmp = project_tree();
        let cwd = tmp.path().join("loose/dir");
        assert_eq!(find_project_root(&cwd, None, None), cwd);
    }

    #[test]
    fn stops_at_home() {
        let tmp = project_tree();
        let home = tmp.path().join("outer/inner/src");
        let cwd = tmp.path().join("outer/inner/src/deep");
        assert_eq!(find_project_root(&cwd, Some(&home), None), cwd);
    }

    #[test]
    fn uses_direnv_dir() {
        let tmp = project_tree();
        let cwd = tmp.path().join("loose/dir");
        let mut direnv = std::ffi::OsString::from("-");
        direnv.push(tmp.path().join("loose"));
        assert_eq!(find_project_root(&cwd, None, Some(&direnv)), tmp.path().join("loose"));
    }

    #[test]
    fn handles_root_and_relative_paths() {
        assert_eq!(find_project_root(Path::new("/"), None, None), PathBuf::from("/"));
        assert_eq!(
            find_project_root(Path::new("no/such/dir"), None, None),
            PathBuf::from("no/such/dir")
        );
    }
}
