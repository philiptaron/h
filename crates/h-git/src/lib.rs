//! Fast shell navigation for projects organized as `<code-root>/<domain>/<path>`.
//!
//! The crate ships four binaries:
//!
//! - `h` resolves a project name, `user/repo`, or URL to a directory, cloning it if needed.
//! - `h-shell-init` prints the shell function (and completion) that wraps `h`.
//! - `up` finds the root of the project containing the current directory.
//! - `up-shell-init` prints the shell function that wraps `up`.

pub mod clone;
pub mod github;
pub mod resolve;
pub mod search;
pub mod shell;
pub mod up;
pub mod util;
