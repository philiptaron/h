//! GitHub's API: canonical casing of `owner/repo`, and the token for an identity.

use std::cell::{OnceCell, RefCell};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;
use std::time::{Duration, Instant};

use ureq::tls::{RootCerts, TlsConfig};
use ureq::{Agent, Body, http};

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

/// Read the API base URL from [`API_ENV`], else [`DEFAULT_API`].
pub fn read_api_base() -> String {
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

/// Ask git's credential helpers for the token they keep for the API at `api_base` (github.com's,
/// for GitHub's own; see [`credential_host`]), as `git credential fill` gives it, with the
/// `credential.*` settings among `config` (the `-c` pairs h is given), so that each identity's
/// `credential.username` picks its own. `None` when no helper has one.
///
/// Nothing ever prompts: `credential.interactive=false` keeps git from asking at all (git 2.46),
/// and for older versions an empty `GIT_ASKPASS` stops every askpass program, since git tries
/// none when the first it finds is empty, and `GIT_TERMINAL_PROMPT=0` stops the terminal.
pub fn ask_credential_token(api_base: &str, config: &[(OsString, OsString)]) -> Option<String> {
    let (protocol, host) = credential_host(api_base)?;
    let is_credential = |key: &OsString| {
        key.as_bytes().get(..11).is_some_and(|prefix| {
            prefix.eq_ignore_ascii_case(b"credential.") && key.len() > prefix.len()
        })
    };
    let credential: Vec<_> = config.iter().filter(|(key, _)| is_credential(key)).cloned().collect();
    let mut args = git::config_args(&credential);
    args.extend(["-c", "credential.interactive=false", "credential", "fill"].map(OsString::from));
    let mut child = git::prepare_command(&args)
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

/// What GitHub said about a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// GitHub knows it, by this name.
    Found(RepoInfo),
    /// GitHub does not know it, even asked with a token: it does not exist, or the token's
    /// account cannot see it.
    Missing,
    /// GitHub could not be asked, or gave no answer to go by.
    Unknown,
}

impl Answer {
    /// The repository GitHub found, if it found one.
    pub fn found(self) -> Option<RepoInfo> {
        match self {
            Answer::Found(info) => Some(info),
            Answer::Missing | Answer::Unknown => None,
        }
    }
}

/// How many repositories a page of a list holds, which is the most GitHub gives.
const PER_PAGE: usize = 100;

/// An owner's repositories on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerRepos {
    /// The owner's login, as GitHub spells it.
    pub login: String,
    pub names: Vec<String>,
}

/// What GitHub said about an owner's repositories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listed {
    Found(OwnerRepos),
    /// There is no such user or organization, as far as the token's account can see.
    Missing,
    /// GitHub could not be asked, or gave no answer to go by.
    Unknown,
}

/// Asks for one page of the names of the repositories `$login` owns, after the cursor `$after`.
const LIST_REPOS_QUERY: &str = "query($login: String!, $after: String) { \
    repositoryOwner(login: $login) { login \
    repositories(first: 100, after: $after, ownerAffiliations: OWNER, \
    orderBy: { field: PUSHED_AT, direction: DESC }) { \
    nodes { name } pageInfo { hasNextPage endCursor } } } }";

/// The GraphQL endpoint of the REST API at `api`: `/graphql` beside it, or for GitHub Enterprise
/// Server, whose REST API is at `/api/v3`, `/api/graphql`.
fn graphql_url(api: &str) -> String {
    match api.strip_suffix("/v3") {
        Some(base) if base.ends_with("/api") => format!("{base}/graphql"),
        _ => format!("{api}/graphql"),
    }
}

/// A page of an owner's repositories from [`LIST_REPOS_QUERY`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReposPage {
    Repos(OwnerReposPage),
    NoSuchOwner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OwnerReposPage {
    login: String,
    names: Vec<String>,
    /// The cursor to ask for the next page after, when there is one.
    next: Option<String>,
}

/// Read a reply to [`LIST_REPOS_QUERY`]. `None` when it has errors, or not what was asked for.
fn parse_repos_page(body: &str) -> Option<ReposPage> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    if json.get("errors").is_some() {
        return None;
    }
    let owner = json.get("data")?.get("repositoryOwner")?;
    if owner.is_null() {
        return Some(ReposPage::NoSuchOwner);
    }
    let repos = owner.get("repositories")?;
    let names = repos.get("nodes")?.as_array()?.iter();
    let names = names.map(|node| Some(node.get("name")?.as_str()?.to_string()));
    let page_info = repos.get("pageInfo")?;
    let next = match page_info.get("hasNextPage")?.as_bool()? {
        true => Some(page_info.get("endCursor")?.as_str()?.to_string()),
        false => None,
    };
    Some(ReposPage::Repos(OwnerReposPage {
        login: owner.get("login")?.as_str()?.to_string(),
        names: names.collect::<Option<_>>()?,
        next,
    }))
}

/// The body of `response` when it is a 200, as [`Reply::Body`].
fn read_reply(mut response: http::Response<Body>) -> Reply {
    if response.status() != 200 {
        return Reply::Failed;
    }
    response.body_mut().read_to_string().map_or(Reply::Failed, Reply::Body)
}

/// What GitHub replied to a request.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reply {
    /// A 200, with this body.
    Body(String),
    /// A 404 to a request with a token: there is nothing there that the token's account can see.
    Missing,
    /// No reply, or none to go by.
    Failed,
}

/// GitHub's API, as one run of h talks to it, with the token for the identity in `config` (the
/// `-c` pairs h is given) when one could help. Each repository is asked about once, whatever
/// casing it is asked in, and the token is asked for at most once.
pub struct Lookup {
    api: String,
    agent: Agent,
    config: Vec<(OsString, OsString)>,
    token: OnceCell<Option<String>>,
    answers: RefCell<HashMap<String, Answer>>,
}

impl Lookup {
    /// Talk to the API at [`read_api_base`].
    pub fn new(config: &[(OsString, OsString)]) -> Lookup {
        Lookup::with_api(&read_api_base(), config)
    }

    /// Talk to the API at `api`.
    pub fn with_api(api: &str, config: &[(OsString, OsString)]) -> Lookup {
        let agent = Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
            .build()
            .into();
        Lookup {
            api: api.trim_end_matches('/').to_string(),
            agent,
            config: config.to_vec(),
            token: OnceCell::new(),
            answers: RefCell::default(),
        }
    }

    /// Ask GitHub about `user/repo`, once whatever the casing: its canonical owner and name, its
    /// current one if it was renamed or transferred, and its parent if it is a fork.
    ///
    /// Only a 404 to a request with a token is [`Answer::Missing`]: without one, a private
    /// repository looks the same, and every other failure is [`Answer::Unknown`].
    pub fn ask_github(&self, user: &str, repo: &str) -> Answer {
        let key = format!("{user}/{repo}").to_ascii_lowercase();
        if let Some(answer) = self.answers.borrow().get(&key) {
            return answer.clone();
        }
        let answer = match self.get(&format!("/repos/{user}/{repo}")) {
            Reply::Body(body) => parse_repo_info(&body).map_or(Answer::Unknown, Answer::Found),
            Reply::Missing => Answer::Missing,
            Reply::Failed => Answer::Unknown,
        };
        self.answers.borrow_mut().insert(key, answer.clone());
        answer
    }

    /// Get `path` from the API.
    ///
    /// The first request carries no token, so public repositories never touch git's credential
    /// helpers, and a locked keyring is never asked to unlock for one. Only an answer a token
    /// could change (see [`token_could_help`]) asks git for one, as [`ask_credential_token`]
    /// does, and tries again with it. The token is never rejected through git's credential
    /// helpers, even when GitHub turns it down: git uses it for more than the API, and h did not
    /// store it.
    fn get(&self, path: &str) -> Reply {
        let Some(mut response) = self.send(path, None) else {
            return Reply::Failed;
        };
        let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok());
        let could_help = token_could_help(
            response.status().as_u16(),
            header("x-ratelimit-remaining"),
            header("retry-after").is_some(),
        );
        if could_help {
            let Some(again) = self.ask_token().and_then(|token| self.send(path, Some(token)))
            else {
                return Reply::Failed;
            };
            response = again;
            if response.status() == 404 {
                return Reply::Missing;
            }
        }
        read_reply(response)
    }

    /// Get `path` from the API with the token from the start, if there is one: for lists, where
    /// a token's account may see more than anyone can, and an answer without it would look
    /// whole. A 404 is [`Reply::Missing`] either way.
    fn get_with_token(&self, path: &str) -> Reply {
        match self.send(path, self.ask_token()) {
            Some(response) if response.status() == 404 => Reply::Missing,
            Some(response) => read_reply(response),
            None => Reply::Failed,
        }
    }

    /// Ask git for the token, the first time only.
    fn ask_token(&self) -> Option<&str> {
        let token = self.token.get_or_init(|| ask_credential_token(&self.api, &self.config));
        token.as_deref()
    }

    /// Send a request for `path`, with `token` if there is one. `None` when no reply came.
    fn send(&self, path: &str, token: Option<&str>) -> Option<http::Response<Body>> {
        let mut request = self
            .agent
            .get(format!("{}{path}", self.api))
            .header("User-Agent", "h-cli")
            .header("Accept", "application/vnd.github.v3+json");
        if let Some(token) = token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        request.call().ok()
    }

    /// List the repositories `owner`, a user or an organization, has on GitHub, as far as the
    /// identity's token lets it see, the most recently pushed to first. Pages of them are asked
    /// for until there are no more or `until` has passed, so an owner with more than can be
    /// listed in time is listed in part, with its liveliest repositories.
    ///
    /// With a token, the GraphQL API is asked, for the names alone, which is several times
    /// faster than the REST API's whole records, and sees everything the token's account can: an
    /// organization's private and internal repositories, and a user's private ones. Without
    /// one, or should that fail, the REST API is, which shows only a user's public repositories.
    pub fn list_repos(&self, owner: &str, until: Instant) -> Listed {
        if let Some(token) = self.ask_token()
            && let Some(listed) = self.list_repos_with_graphql(token, owner, until)
        {
            return listed;
        }
        self.list_repos_with_rest(owner, until)
    }

    /// List `owner`'s repositories through the GraphQL API with `token`. `None` when that fails.
    fn list_repos_with_graphql(&self, token: &str, owner: &str, until: Instant) -> Option<Listed> {
        let (mut login, mut names, mut after) = (None, Vec::new(), None);
        for page in 1.. {
            if page > 1 && Instant::now() >= until {
                break;
            }
            let variables = serde_json::json!({ "login": owner, "after": after });
            let body = self.post_graphql(token, LIST_REPOS_QUERY, variables)?;
            let page = match parse_repos_page(&body)? {
                ReposPage::NoSuchOwner => return Some(Listed::Missing),
                ReposPage::Repos(page) => page,
            };
            login.get_or_insert(page.login);
            names.extend(page.names);
            after = page.next;
            if after.is_none() {
                break;
            }
        }
        Some(Listed::Found(OwnerRepos { login: login?, names }))
    }

    /// Post a GraphQL `query`, with `variables`, to the API with `token`, and return the body of
    /// the reply. `None` when there is no reply, or it is not a 200.
    fn post_graphql(
        &self,
        token: &str,
        query: &str,
        variables: serde_json::Value,
    ) -> Option<String> {
        let body = serde_json::json!({ "query": query, "variables": variables }).to_string();
        let response = self
            .agent
            .post(graphql_url(&self.api))
            .header("User-Agent", "h-cli")
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .send(body.as_str())
            .ok()?;
        match read_reply(response) {
            Reply::Body(body) => Some(body),
            Reply::Missing | Reply::Failed => None,
        }
    }

    /// List `owner`'s repositories through the REST API: an organization's from
    /// `/orgs/<owner>/repos`, else a user's public ones from `/users/<owner>/repos`.
    fn list_repos_with_rest(&self, owner: &str, until: Instant) -> Listed {
        'kinds: for kind in ["orgs", "users"] {
            let (mut login, mut names) = (None, Vec::new());
            for page in 1.. {
                if page > 1 && Instant::now() >= until {
                    break;
                }
                let path =
                    format!("/{kind}/{owner}/repos?sort=pushed&per_page={PER_PAGE}&page={page}");
                let repos = match self.get_with_token(&path) {
                    Reply::Body(body) => parse_repo_list(&body),
                    Reply::Missing if page == 1 => continue 'kinds,
                    Reply::Missing | Reply::Failed => None,
                };
                let Some(repos) = repos else {
                    return Listed::Unknown;
                };
                let last = repos.len() < PER_PAGE;
                for (owner, name) in repos {
                    login.get_or_insert(owner);
                    names.push(name);
                }
                if last {
                    break;
                }
            }
            // An owner with no repositories says nothing of how it spells its name.
            let login = login.unwrap_or_else(|| owner.to_string());
            return Listed::Found(OwnerRepos { login, names });
        }
        Listed::Missing
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

/// Extract each repository's `owner.login` and `name` from a GitHub list of repositories.
fn parse_repo_list(body: &str) -> Option<Vec<(String, String)>> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let repo = |repo: &serde_json::Value| {
        let owner = repo.get("owner")?.get("login")?.as_str()?;
        Some((owner.to_string(), repo.get("name")?.as_str()?.to_string()))
    };
    json.as_array()?.iter().map(repo).collect()
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
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

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
    fn parses_repo_lists() {
        let body = r#"[{"name": "nixpkgs", "owner": {"login": "NixOS"}},
                       {"name": "nix", "owner": {"login": "NixOS"}, "fork": false}]"#;
        let want = [("NixOS", "nixpkgs"), ("NixOS", "nix")].map(|(o, n)| (o.into(), n.into()));
        assert_eq!(parse_repo_list(body), Some(want.to_vec()));
        assert_eq!(parse_repo_list("[]"), Some(vec![]));
        assert_eq!(parse_repo_list(r#"[{"name": "x"}]"#), None);
        assert_eq!(parse_repo_list(r#"{"message": "Not Found"}"#), None);
    }

    #[test]
    fn finds_the_graphql_endpoint() {
        assert_eq!(graphql_url("https://api.github.com"), "https://api.github.com/graphql");
        assert_eq!(
            graphql_url("https://ghe.example.com/api/v3"),
            "https://ghe.example.com/api/graphql"
        );
        assert_eq!(graphql_url("http://127.0.0.1:8080"), "http://127.0.0.1:8080/graphql");
    }

    #[test]
    fn parses_pages_of_repositories() {
        let page = |next: &str| {
            format!(
                r#"{{"data": {{"repositoryOwner": {{"login": "NixOS", "repositories": {{
                    "nodes": [{{"name": "nix"}}, {{"name": "nixpkgs"}}],
                    "pageInfo": {{{next}}}}}}}}}}}"#
            )
        };
        let repos = |next: Option<&str>| {
            Some(ReposPage::Repos(OwnerReposPage {
                login: "NixOS".into(),
                names: vec!["nix".into(), "nixpkgs".into()],
                next: next.map(String::from),
            }))
        };
        let more = page(r#""hasNextPage": true, "endCursor": "Y3Vy""#);
        assert_eq!(parse_repos_page(&more), repos(Some("Y3Vy")));
        let last = page(r#""hasNextPage": false, "endCursor": null"#);
        assert_eq!(parse_repos_page(&last), repos(None));
        let missing = r#"{"data": {"repositoryOwner": null}}"#;
        assert_eq!(parse_repos_page(missing), Some(ReposPage::NoSuchOwner));
        let error = r#"{"data": null, "errors": [{"message": "Bad credentials"}]}"#;
        assert_eq!(parse_repos_page(error), None);
        assert_eq!(parse_repos_page(r#"{"message": "Bad credentials"}"#), None);
    }

    /// Serve every request, after `delay`, with a full page of repositories, and note the paths
    /// asked for.
    fn serve_endless_pages(delay: Duration) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let paths = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&paths);
        let page: Vec<String> = (0..PER_PAGE)
            .map(|i| format!(r#"{{"name": "r{i}", "owner": {{"login": "Big"}}}}"#))
            .collect();
        let body = format!("[{}]", page.join(","));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = String::new();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                while reader.read_line(&mut request).unwrap_or(0) > 2 {}
                let path = request.split_whitespace().nth(1).unwrap_or_default().to_string();
                log.lock().unwrap().push(path);
                std::thread::sleep(delay);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (url, paths)
    }

    #[test]
    fn lists_pages_until_the_time_is_up() {
        let (url, paths) = serve_endless_pages(Duration::from_millis(50));
        let lookup = Lookup::with_api(&url, &[]);
        lookup.token.set(None).unwrap(); // no token, so no git credential helpers, and REST
        let started = Instant::now();
        let Listed::Found(repos) = lookup.list_repos("big", started + Duration::from_millis(300))
        else {
            panic!("listed nothing");
        };
        assert!(started.elapsed() < Duration::from_secs(2), "stopped in time");
        let pages = paths.lock().unwrap().clone();
        assert!((2..20).contains(&pages.len()), "{pages:?}");
        assert_eq!(repos.names.len(), pages.len() * PER_PAGE);
        assert_eq!(repos.login, "Big");
        assert_eq!(pages[1], "/orgs/big/repos?sort=pushed&per_page=100&page=2");
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
        let lookup = Lookup::with_api("http://127.0.0.1:1", &[]);
        assert_eq!(lookup.ask_github("a", "b"), Answer::Unknown);
        assert_eq!(lookup.token.get(), None, "no answer at all, so no reason to ask for a token");
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
