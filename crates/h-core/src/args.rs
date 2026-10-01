//! Command-line parsing shared by the `*-shell-init` binaries.

use std::ffi::OsStr;

/// What a `*-shell-init` binary was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command<T> {
    Help,
    Version,
    Run(T),
}

/// The error message for an unrecognized command-line argument.
pub fn unknown_option(arg: &OsStr) -> String {
    format!("Unknown option: {}", arg.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_unknown_option() {
        assert_eq!(unknown_option(OsStr::new("--bogus")), "Unknown option: --bogus");
    }
}
