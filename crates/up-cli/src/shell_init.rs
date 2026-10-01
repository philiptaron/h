//! The shell function printed by `up-shell-init`.

use std::ffi::OsString;

use h_core::args::{Command, unknown_option};
use h_core::shell::CdCommand;

pub const UP_USAGE: &str = "Usage: eval \"$(up-shell-init [--pushd])\"";

/// Parse the arguments to `up-shell-init` (excluding the program name).
pub fn parse_up_init_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Command<CdCommand>, String> {
    let mut cd = CdCommand::Cd;
    for arg in args {
        match arg.to_str() {
            Some("--pushd") => cd = CdCommand::Pushd,
            Some("-h" | "--help") => return Ok(Command::Help),
            Some("-V" | "--version") => return Ok(Command::Version),
            _ => return Err(unknown_option(&arg)),
        }
    }
    Ok(Command::Run(cd))
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
        assert_eq!(parse_up_init_args(args(&["-V"])), Ok(Command::Version));
        assert_eq!(parse_up_init_args(args(&["--version"])), Ok(Command::Version));
        assert_eq!(parse_up_init_args(args(&["x"])), Err("Unknown option: x".into()));
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
