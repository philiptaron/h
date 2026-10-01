//! Print the `h` shell function, plus tab completion for bash and zsh.
//!
//! Usage: `eval "$(h-shell-init [--pushd] [--name NAME] [--git-opts "OPTIONS"] [code-root])"`

use std::ffi::OsString;
use std::process::ExitCode;

use h::shell::{
    CODE_ROOT_ENV, Command, DEFAULT_CODE_ROOT, H_USAGE, Shell, parse_h_init_args, render_h_init,
    sibling_exe,
};
use h::util::{expand_tilde, fail};

fn main() -> ExitCode {
    let opts = match parse_h_init_args(std::env::args_os().skip(1)) {
        Ok(Command::Run(opts)) => opts,
        Ok(Command::Help) => {
            println!("{H_USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(msg) => return fail(&msg),
    };

    let code_root = opts.code_root.clone().unwrap_or_else(|| {
        std::env::var_os(CODE_ROOT_ENV).unwrap_or_else(|| OsString::from(DEFAULT_CODE_ROOT))
    });
    let code_root = expand_tilde(&code_root);
    let Some(code_root) = code_root.to_str() else {
        return fail(&format!("Code root is not valid UTF-8: {}", code_root.display()));
    };
    let exe = sibling_exe("h");
    let Some(exe) = exe.to_str() else {
        return fail(&format!("Path to h is not valid UTF-8: {}", exe.display()));
    };

    print!("{}", render_h_init(&opts, exe, code_root, Shell::detect_parent()));
    ExitCode::SUCCESS
}
