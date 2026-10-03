//! GitHub API lookups used to canonicalize the casing of `owner/repo`.

use std::cell::RefCell;
use std::collections::HashMap;
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

/// The protocol and host whose credential may go to the API at `api_base`: github.com's for
/// GitHub's own API, since git keeps that token under github.com rather than api.github.com, and
/// otherwise the API's own, so a test server or another host never gets github.com's token.
/// `None` when `api_base` has no usable protocol and host.
fn credential_host(api_base: &str) -> Option<(String, String)> {
    let (protocol, rest) = api_base.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let usable = |s: &str| !s.is_empty() && !s.chars().any(|c| c.is_control() || c == ' ');
    if !usable(protocol) || !usable(host) {
        return None;
    }
    let (protocol, host) = (protocol.to_ascii_lowercase(), host.to_ascii_lowercase());
    if protocol == "https" && host == "api.github.com" {
        return Some((protocol, "github.com".into()));
    }
    Some((protocol, host))
}

/// The token git's credential helpers keep for the API at `api_base` (github.com's, for GitHub's
/// own; see [`credential_host`]), as `git credential fill` gives it, with the `credential.*`
/// settings among `config` (the `-c` pairs h is given), so that each identity's
/// `credential.username` picks its own. `None` when no helper has one.
///
/// Nothing ever prompts: `credential.interactive=false` keeps git from asking at all (git 2.46),
/// and for older versions an empty `GIT_ASKPASS` stops every askpass program, since git tries
/// none when the first it finds is empty, and `GIT_TERMINAL_PROMPT=0` stops the terminal.
pub fn credential_token(api_base: &str, config: &[(OsString, OsString)]) -> Option<String> {
    let (protocol, host) = credential_host(api_base)?;
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
    let request = format!("protocol={protocol}\nhost={host}\n\n");
    let asked = child.stdin.take()?.write_all(request.as_bytes());
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

/// Ask GitHub for the canonical casing of `user/repo`. Returns `None` on any failure.
///
/// The first request carries no token, so public repositories never touch git's credential
/// helpers, and a locked keyring is never asked to unlock for one. Only an answer a token could
/// change asks `token` for one and tries again with it: see [`token_could_help`]. The token is
/// never rejected through git's credential helpers, even when GitHub turns it down: git uses it
/// for more than the API, and h did not store it.
pub fn fetch_repo_info(
    api_base: &str,
    user: &str,
    repo: &str,
    token: impl FnOnce() -> Option<String>,
) -> Option<RepoInfo> {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
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
        request.call().ok()
    };
    let mut response = get(None)?;
    let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok());
    let could_help = token_could_help(
        response.status().as_u16(),
        header("x-ratelimit-remaining"),
        header("retry-after").is_some(),
    );
    if could_help {
        response = get(Some(&token()?))?;
    }
    if response.status() != 200 {
        return None;
    }
    let body = response.body_mut().read_to_string().ok()?;
    parse_repo_info(&body)
}

/// GitHub lookups for one run of h, as [`fetch_repo_info`] makes them, with the token for the
/// identity in `config` (the `-c` pairs h is given) when one could help. Each repository is asked
/// about once, whatever casing it is asked in.
pub struct Lookup {
    api: String,
    config: Vec<(OsString, OsString)>,
    answers: RefCell<HashMap<String, Option<RepoInfo>>>,
}

impl Lookup {
    pub fn new(config: &[(OsString, OsString)]) -> Lookup {
        Lookup { api: api_base(), config: config.to_vec(), answers: RefCell::default() }
    }

    /// GitHub's canonical owner and name for `user/repo`, its current one if it was renamed or
    /// transferred. `None` when GitHub could not be asked or does not know it.
    pub fn repo(&self, user: &str, repo: &str) -> Option<RepoInfo> {
        let key = format!("{user}/{repo}").to_ascii_lowercase();
        if let Some(answer) = self.answers.borrow().get(&key) {
            return answer.clone();
        }
        let token = || credential_token(&self.api, &self.config);
        let answer = fetch_repo_info(&self.api, user, repo, token);
        self.answers.borrow_mut().insert(key, answer.clone());
        answer
    }
}

/// Whether a token could change GitHub's answer `status`: a 404, which is also what a private
/// repository looks like without one, or a rate limit, which is far higher with one. GitHub
/// reports a rate limit as a 429, or as a 403 with `x-ratelimit-remaining: 0` (the primary limit)
/// or a `retry-after` header (a secondary one); any other 403 is a refusal a token won't undo.
fn token_could_help(status: u16, ratelimit_remaining: Option<&str>, retry_after: bool) -> bool {
    match status {
        404 | 429 => true,
        403 => ratelimit_remaining.is_some_and(|n| n.trim() == "0") || retry_after,
        _ => false,
    }
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
        let token = || panic!("no answer at all, so no reason to ask for a token");
        assert_eq!(fetch_repo_info("http://127.0.0.1:1", "a", "b", token), None);
    }

    #[test]
    fn tokens_are_asked_for_only_for_the_apis_own_host() {
        let host = |base: &str| credential_host(base);
        let pair = |p: &str, h: &str| Some((p.to_string(), h.to_string()));
        assert_eq!(host("https://api.github.com"), pair("https", "github.com"));
        assert_eq!(host("https://API.GitHub.com/"), pair("https", "github.com"));
        // Anything else is asked for its own credential, never github.com's.
        assert_eq!(host("http://api.github.com"), pair("http", "api.github.com"));
        assert_eq!(host("http://127.0.0.1:8080"), pair("http", "127.0.0.1:8080"));
        assert_eq!(host("https://ghe.example.com/api/v3"), pair("https", "ghe.example.com"));
        assert_eq!(host("https://me@ghe.example.com/api"), pair("https", "ghe.example.com"));
        assert_eq!(host("https://github.com.example.org"), pair("https", "github.com.example.org"));
        for bad in ["api.github.com", "https://", "://x", "https://a\nb", "https://a b"] {
            assert_eq!(host(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn tokens_are_asked_for_only_when_they_could_help() {
        assert!(token_could_help(404, None, false), "private repositories look missing");
        assert!(token_could_help(429, None, false));
        assert!(token_could_help(403, Some("0"), false), "primary rate limit");
        assert!(token_could_help(403, Some("12"), true), "secondary rate limit");
        assert!(!token_could_help(403, Some("12"), false), "a plain refusal");
        assert!(!token_could_help(403, None, false));
        for status in [200, 301, 401, 500] {
            assert!(!token_could_help(status, Some("0"), true), "{status}");
        }
    }
}
