//! Print the `up` shell function.
//!
//! Usage: `eval "$(up-shell-init [--pushd])"`

use std::process::ExitCode;

use h_core::args::Command;
use h_core::exe;
use h_core::output::fail;
use up_cli::shell_init::{UP_USAGE, parse_up_init_args, render_up_init};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let cd = match parse_up_init_args(std::env::args_os().skip(1)) {
        Ok(Command::Run(cd)) => cd,
        Ok(Command::Help) => {
            println!("up-shell-init {VERSION}\n{UP_USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Command::Version) => {
            println!("up-shell-init {VERSION}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => return fail(&msg),
    };

    let exe = exe::sibling("up");
    let Some(exe) = exe.to_str() else {
        return fail(&format!("Path to up is not valid UTF-8: {}", exe.display()));
    };

    print!("{}", render_up_init(cd, exe));
    ExitCode::SUCCESS
}
