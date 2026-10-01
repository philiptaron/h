//! GitHub API lookups used to canonicalize the casing of `owner/repo`.

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;
use std::time::Duration;

use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig};

use crate::git;

/// Base URL of the GitHub REST API.
pub const DEFAULT_API: &str = "https://api.github.com";

/// Environment variable that overrides [`DEFAULT_API`] (useful for testing).
pub const API_ENV: &str = "H_GITHUB_API";

/// The canonical owner and name of a repository, as reported by GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub owner: String,
    pub name: String,
    /// For a fork, the `owner/name` of the repository it was forked from.
    pub parent: Option<String>,
}

/// The API base URL, honoring [`API_ENV`].
pub fn api_base() -> String {
    std::env::var(API_ENV).unwrap_or_else(|_| DEFAULT_API.to_string())
}

/// The token git's credential helpers keep for github.com, as `git credential fill` gives it,
/// with the `credential.*` settings among `config` (the `-c` pairs h is given), so that each
/// identity's `credential.username` picks its own. `None` when no helper has one.
///
/// Nothing ever prompts: `credential.interactive=false` keeps git from asking at all (git 2.46),
/// and for older versions an empty `GIT_ASKPASS` stops every askpass program, since git tries
/// none when the first it finds is empty, and `GIT_TERMINAL_PROMPT=0` stops the terminal.
pub fn credential_token(config: &[(OsString, OsString)]) -> Option<String> {
    let mut args: Vec<OsString> = Vec::new();
    for (key, value) in config {
        let is_credential = key.as_bytes().get(..11).is_some_and(|prefix| {
            prefix.eq_ignore_ascii_case(b"credential.") && key.len() > prefix.len()
        });
        if is_credential {
            let mut setting = key.clone();
            setting.push("=");
            setting.push(value);
            args.extend(["-c".into(), setting]);
        }
    }
    args.extend(["-c", "credential.interactive=false", "credential", "fill"].map(OsString::from));
    let mut child = git::command(None, &args)
        .env("GIT_ASKPASS", "")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let asked = child.stdin.take()?.write_all(b"protocol=https\nhost=github.com\n\n");
    let out = child.wait_with_output().ok()?;
    if asked.is_err() || !out.status.success() {
        return None;
    }
    let out = String::from_utf8(out.stdout).ok()?;
    out.lines()
        .find_map(|line| line.strip_prefix("password="))
        .filter(|p| !p.is_empty())
        .map(String::from)
}

/// Ask GitHub for the canonical casing of `user/repo`, with `token` when there is one. Returns
/// `None` on any failure.
///
/// A token GitHub turns down (401 or 403: expired, revoked, or without access to repository
/// metadata) is dropped and the request made again without it, since public repositories need
/// none. The token is never rejected through git's credential helpers: git uses it for more
/// than the API, and h did not store it.
pub fn fetch_repo_info(
    api_base: &str,
    user: &str,
    repo: &str,
    token: Option<&str>,
) -> Option<RepoInfo> {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .build()
        .into();

    let url = format!("{}/repos/{user}/{repo}", api_base.trim_end_matches('/'));
    let get = |token: Option<&str>| {
        let mut request = agent
            .get(&url)
            .header("User-Agent", "h-cli")
            .header("Accept", "application/vnd.github.v3+json");
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        request.call()
    };
    let mut response = match get(token) {
        Err(ureq::Error::StatusCode(401 | 403)) if token.is_some() => get(None).ok()?,
        response => response.ok()?,
    };
    if response.status() != 200 {
        return None;
    }
    let body = response.body_mut().read_to_string().ok()?;
    parse_repo_info(&body)
}

/// Extract `owner.login`, `name` and, for a fork, `parent.full_name` from a GitHub repository
/// JSON document.
pub fn parse_repo_info(body: &str) -> Option<RepoInfo> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let owner = json.get("owner")?.get("login")?.as_str()?;
    let name = json.get("name")?.as_str()?;
    let parent = json
        .get("parent")
        .and_then(|parent| parent.get("full_name"))
        .and_then(|full_name| full_name.as_str())
        .filter(|full_name| full_name.contains('/'))
        .map(String::from);
    Some(RepoInfo { owner: owner.to_string(), name: name.to_string(), parent })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repo_info() {
        let body = r#"{"id": 1, "name": "H", "owner": {"login": "ZimBatm", "id": 2}}"#;
        assert_eq!(
            parse_repo_info(body),
            Some(RepoInfo { owner: "ZimBatm".into(), name: "H".into(), parent: None })
        );
    }

    #[test]
    fn parses_fork_parent() {
        let body = r#"{"name": "nixpkgs", "owner": {"login": "me"}, "fork": true,
                       "parent": {"full_name": "NixOS/nixpkgs", "owner": {"login": "NixOS"}}}"#;
        assert_eq!(parse_repo_info(body).unwrap().parent, Some("NixOS/nixpkgs".into()));
        let body = r#"{"name": "h", "owner": {"login": "me"}, "parent": {"full_name": "bad"}}"#;
        assert_eq!(parse_repo_info(body).unwrap().parent, None);
        let body = r#"{"name": "h", "owner": {"login": "me"}, "parent": null}"#;
        assert_eq!(parse_repo_info(body).unwrap().parent, None);
    }

    #[test]
    fn rejects_incomplete_json() {
        assert_eq!(parse_repo_info(r#"{"name": "h"}"#), None);
        assert_eq!(parse_repo_info(r#"{"owner": {"login": "z"}}"#), None);
        assert_eq!(parse_repo_info(r#"{"owner": {}, "name": "h"}"#), None);
        assert_eq!(parse_repo_info(r#"{"owner": {"login": 3}, "name": "h"}"#), None);
        assert_eq!(parse_repo_info(r#"{"owner": {"login": "z"}, "name": null}"#), None);
        assert_eq!(parse_repo_info(r#"{"message": "Not Found"}"#), None);
    }

    #[test]
    fn rejects_invalid_json() {
        assert_eq!(parse_repo_info(""), None);
        assert_eq!(parse_repo_info("not json"), None);
        assert_eq!(parse_repo_info("[]"), None);
    }

    #[test]
    fn unreachable_api_returns_none() {
        // Port 1 on localhost is essentially never listening; the connection is refused at once.
        assert_eq!(fetch_repo_info("http://127.0.0.1:1", "a", "b", None), None);
        assert_eq!(fetch_repo_info("http://127.0.0.1:1", "a", "b", Some("token")), None);
    }
}
