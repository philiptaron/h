//! Turning the user's search term into a directory (and, if needed, a URL to clone).

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use crate::github::RepoInfo;
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
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
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
pub fn strip_git_extension(path: &Path) -> PathBuf {
    let bytes = path.as_os_str().as_bytes();
    match bytes.strip_suffix(b".git") {
        Some(stem) if !stem.is_empty() => PathBuf::from(OsStr::from_bytes(stem)),
        _ => path.to_path_buf(),
    }
}

/// Join `parts` onto `root` with `/`, without letting absolute or empty parts reset the path.
fn concat_path(root: &Path, parts: &[&str]) -> PathBuf {
    let mut bytes = root.as_os_str().to_owned().into_vec();
    for part in parts {
        bytes.push(b'/');
        bytes.extend_from_slice(part.as_bytes());
    }
    PathBuf::from(OsString::from_vec(bytes))
}

/// Resolve `term` against `code_root`.
///
/// `lookup` is consulted for GitHub repositories to fix up the casing of `user/repo`.
/// On failure, returns the message to show the user.
pub fn resolve(
    code_root: &Path,
    term: &str,
    lookup: impl FnOnce(&str, &str) -> Option<RepoInfo>,
) -> Result<Resolution, String> {
    let not_found = || format!("{term} not found");
    let target = parse_term(term).map_err(|err| match err {
        ParseError::UnknownPattern => format!("Unknown pattern for {term}"),
        ParseError::NotFound => not_found(),
    })?;

    match target {
        Target::GitHub { user, repo } => {
            let (user, repo, parent) = match lookup(&user, &repo) {
                Some(info) => (info.owner, info.name, info.parent),
                None => (user, repo, None),
            };
            Ok(Resolution {
                path: concat_path(code_root, &["github.com", &user, &repo]),
                clone_url: Some(format!("https://github.com/{user}/{repo}.git")),
                remote: Some(remote_name("github.com", &format!("{user}/{repo}"))),
                upstream_url: parent.map(|parent| format!("https://github.com/{parent}.git")),
            })
        }
        Target::Remote { url, host, path } => Ok(Resolution {
            path: strip_git_extension(&concat_path(code_root, &[&host, &path])),
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
    use std::fs;

    fn github(user: &str, repo: &str) -> Result<Target, ParseError> {
        Ok(Target::GitHub { user: user.into(), repo: repo.into() })
    }

    fn remote(url: &str, host: &str, path: &str) -> Result<Target, ParseError> {
        Ok(Target::Remote { url: url.into(), host: host.into(), path: path.into() })
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
        let url = "ssh://git@github.com/zimbatm/h";
        assert_eq!(parse_term(url), remote(url, "git@github.com", "zimbatm/h"));
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
        let root = Path::new("/code");
        assert_eq!(concat_path(root, &["host", "/abs"]), PathBuf::from("/code/host//abs"));
        assert_eq!(concat_path(root, &["", "srv/x"]), PathBuf::from("/code//srv/x"));
    }

    #[test]
    fn resolves_github_with_lookup() {
        let root = Path::new("/code");
        let res = resolve(root, "zimbatm/H", |user, repo| {
            assert_eq!((user, repo), ("zimbatm", "H"));
            Some(RepoInfo { owner: "ZimBatm".into(), name: "h".into(), parent: None })
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
        let res = resolve(Path::new("/code"), "git@github.com:a/b.git", |_, _| None);
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
    fn resolves_forks_with_their_upstream() {
        let res = resolve(Path::new("/code"), "me/nixpkgs", |_, _| {
            Some(RepoInfo {
                owner: "me".into(),
                name: "nixpkgs".into(),
                parent: Some("NixOS/nixpkgs".into()),
            })
        });
        assert_eq!(res.unwrap().upstream_url, Some("https://github.com/NixOS/nixpkgs.git".into()));
    }

    #[test]
    fn remote_names_have_no_empty_segments() {
        let res = resolve(Path::new("/code"), "file:///srv/git/proj.git", |_, _| None).unwrap();
        assert_eq!(res.remote, Some("srv/git/proj".into()));
        let res = resolve(Path::new("/code"), "https://host//a/b/", |_, _| None).unwrap();
        assert_eq!(res.remote, Some("host/a/b".into()));
    }

    #[test]
    fn remote_names_are_valid_in_refs() {
        let res = resolve(Path::new("/code"), "file:///home/me/.local/x.lock.git", |_, _| None);
        assert_eq!(res.unwrap().remote, Some("home/me/_.local/x.lock_".into()));
        let res = resolve(Path::new("/code"), "owner/.github", |_, _| None);
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
        let res = resolve(Path::new("/code"), url, |_, _| panic!("not GitHub"));
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
        let res = resolve(tmp.path(), "proj", |_, _| panic!("not GitHub"));
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
        let res = resolve(tmp.path(), "bare.git", |_, _| None);
        let remote = Some("example.com/bare.git".into());
        assert_eq!(
            res,
            Ok(Resolution { path: project, clone_url: None, remote, upstream_url: None })
        );
    }

    #[test]
    fn reports_errors() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(resolve(tmp.path(), "nope", |_, _| None), Err("nope not found".into()));
        assert_eq!(resolve(tmp.path(), "a b", |_, _| None), Err("Unknown pattern for a b".into()));
        assert_eq!(resolve(tmp.path(), "git@host", |_, _| None), Err("git@host not found".into()));
    }

    #[test]
    fn strips_git_extension() {
        assert_eq!(strip_git_extension(Path::new("/a/b.git")), PathBuf::from("/a/b"));
        assert_eq!(strip_git_extension(Path::new("/a/b")), PathBuf::from("/a/b"));
        assert_eq!(strip_git_extension(Path::new("/a/.git")), PathBuf::from("/a/"));
        assert_eq!(strip_git_extension(Path::new(".git")), PathBuf::from(".git"));
    }

    #[test]
    fn strips_git_extension_from_non_utf8() {
        let path = PathBuf::from(OsString::from_vec(b"/a/\xff.git".to_vec()));
        let want = PathBuf::from(OsString::from_vec(b"/a/\xff".to_vec()));
        assert_eq!(strip_git_extension(&path), want);
    }
}
