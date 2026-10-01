//! Core of `h`: resolving search terms (project names, `user/repo`, URLs) to directories under a
//! code root, looking up canonical GitHub casing, and cloning repositories.

pub mod clone;
pub mod git;
pub mod github;
pub mod resolve;
pub mod search;
