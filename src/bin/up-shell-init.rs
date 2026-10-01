//! Print the `up` shell function.
//!
//! Usage: `eval "$(up-shell-init [--pushd])"`

use std::process::ExitCode;

use h::shell::{Command, UP_USAGE, parse_up_init_args, render_up_init, sibling_exe};
use h::util::fail;

fn main() -> ExitCode {
    let cd = match parse_up_init_args(std::env::args_os().skip(1)) {
        Ok(Command::Run(cd)) => cd,
        Ok(Command::Help) => {
            println!("{UP_USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => return fail(&msg),
    };

    let exe = sibling_exe("up");
    let Some(exe) = exe.to_str() else {
        return fail(&format!("Path to up is not valid UTF-8: {}", exe.display()));
    };

    print!("{}", render_up_init(cd, exe));
    ExitCode::SUCCESS
}
