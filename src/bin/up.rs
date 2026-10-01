//! Print the root of the project containing the current directory.
//!
//! Invoked by the shell function from `up-shell-init`.

use std::path::PathBuf;
use std::process::ExitCode;

use h::up::find_project_root;
use h::util::{fail_with_cwd, print_path};

fn main() -> ExitCode {
    if let Some(arg) = std::env::args_os().nth(1)
        && (arg == "-h" || arg == "--help")
    {
        return fail_with_cwd("up is not installed\n\nUsage: eval \"$(up-shell-init [--pushd])\"");
    }

    // Prefer $PWD, which preserves the symlinks the user navigated through.
    let cwd = match std::env::var_os("PWD") {
        Some(pwd) => PathBuf::from(pwd),
        None => match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(err) => {
                eprintln!("getcwd: {err}");
                return ExitCode::FAILURE;
            }
        },
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let direnv_dir = std::env::var_os("DIRENV_DIR");

    print_path(&find_project_root(&cwd, home.as_deref(), direnv_dir.as_deref()));
    ExitCode::SUCCESS
}
