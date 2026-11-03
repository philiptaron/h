//! Turning the user's search term into a directory (and, if needed, a URL to clone).

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::github::Answer;
use crate::search::search;

/// What a search term refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A GitHub repository, from `user/repo` or any GitHub URL.
    GitHub { user: String, repo: String },
    /// A repository on another host, cloned from `url` into `<code-root>/<host>/<path>`.
    Remote { url: String, host: String, path: String },
    /// A bare project name to search for under the code root.
    Name(String),
}

/// Why a search term could not be turned into a [`Target`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// The term does not look like a name, `user/repo`, or URL.
    UnknownPattern,
    /// The term looks like an SCP-style URL but is missing its `:path`.
    NotFound,
}

/// Which decides the casing of a GitHub repository's `user/repo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Casing {
    /// The directory the repository already has under the code root, in any casing. GitHub is
    /// asked only when there is none, that is, only before cloning.
    Local,
    /// GitHub, always: the store names upstreams by their canonical casing.
    GitHub,
}

/// Where a term leads: a directory, plus the URL to clone it from if it does not exist yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub path: PathBuf,
    pub clone_url: Option<String>,
    /// The name of the repository in an object store: its path relative to the code root, such
    /// as `github.com/NixOS/nixpkgs`. `None` when the path is not valid UTF-8.
    pub remote: Option<String>,
    /// The URL of the repository this one was forked from, when GitHub reports one.
    pub upstream_url: Option<String>,
}

fn is_valid_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')
}

fn is_simple_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_valid_name_char)
}

/// If `host`/`path` names a GitHub repository, return its `(user, repo)`, minus any `.git`.
pub fn github_repo(host: &str, path: &str) -> Option<(String, String)> {
    if host != "github.com" {
        return None;
    }
    let (user, repo) = path.split_once('/')?;
    if !is_simple_name(user) || !is_simple_name(repo) {
        return None;
    }
    let repo = match repo.strip_suffix(".git") {
        Some(stem) if !stem.is_empty() => stem,
        _ => repo,
    };
    Some((user.to_string(), repo.to_string()))
}

/// Classify a search term.
pub fn parse_term(term: &str) -> Result<Target, ParseError> {
    let remote = |host: &str, path: &str| {
        let host = host.to_ascii_lowercase();
        match github_repo(&host, path) {
            Some((user, repo)) => Target::GitHub { user, repo },
            None => Target::Remote { url: term.to_string(), host, path: path.to_string() },
        }
    };

    if let Some((user, repo)) = github_repo("github.com", term) {
        Ok(Target::GitHub { user, repo })
    } else if let Some((_, rest)) = term.split_once("://") {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        // The user (`ssh://git@github.com/...`) is how to log in, not where the repository is.
        let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
        Ok(remote(host, path))
    } else if term.starts_with("git@") || term.starts_with("gitea@") {
        let (_, rest) = term.split_once('@').expect("term starts with user@");
        let (host, path) = rest.split_once(':').ok_or(ParseError::NotFound)?;
        Ok(remote(host, path))
    } else if is_simple_name(term) {
        Ok(Target::Name(term.to_string()))
    } else {
        Err(ParseError::UnknownPattern)
    }
}

/// Strip a trailing `.git` from a string, provided something precedes it.
fn strip_git_suffix(s: &str) -> &str {
    match s.strip_suffix(".git") {
        Some(stem) if !stem.is_empty() => stem,
        _ => s,
    }
}

/// The store's name for the repository at `host` and `path`: the two joined by `/`, without
/// empty segments (`file:///abs/path` has an empty host) and with each segment escaped as
/// [`escape_segment`] does, since git rejects remote names that are not valid in a ref.
pub fn remote_name(host: &str, path: &str) -> String {
    let segments: Vec<&str> =
        std::iter::once(host).chain(path.split('/')).filter(|s| !s.is_empty()).collect();
    escape_name(strip_git_suffix(&segments.join("/")))
}

/// The store's name for the repository a remote fetches from at `url`, and the URL the store
/// should fetch it from, as `h store add <url>` would add it: for GitHub, the HTTPS URL of
/// `<user>/<repo>` whatever form `url` takes. `None` for what is not a repository URL, such as a
/// local path.
pub fn store_upstream(url: &str) -> Option<(String, String)> {
    match parse_term(url).ok()? {
        Target::GitHub { user, repo } => Some((
            remote_name("github.com", &format!("{user}/{repo}")),
            format!("https://github.com/{user}/{repo}.git"),
        )),
        Target::Remote { url, host, path } => Some((remote_name(&host, &path), url)),
        Target::Name(_) => None,
    }
}

/// `name` without empty segments, and with each segment escaped as [`escape_segment`] does.
fn escape_name(name: &str) -> String {
    let segments: Vec<String> =
        name.split('/').filter(|s| !s.is_empty()).map(escape_segment).collect();
    segments.join("/")
}

/// One segment of a store name, made valid in a ref: characters git forbids become `_`, `..`
/// and `@{` are broken up, and a segment starting with `.` (as `~/.local` does) or ending with
/// `.` or `.lock` gets a `_` added at that end. A segment of just `-` becomes `_-`, since the
/// store uses `/-/` to mark where a name ends and its refs begin.
pub fn escape_segment(segment: &str) -> String {
    if segment == "-" {
        return "_-".into();
    }
    let mut out: String = segment
        .chars()
        .map(|c| if c.is_ascii_control() || " ~^:?*[\\".contains(c) { '_' } else { c })
        .collect();
    out = out.replace("..", "._").replace("@{", "@_");
    if out.starts_with('.') {
        out.insert(0, '_');
    }
    if out.ends_with('.') || out.ends_with(".lock") {
        out.push('_');
    }
    out
}

/// Strip a trailing `.git` from a path, provided something precedes it.
pub fn strip_git_extension(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let bytes = path.as_os_str().as_bytes();
    match bytes.strip_suffix(b".git") {
        Some(stem) if !stem.is_empty() => PathBuf::from(OsStr::from_bytes(stem)),
        _ => path.to_path_buf(),
    }
}

/// Join `parts` onto `root` with `/`, without letting absolute or empty parts reset the path.
fn concat_path(root: impl AsRef<Path>, parts: &[&str]) -> PathBuf {
    let mut bytes = root.as_ref().as_os_str().to_owned().into_vec();
    for part in parts {
        bytes.push(b'/');
        bytes.extend_from_slice(part.as_bytes());
    }
    PathBuf::from(OsString::from_vec(bytes))
}

/// The names under `dir` that are directories spelled like `name` in any casing, sorted.
fn case_variants(dir: &Path, name: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|entry| entry.eq_ignore_ascii_case(name) && dir.join(entry).is_dir())
        .collect();
    names.sort();
    names
}

/// The owner and name of the directory GitHub repository `user/repo` already has under
/// `code_root`, in any casing, since GitHub ignores case: the one spelled exactly as given when
/// it exists, and otherwise the first in sorted order (which puts capitals first, as in the
/// `NixOS/nixpkgs` an earlier lookup would have chosen over a typed `nixos/nixpkgs`).
pub fn find_github_dir(
    code_root: impl AsRef<Path>,
    user: &str,
    repo: &str,
) -> Option<(String, String)> {
    let host = concat_path(code_root, &["github.com"]);
    let found: Vec<(String, String)> = case_variants(&host, user)
        .into_iter()
        .flat_map(|owner| {
            let names = case_variants(&host.join(&owner), repo);
            names.into_iter().map(move |name| (owner.clone(), name))
        })
        .collect();
    let exact = found.iter().position(|(owner, name)| owner == user && name == repo);
    found.into_iter().nth(exact.unwrap_or(0))
}

/// Resolve `term` against `code_root`.
///
/// `lookup` is consulted for GitHub repositories to fix up the casing of `user/repo`: always
/// with [`Casing::GitHub`], and with [`Casing::Local`] only when the repository has no directory
/// in any casing yet. Either way an existing directory for the resulting name, in any casing, is
/// where it leads. A repository GitHub does not know even when asked with a token, and that has
/// no directory, is not found: cloning it could only fail. On failure, returns the message to
/// show the user.
pub fn resolve(
    code_root: impl AsRef<Path>,
    term: &str,
    casing: Casing,
    lookup: impl FnOnce(&str, &str) -> Answer,
) -> Result<Resolution, String> {
    let code_root = code_root.as_ref();
    let not_found = || format!("{term} not found");
    let target = parse_term(term).map_err(|err| match err {
        ParseError::UnknownPattern => format!("Unknown pattern for {term}"),
        ParseError::NotFound => not_found(),
    })?;

    match target {
        Target::GitHub { user, repo } => {
            let existing = match casing {
                Casing::Local => find_github_dir(code_root, &user, &repo),
                Casing::GitHub => None,
            };
            let (user, repo, parent) = match existing {
                Some((user, repo)) => (user, repo, None),
                None => match lookup(&user, &repo) {
                    Answer::Found(info) => (info.owner, info.name, info.parent),
                    Answer::Missing if find_github_dir(code_root, &user, &repo).is_none() => {
                        let msg = "not found on GitHub, even with this identity's token";
                        return Err(format!("{user}/{repo} {msg}"));
                    }
                    Answer::Missing | Answer::Unknown => (user, repo, None),
                },
            };
            // A checkout of the resulting name wins in any casing too: with Casing::GitHub, or
            // when GitHub gives a renamed repository's new name.
            let (owner, name) =
                find_github_dir(code_root, &user, &repo).unwrap_or((user.clone(), repo.clone()));
            Ok(Resolution {
                path: concat_path(code_root, &["github.com", &owner, &name]),
                clone_url: Some(format!("https://github.com/{user}/{repo}.git")),
                remote: Some(remote_name("github.com", &format!("{user}/{repo}"))),
                upstream_url: parent.map(|parent| format!("https://github.com/{parent}.git")),
            })
        }
        Target::Remote { url, host, path } => Ok(Resolution {
            path: strip_git_extension(concat_path(code_root, &[&host, &path])),
            clone_url: Some(url),
            remote: Some(remote_name(&host, &path)),
            upstream_url: None,
        }),
        Target::Name(name) => {
            let path = search(code_root, &name).ok_or_else(not_found)?;
            let remote =
                path.strip_prefix(code_root).ok().and_then(|p| p.to_str()).map(escape_name);
            Ok(Resolution { path, clone_url: None, remote, upstream_url: None })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::RepoInfo;
    use std::fs;

    fn github(user: &str, repo: &str) -> Result<Target, ParseError> {
        Ok(Target::GitHub { user: user.into(), repo: repo.into() })
    }

    fn remote(url: &str, host: &str, path: &str) -> Result<Target, ParseError> {
        Ok(Target::Remote { url: url.into(), host: host.into(), path: path.into() })
    }

    /// A lookup GitHub has no answer for.
    fn unknown(_: &str, _: &str) -> Answer {
        Answer::Unknown
    }

    /// A lookup GitHub answers 404 to, even with the token.
    fn missing(_: &str, _: &str) -> Answer {
        Answer::Missing
    }

    /// A lookup that must not be made.
    fn not_asked(_: &str, _: &str) -> Answer {
        panic!("GitHub was asked")
    }

    /// A lookup that finds `full`, as `owner/name`, whatever it is asked about.
    fn found(full: &str) -> impl FnOnce(&str, &str) -> Answer {
        let info = repo_info(full, None);
        move |_, _| Answer::Found(info)
    }

    /// A lookup that finds `full`, a fork of `parent`.
    fn fork_of(full: &str, parent: &str) -> impl FnOnce(&str, &str) -> Answer {
        let info = repo_info(full, Some(parent));
        move |_, _| Answer::Found(info)
    }

    fn repo_info(full: &str, parent: Option<&str>) -> RepoInfo {
        let (owner, name) = full.split_once('/').expect("owner/name");
        RepoInfo { owner: owner.into(), name: name.into(), parent: parent.map(String::from) }
    }

    #[test]
    fn parses_user_repo_shorthand() {
        assert_eq!(parse_term("zimbatm/h"), github("zimbatm", "h"));
        assert_eq!(parse_term("a-b_c.d/e.f-g_h"), github("a-b_c.d", "e.f-g_h"));
        assert_eq!(parse_term("zimbatm/h.git"), github("zimbatm", "h"));
    }

    #[test]
    fn rejects_malformed_shorthand() {
        assert_eq!(parse_term("/h"), Err(ParseError::UnknownPattern));
        assert_eq!(parse_term("zimbatm/"), Err(ParseError::UnknownPattern));
        assert_eq!(parse_term("a/b/c"), Err(ParseError::UnknownPattern));
        assert_eq!(parse_term("a b/c"), Err(ParseError::UnknownPattern));
        assert_eq!(parse_term("a/b c"), Err(ParseError::UnknownPattern));
    }

    #[test]
    fn parses_simple_names() {
        assert_eq!(parse_term("h"), Ok(Target::Name("h".into())));
        assert_eq!(parse_term("My.Project-1_x"), Ok(Target::Name("My.Project-1_x".into())));
        assert_eq!(parse_term(""), Err(ParseError::UnknownPattern));
        assert_eq!(parse_term("caf\u{e9}"), Err(ParseError::UnknownPattern));
        assert_eq!(parse_term("a b"), Err(ParseError::UnknownPattern));
    }

    #[test]
    fn parses_github_urls() {
        assert_eq!(parse_term("https://github.com/zimbatm/h"), github("zimbatm", "h"));
        assert_eq!(parse_term("https://github.com/zimbatm/h.git"), github("zimbatm", "h"));
        assert_eq!(parse_term("https://GitHub.COM/zimbatm/h"), github("zimbatm", "h"));
        assert_eq!(parse_term("git://github.com/zimbatm/h"), github("zimbatm", "h"));
        assert_eq!(parse_term("git@github.com:zimbatm/h.git"), github("zimbatm", "h"));
        assert_eq!(parse_term("git@GITHUB.com:zimbatm/h"), github("zimbatm", "h"));
        assert_eq!(parse_term("ssh://git@github.com/zimbatm/h"), github("zimbatm", "h"));
        assert_eq!(parse_term("ssh://git@github.com/zimbatm/h.git"), github("zimbatm", "h"));
        assert_eq!(parse_term("https://me:tok@github.com/zimbatm/h"), github("zimbatm", "h"));
    }

    #[test]
    fn github_urls_with_extra_segments_are_generic() {
        let url = "https://github.com/zimbatm/h/tree/main";
        assert_eq!(parse_term(url), remote(url, "github.com", "zimbatm/h/tree/main"));
        let url = "https://github.com/zimbatm/h/";
        assert_eq!(parse_term(url), remote(url, "github.com", "zimbatm/h/"));
    }

    #[test]
    fn parses_other_urls() {
        let url = "https://GitLab.com/group/sub/project.git";
        assert_eq!(parse_term(url), remote(url, "gitlab.com", "group/sub/project.git"));
        // The user is no part of the host, so it is no part of the name either.
        let url = "ssh://git@git.lix.systems/me/lix";
        assert_eq!(parse_term(url), remote(url, "git.lix.systems", "me/lix"));
        let url = "git+ssh://me@git.launchpad.net/ubuntu/+source/x";
        assert_eq!(parse_term(url), remote(url, "git.launchpad.net", "ubuntu/+source/x"));
        let url = "https://example.com";
        assert_eq!(parse_term(url), remote(url, "example.com", ""));
        let url = "file:///srv/git/repo.git";
        assert_eq!(parse_term(url), remote(url, "", "srv/git/repo.git"));
    }

    #[test]
    fn parses_scp_style_urls() {
        let url = "git@gitlab.com:group/project.git";
        assert_eq!(parse_term(url), remote(url, "gitlab.com", "group/project.git"));
        let url = "gitea@Git.Example.org:me/thing";
        assert_eq!(parse_term(url), remote(url, "git.example.org", "me/thing"));
        assert_eq!(parse_term("git@gitlab.com"), Err(ParseError::NotFound));
    }

    #[test]
    fn concat_path_keeps_parts_under_root() {
        assert_eq!(concat_path("/code", &["host", "/abs"]), PathBuf::from("/code/host//abs"));
        assert_eq!(concat_path("/code", &["", "srv/x"]), PathBuf::from("/code//srv/x"));
    }

    #[test]
    fn resolves_github_with_lookup() {
        let res = resolve("/code", "zimbatm/H", Casing::Local, |user, repo| {
            assert_eq!((user, repo), ("zimbatm", "H"));
            Answer::Found(repo_info("ZimBatm/h", None))
        });
        assert_eq!(
            res,
            Ok(Resolution {
                path: PathBuf::from("/code/github.com/ZimBatm/h"),
                clone_url: Some("https://github.com/ZimBatm/h.git".into()),
                remote: Some("github.com/ZimBatm/h".into()),
                upstream_url: None,
            })
        );
    }

    #[test]
    fn resolves_github_without_lookup() {
        let res = resolve("/code", "git@github.com:a/b.git", Casing::Local, unknown);
        assert_eq!(
            res,
            Ok(Resolution {
                path: PathBuf::from("/code/github.com/a/b"),
                clone_url: Some("https://github.com/a/b.git".into()),
                remote: Some("github.com/a/b".into()),
                upstream_url: None,
            })
        );
    }

    #[test]
    fn checkouts_in_any_casing_win_without_a_lookup() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("github.com/NixOS/nixpkgs")).unwrap();
        let res = resolve(tmp.path(), "nixos/NIXPKGS", Casing::Local, not_asked);
        assert_eq!(
            res,
            Ok(Resolution {
                path: tmp.path().join("github.com/NixOS/nixpkgs"),
                clone_url: Some("https://github.com/NixOS/nixpkgs.git".into()),
                remote: Some("github.com/NixOS/nixpkgs".into()),
                upstream_url: None,
            })
        );

        // The store always asks GitHub, and still finds the checkout.
        let res = resolve(tmp.path(), "nixos/nixpkgs", Casing::GitHub, found("NixOS/nixpkgs"));
        assert_eq!(res.unwrap().path, tmp.path().join("github.com/NixOS/nixpkgs"));

        // A repository GitHub has renamed is found under its new name, in any casing.
        let res = resolve(tmp.path(), "old/name", Casing::Local, found("nixos/nixpkgs"));
        assert_eq!(res.unwrap().path, tmp.path().join("github.com/NixOS/nixpkgs"));
    }

    #[test]
    fn exact_casing_wins_then_sorted_order() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("github.com/nixos/nixpkgs")).unwrap();
        fs::create_dir_all(root.join("github.com/NixOS/nixpkgs")).unwrap();
        if fs::read_dir(root.join("github.com")).unwrap().count() < 2 {
            return; // a case-insensitive filesystem cannot hold both
        }
        fs::create_dir_all(root.join("github.com/Nixos/Nixpkgs")).unwrap();
        fs::write(root.join("github.com/NixOS/NIXPKGS"), "a file, not a checkout").unwrap();
        let found = |user, repo| find_github_dir(root, user, repo);
        assert_eq!(found("nixos", "nixpkgs"), Some(("nixos".into(), "nixpkgs".into())));
        assert_eq!(found("Nixos", "Nixpkgs"), Some(("Nixos".into(), "Nixpkgs".into())));
        assert_eq!(found("NIXOS", "nixpkgs"), Some(("NixOS".into(), "nixpkgs".into())));
        assert_eq!(found("nixos", "other"), None);
        assert_eq!(find_github_dir("/nonexistent/h-root", "a", "b"), None);
    }

    #[test]
    fn repositories_github_does_not_have_are_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let res = resolve(tmp.path(), "me/gone", Casing::Local, missing);
        assert_eq!(res, Err("me/gone not found on GitHub, even with this identity's token".into()));

        // A checkout is still where it leads, whatever GitHub says.
        fs::create_dir_all(tmp.path().join("github.com/me/gone")).unwrap();
        let res = resolve(tmp.path(), "me/gone", Casing::GitHub, missing);
        assert_eq!(res.unwrap().path, tmp.path().join("github.com/me/gone"));
    }

    #[test]
    fn resolves_forks_with_their_upstream() {
        let lookup = fork_of("me/nixpkgs", "NixOS/nixpkgs");
        let res = resolve("/code", "me/nixpkgs", Casing::Local, lookup);
        assert_eq!(res.unwrap().upstream_url, Some("https://github.com/NixOS/nixpkgs.git".into()));
    }

    #[test]
    fn remote_names_have_no_empty_segments() {
        let res = resolve("/code", "file:///srv/git/proj.git", Casing::Local, unknown);
        assert_eq!(res.unwrap().remote, Some("srv/git/proj".into()));
        let res = resolve("/code", "https://host//a/b/", Casing::Local, unknown);
        assert_eq!(res.unwrap().remote, Some("host/a/b".into()));
    }

    #[test]
    fn remote_names_are_valid_in_refs() {
        let url = "file:///home/me/.local/x.lock.git";
        let res = resolve("/code", url, Casing::Local, unknown);
        assert_eq!(res.unwrap().remote, Some("home/me/_.local/x.lock_".into()));
        let res = resolve("/code", "owner/.github", Casing::Local, unknown);
        assert_eq!(res.unwrap().remote, Some("github.com/owner/_.github".into()));
        for (segment, escaped) in [
            ("plain-name_1.2", "plain-name_1.2"),
            (".hidden", "_.hidden"),
            ("trailing.", "trailing._"),
            ("a..b", "a._b"),
            ("a...b", "a._.b"),
            ("x@{y}", "x@_y}"),
            ("~me", "_me"),
            ("-", "_-"),
            ("-x", "-x"),
            ("sp ace:col?*[\\", "sp_ace_col____"),
        ] {
            assert_eq!(escape_segment(segment), escaped, "{segment}");
            let refname = format!("refs/remotes/host/{escaped}/main");
            let ok = std::process::Command::new("git")
                .args(["check-ref-format", &refname])
                .status()
                .unwrap()
                .success();
            assert!(ok, "{refname}");
        }
    }

    #[test]
    fn resolves_remote_and_strips_git() {
        let url = "https://gitlab.com/group/project.git";
        let res = resolve("/code", url, Casing::Local, not_asked);
        assert_eq!(
            res,
            Ok(Resolution {
                path: PathBuf::from("/code/gitlab.com/group/project"),
                clone_url: Some(url.into()),
                remote: Some("gitlab.com/group/project".into()),
                upstream_url: None,
            })
        );
    }

    #[test]
    fn resolves_names_by_search() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("example.com/owner/proj");
        fs::create_dir_all(&project).unwrap();
        let res = resolve(tmp.path(), "proj", Casing::Local, not_asked);
        let remote = Some("example.com/owner/proj".into());
        assert_eq!(
            res,
            Ok(Resolution { path: project, clone_url: None, remote, upstream_url: None })
        );
    }

    #[test]
    fn search_results_keep_git_suffix() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("example.com/bare.git");
        fs::create_dir_all(&project).unwrap();
        let res = resolve(tmp.path(), "bare.git", Casing::Local, unknown);
        let remote = Some("example.com/bare.git".into());
        assert_eq!(
            res,
            Ok(Resolution { path: project, clone_url: None, remote, upstream_url: None })
        );
    }

    #[test]
    fn reports_errors() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve(tmp.path(), "nope", Casing::Local, unknown),
            Err("nope not found".into())
        );
        assert_eq!(
            resolve(tmp.path(), "a b", Casing::Local, unknown),
            Err("Unknown pattern for a b".into())
        );
        assert_eq!(
            resolve(tmp.path(), "git@host", Casing::Local, unknown),
            Err("git@host not found".into())
        );
    }

    #[test]
    fn strips_git_extension() {
        assert_eq!(strip_git_extension("/a/b.git"), PathBuf::from("/a/b"));
        assert_eq!(strip_git_extension("/a/b"), PathBuf::from("/a/b"));
        assert_eq!(strip_git_extension("/a/.git"), PathBuf::from("/a/"));
        assert_eq!(strip_git_extension(".git"), PathBuf::from(".git"));
    }

    #[test]
    fn strips_git_extension_from_non_utf8() {
        let path = PathBuf::from(OsString::from_vec(b"/a/\xff.git".to_vec()));
        let want = PathBuf::from(OsString::from_vec(b"/a/\xff".to_vec()));
        assert_eq!(strip_git_extension(&path), want);
    }
}
