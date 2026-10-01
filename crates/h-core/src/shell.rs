//! The shell that will run the generated functions, and how they change directory.

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
        parent_name().map_or(Shell::Unknown, |name| Shell::from_comm(&name))
    }
}

/// The parent process's command name, from `/proc/<ppid>/comm`.
#[cfg(not(target_os = "macos"))]
fn parent_name() -> Option<String> {
    let ppid = std::os::unix::process::parent_id();
    std::fs::read_to_string(format!("/proc/{ppid}/comm")).ok()
}

/// The parent process's command name, from `proc_name`, since macOS has no `/proc`.
#[cfg(target_os = "macos")]
fn parent_name() -> Option<String> {
    let ppid = libc::pid_t::try_from(std::os::unix::process::parent_id()).ok()?;
    let mut buf = [0u8; 256];
    // SAFETY: `buf` is valid for writes of `buf.len()` bytes, the size passed.
    let len = unsafe { libc::proc_name(ppid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    let len = usize::try_from(len).ok().filter(|&len| len > 0 && len < buf.len())?;
    String::from_utf8(buf[..len].to_vec()).ok()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_shell_from_comm() {
        assert_eq!(Shell::from_comm("bash\n"), Shell::Bash);
        assert_eq!(Shell::from_comm("zsh\n"), Shell::Zsh);
        assert_eq!(Shell::from_comm("fish\n"), Shell::Unknown);
        assert_eq!(Shell::from_comm(""), Shell::Unknown);
    }

    #[test]
    fn finds_the_parent_process_name() {
        // The test harness is the parent; whatever it is called, its name can be read.
        assert!(parent_name().is_some_and(|name| !name.trim().is_empty()));
    }
}
