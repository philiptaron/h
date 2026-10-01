//! GitHub API lookups used to canonicalize the casing of `owner/repo`.

use std::time::Duration;

use ureq::Agent;
use ureq::tls::{RootCerts, TlsConfig};

/// Base URL of the GitHub REST API.
pub const DEFAULT_API: &str = "https://api.github.com";

/// Environment variable that overrides [`DEFAULT_API`] (useful for testing).
pub const API_ENV: &str = "H_GITHUB_API";

/// The canonical owner and name of a repository, as reported by GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub owner: String,
    pub name: String,
}

/// The API base URL, honoring [`API_ENV`].
pub fn api_base() -> String {
    std::env::var(API_ENV).unwrap_or_else(|_| DEFAULT_API.to_string())
}

/// Ask GitHub for the canonical casing of `user/repo`. Returns `None` on any failure.
pub fn fetch_repo_info(api_base: &str, user: &str, repo: &str) -> Option<RepoInfo> {
    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .build()
        .into();

    let url = format!("{}/repos/{user}/{repo}", api_base.trim_end_matches('/'));
    let mut response = agent
        .get(&url)
        .header("User-Agent", "h-cli")
        .header("Accept", "application/vnd.github.v3+json")
        .call()
        .ok()?;
    if response.status() != 200 {
        return None;
    }
    let body = response.body_mut().read_to_string().ok()?;
    parse_repo_info(&body)
}

/// Extract `owner.login` and `name` from a GitHub repository JSON document.
pub fn parse_repo_info(body: &str) -> Option<RepoInfo> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let owner = json.get("owner")?.get("login")?.as_str()?;
    let name = json.get("name")?.as_str()?;
    Some(RepoInfo { owner: owner.to_string(), name: name.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repo_info() {
        let body = r#"{"id": 1, "name": "H", "owner": {"login": "ZimBatm", "id": 2}}"#;
        assert_eq!(
            parse_repo_info(body),
            Some(RepoInfo { owner: "ZimBatm".into(), name: "H".into() })
        );
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
        assert_eq!(fetch_repo_info("http://127.0.0.1:1", "a", "b"), None);
    }
}
