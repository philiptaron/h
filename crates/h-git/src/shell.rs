//! Pieces shared by the `h-shell-init` and `up-shell-init` binaries.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

pub const UP_USAGE: &str = "Usage: eval \"$(up-shell-init [--pushd])\"";

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

/// What `h-shell-init` or `up-shell-init` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command<T> {
    Help,
    Run(T),
}

/// The error message for an unrecognized command-line argument.
pub fn unknown_option(arg: &OsStr) -> String {
    format!("Unknown option: {}", arg.to_string_lossy())
}

/// Parse the arguments to `up-shell-init` (excluding the program name).
pub fn parse_up_init_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Command<CdCommand>, String> {
    let mut cd = CdCommand::Cd;
    for arg in args {
        match arg.to_str() {
            Some("--pushd") => cd = CdCommand::Pushd,
            Some("-h" | "--help") => return Ok(Command::Help),
            _ => return Err(unknown_option(&arg)),
        }
    }
    Ok(Command::Run(cd))
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

/// The shell function that wraps `up`.
pub fn render_up_init(cd: CdCommand, up_exe: &str) -> String {
    let cd = cd.as_str();
    format!(
        "up() {{\n\
         \x20 _up_dir=$(command {up_exe} \"$@\")\n\
         \x20 if [ $? = 0 ]; then\n\
         \x20   [ \"$_up_dir\" != \"$PWD\" ] && {cd} \"$_up_dir\"\n\
         \x20 fi\n\
         }}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn parses_up_init_args() {
        assert_eq!(parse_up_init_args(args(&[])), Ok(Command::Run(CdCommand::Cd)));
        assert_eq!(parse_up_init_args(args(&["--pushd"])), Ok(Command::Run(CdCommand::Pushd)));
        assert_eq!(parse_up_init_args(args(&["--help"])), Ok(Command::Help));
        assert_eq!(parse_up_init_args(args(&["x"])), Err("Unknown option: x".into()));
    }

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

    #[test]
    fn renders_up_function() {
        assert_eq!(
            render_up_init(CdCommand::Pushd, "/bin/up"),
            r#"up() {
  _up_dir=$(command /bin/up "$@")
  if [ $? = 0 ]; then
    [ "$_up_dir" != "$PWD" ] && pushd "$_up_dir"
  fi
}
"#
        );
    }
}
