//! Core of `h`: fast shell navigation for projects organized as `<code-root>/<domain>/<path>`.
//!
//! Resolves search terms (project names, `user/repo`, URLs) to directories, looks up canonical
//! GitHub casing, clones repositories, and holds the pieces shared by the `h-cli` and `up-cli`
//! binaries.

pub mod clone;
pub mod github;
pub mod resolve;
pub mod search;
pub mod shell;
pub mod util;
