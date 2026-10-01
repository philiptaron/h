//! Finding the binaries the generated shell functions call.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// The absolute path of the program `name` installed next to the running executable.
///
/// The running executable is found through `/proc/self/exe` on Linux and `_NSGetExecutablePath`
/// (with symlinks resolved) on macOS, else through `argv[0]`, looked up in `$PATH` when it has no
/// slash. If it cannot be found at all, `name` is returned as is, for the shell to look up.
pub fn sibling(name: &str) -> PathBuf {
    let current = std::env::current_exe().ok().map(|exe| exe.canonicalize().unwrap_or(exe));
    let argv0 = std::env::args_os().next().unwrap_or_default();
    let path = std::env::var_os("PATH").unwrap_or_default();
    let cwd = std::env::current_dir().ok();
    sibling_of(current, &argv0, &path, cwd.as_deref(), name)
}

/// [`sibling`], with the process state passed in.
pub fn sibling_of(
    current_exe: Option<PathBuf>,
    argv0: &OsStr,
    path: &OsStr,
    cwd: Option<&Path>,
    name: &str,
) -> PathBuf {
    let exe = current_exe.or_else(|| {
        if argv0.as_bytes().contains(&b'/') {
            Some(PathBuf::from(argv0))
        } else {
            search_path(argv0, path, cwd)
        }
    });
    let Some(exe) = exe else {
        return PathBuf::from(name);
    };
    let exe = match cwd {
        Some(cwd) if exe.is_relative() => cwd.join(exe),
        _ => exe,
    };
    exe.with_file_name(name)
}

/// Find `program` in a `$PATH`-style list the way the shell does: the first executable file wins,
/// and an empty entry means the current directory, `cwd`.
pub fn search_path(program: &OsStr, path: &OsStr, cwd: Option<&Path>) -> Option<PathBuf> {
    path.as_bytes().split(|&b| b == b':').find_map(|dir| {
        let dir = match dir {
            b"" => cwd.unwrap_or(Path::new(".")),
            dir => Path::new(OsStr::from_bytes(dir)),
        };
        let candidate = dir.join(program);
        is_executable(&candidate).then_some(candidate)
    })
}

fn is_executable(path: &Path) -> bool {
    path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;

    fn install(dir: &Path, name: &str, mode: u32) {
        fs::create_dir_all(dir).unwrap();
        let file = dir.join(name);
        fs::write(&file, "").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn path_list(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    fn os(s: &str) -> &OsStr {
        OsStr::new(s)
    }

    #[test]
    fn sibling_of_the_running_test() {
        let up = sibling("up");
        assert_eq!(up.file_name(), Some(os("up")));
        let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
        assert_eq!(up.parent(), exe.parent());
    }

    #[test]
    fn prefers_the_current_exe() {
        let exe = Some(PathBuf::from("/nix/store/x/bin/h-shell-init"));
        let got = sibling_of(exe, os("./elsewhere/h-shell-init"), os(""), None, "h");
        assert_eq!(got, PathBuf::from("/nix/store/x/bin/h"));
    }

    #[test]
    fn falls_back_to_argv0_with_a_slash() {
        let cwd = Some(Path::new("/work"));
        let got = sibling_of(None, os("/opt/bin/up-shell-init"), os(""), cwd, "up");
        assert_eq!(got, PathBuf::from("/opt/bin/up"));
        let got = sibling_of(None, os("bin/up-shell-init"), os(""), cwd, "up");
        assert_eq!(got, PathBuf::from("/work/bin/up"));
    }

    #[test]
    fn looks_up_bare_argv0_in_path() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        install(&a, "h-shell-init", 0o644); // not executable, so skipped
        install(&b, "h-shell-init", 0o755);
        let path = path_list(&[&tmp.path().join("missing"), &a, &b]);
        assert_eq!(sibling_of(None, os("h-shell-init"), &path, None, "h"), b.join("h"));
    }

    #[test]
    fn empty_path_entry_is_the_current_directory() {
        let tmp = tempfile::tempdir().unwrap();
        install(tmp.path(), "h-shell-init", 0o755);
        let got = sibling_of(None, os("h-shell-init"), os("/nonexistent:"), Some(tmp.path()), "h");
        assert_eq!(got, tmp.path().join("h"));
    }

    #[test]
    fn unknown_location_falls_back_to_the_bare_name() {
        let got = sibling_of(None, os("h-shell-init"), os("/nonexistent"), None, "h");
        assert_eq!(got, PathBuf::from("h"));
        assert_eq!(sibling_of(None, os(""), os(""), None, "h"), PathBuf::from("h"));
    }
}
