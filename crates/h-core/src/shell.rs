//! The shell that will run the generated functions, how they change directory, and the binary
//! they call.

use std::path::{Path, PathBuf};

/// The shell that will evaluate the generated code, which decides the completion code emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Unknown,
}

impl Shell {
    /// Identify a shell from a process name, as found in `/proc/<pid>/comm`.
    pub fn from_comm(comm: &str) -> Shell {
        match comm.trim_end_matches('\n') {
            "bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            _ => Shell::Unknown,
        }
    }

    /// Identify the shell running this process, which is the one that will `eval` our output.
    pub fn detect_parent() -> Shell {
        let ppid = std::os::unix::process::parent_id();
        std::fs::read_to_string(format!("/proc/{ppid}/comm"))
            .map_or(Shell::Unknown, |comm| Shell::from_comm(&comm))
    }
}

/// Whether `cd` or `pushd` is used to change directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CdCommand {
    Cd,
    Pushd,
}

impl CdCommand {
    pub fn as_str(self) -> &'static str {
        match self {
            CdCommand::Cd => "cd",
            CdCommand::Pushd => "pushd",
        }
    }
}

/// The path of the executable `name` that sits next to the running executable.
pub fn sibling_exe(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| {
        let argv0 = PathBuf::from(std::env::args_os().next().unwrap_or_default());
        if argv0.is_absolute() {
            argv0
        } else {
            let pwd = std::env::var_os("PWD").unwrap_or_else(|| ".".into());
            Path::new(&pwd).join(argv0)
        }
    });
    exe.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn detects_shell_from_comm() {
        assert_eq!(Shell::from_comm("bash\n"), Shell::Bash);
        assert_eq!(Shell::from_comm("zsh\n"), Shell::Zsh);
        assert_eq!(Shell::from_comm("fish\n"), Shell::Unknown);
        assert_eq!(Shell::from_comm(""), Shell::Unknown);
    }

    #[test]
    fn sibling_exe_replaces_file_name() {
        let up = sibling_exe("up");
        assert_eq!(up.file_name(), Some(OsStr::new("up")));
        assert_eq!(up.parent(), std::env::current_exe().unwrap().parent());
    }
}
