//! Core of `h`: resolving search terms (project names, `user/repo`, URLs) to directories under a
//! code root, looking up canonical GitHub casing, cloning repositories, keeping a shared object
//! store of upstream repositories, and making worktrees for Claude Code's hooks.

pub mod clone;
pub mod files;
pub mod git;
pub mod github;
pub mod hook;
pub mod ingest;
pub mod projects;
pub mod resolve;
pub mod store;
pub mod submodules;

#[cfg(test)]
mod test_util;
