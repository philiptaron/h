//! Resolve a project name, `user/repo`, or URL to a directory, cloning it if needed.
//!
//! Invoked by the shell function from `h-shell-init` as `h --resolve <code-root> <term> [git
//! opts]`. Prints the directory to `cd` into on stdout; on failure prints the current directory
//! instead, so the shell function stays put.

use std::ffi::OsString;
use std::process::ExitCode;

use h_core::output::{fail_with_cwd, print_cwd, print_path};
use h_core::path::expand_tilde;
use h_git::clone::clone_repo;
use h_git::github;
use h_git::resolve::resolve;

const USAGE: &str = "Usage: h (<name> | <repo>/<name> | <url>) [git opts]";
const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.len() < 2 {
        return fail_with_cwd(&format!(
            "h {VERSION}\nUsage: eval \"$(h-shell-init [options] [code-root])\""
        ));
    }
    if args[1] == "-V" || args[1] == "--version" {
        println!("h {VERSION}");
        return ExitCode::SUCCESS;
    }
    if args[1] != "--resolve" {
        return fail_with_cwd(&format!(
            "h {VERSION}\nh is not installed\n\nUsage: eval \"$(h-shell-init [code-root])\""
        ));
    }
    let Some(code_root) = args.get(2) else {
        return fail_with_cwd("Usage: h --resolve <code-root> <term>");
    };
    let code_root = expand_tilde(code_root);
    let Some(term) = args.get(3) else {
        return fail_with_cwd(USAGE);
    };
    if term == "-h" || term == "--help" {
        return fail_with_cwd(USAGE);
    }
    let Some(term) = term.to_str() else {
        return fail_with_cwd(&format!("Unknown pattern for {}", term.to_string_lossy()));
    };

    let api = github::api_base();
    let resolution =
        match resolve(&code_root, term, |user, repo| github::fetch_repo_info(&api, user, repo)) {
            Ok(resolution) => resolution,
            Err(msg) => return fail_with_cwd(&msg),
        };

    if resolution.path.is_dir() {
        print_path(&resolution.path);
        return ExitCode::SUCCESS;
    }
    let Some(url) = resolution.clone_url else {
        return fail_with_cwd(&format!("{term} not found"));
    };

    match clone_repo(&url, &resolution.path, &args[4..]) {
        0 => {
            print_path(&resolution.path);
            ExitCode::SUCCESS
        }
        code => {
            print_cwd();
            ExitCode::from(code)
        }
    }
}
