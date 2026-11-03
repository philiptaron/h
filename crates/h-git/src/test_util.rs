//! What the unit tests share: building and reading the `OsString`s that git's arguments are.

use std::ffi::OsString;

/// `list` as git arguments.
pub fn opts(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

/// The git arguments `args` as `String`s, to compare with literals.
pub fn strings(args: Vec<OsString>) -> Vec<String> {
    args.into_iter().map(|a| a.into_string().unwrap()).collect()
}

/// `list` as the `(key, value)` pairs of `-c key=value` settings.
pub fn pairs(list: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
    list.iter().map(|(k, v)| (OsString::from(k), OsString::from(v))).collect()
}
