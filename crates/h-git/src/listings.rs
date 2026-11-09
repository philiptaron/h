//! Owners' repositories on GitHub, kept in the store between runs, for completing
//! `owner/<Tab>` with repositories that are not checked out.

use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::files::replace_file;
use crate::github::OwnerRepos;
use crate::resolve::is_simple_name;

/// How long a listing is used before GitHub is asked again.
pub const FRESH_FOR: Duration = Duration::from_secs(24 * 60 * 60);

/// The first line of a listing, which says what it holds and how.
const HEADER: &str = "h github repos 1";

/// What is known of an owner's repositories on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    Repos(OwnerRepos),
    /// GitHub has no such user or organization, as far as the token's account can see. This is
    /// kept too, so that completing the owners only checked out elsewhere (`pub/` for
    /// git.kernel.org's `pub/scm/...`) does not ask GitHub each time.
    NoSuchOwner,
}

/// A listing read back, and whether it is younger than [`FRESH_FOR`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Kept {
    pub listing: Listing,
    pub is_fresh: bool,
}

/// Read the listing at `path`. `None` when there is none, or it cannot be read.
///
/// After [`HEADER`] comes `owner <login>` and a repository name to a line, or `no such owner`.
pub fn read_listing(path: &Path) -> Option<Kept> {
    let text = fs::read_to_string(path).ok()?;
    let mut lines = text.lines();
    if lines.next()? != HEADER {
        return None;
    }
    let listing = match lines.next()? {
        "no such owner" => Listing::NoSuchOwner,
        line => {
            let login = line.strip_prefix("owner ")?.to_string();
            Listing::Repos(OwnerRepos { login, names: lines.map(String::from).collect() })
        }
    };
    let age = fs::metadata(path).ok()?.modified().ok()?.elapsed().unwrap_or_default();
    Some(Kept { listing, is_fresh: age < FRESH_FOR })
}

/// Write `listing` to `path`, replacing what was there.
pub fn write_listing(path: &Path, listing: &Listing) -> std::io::Result<()> {
    let mut text = format!("{HEADER}\n");
    match listing {
        Listing::NoSuchOwner => text.push_str("no such owner\n"),
        Listing::Repos(repos) => {
            text.push_str(&format!("owner {}\n", repos.login));
            for name in &repos.names {
                text.push_str(&format!("{name}\n"));
            }
        }
    }
    replace_file(path, text.as_bytes())
}

/// A word to complete from an owner's repositories on GitHub: `owner/start`, or
/// `github.com/owner/start`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitHubWord<'a> {
    /// What comes before the owner, as typed: empty, or `github.com/`.
    pub lead: &'a str,
    pub owner: &'a str,
    /// The start of a repository's name.
    pub start: &'a str,
}

impl GitHubWord<'_> {
    /// Read `word` as one to complete from GitHub, when it can be one: the owner must be
    /// spelled as a GitHub login can be, which rules out the likes of `example.com/`.
    pub fn parse(word: &str) -> Option<GitHubWord<'_>> {
        let (lead, rest) = match word.split_once('/') {
            Some((host, rest)) if host.eq_ignore_ascii_case("github.com") => {
                (&word[..host.len() + 1], rest)
            }
            _ => ("", word),
        };
        let (owner, start) = rest.split_once('/')?;
        let is_login = !owner.is_empty()
            && owner.len() <= 39
            && !owner.starts_with('-')
            && owner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        let is_start = start.is_empty() || is_simple_name(start);
        (is_login && is_start).then_some(GitHubWord { lead, owner, start })
    }

    /// The terms that complete this word from `repos`, best first: those whose case differs from
    /// the word in the fewest letters, then the rest in sorted order. The owner is spelled as
    /// GitHub spells it.
    pub fn list_completions(&self, repos: &OwnerRepos) -> Vec<String> {
        let word = format!("{}{}/{}", self.lead, self.owner, self.start);
        let mut found: Vec<(usize, String)> = repos
            .names
            .iter()
            .filter(|name| {
                let start = name.as_bytes().get(..self.start.len());
                start.is_some_and(|s| s.eq_ignore_ascii_case(self.start.as_bytes()))
            })
            .map(|name| {
                let term = format!("{}{}/{name}", self.lead, repos.login);
                let differences = term.bytes().zip(word.bytes()).filter(|(a, b)| a != b).count();
                (differences, term)
            })
            .collect();
        found.sort();
        found.dedup();
        found.into_iter().map(|(_, term)| term).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn nixos() -> OwnerRepos {
        let names = ["nixpkgs", "nix", "Nixpkgs-Old", "hydra"].map(String::from).to_vec();
        OwnerRepos { login: "NixOS".into(), names }
    }

    fn word(word: &str) -> GitHubWord<'_> {
        GitHubWord::parse(word).unwrap()
    }

    #[test]
    fn parses_words_naming_an_owner() {
        let w = |lead, owner, start| Some(GitHubWord { lead, owner, start });
        assert_eq!(GitHubWord::parse("nixos/nixp"), w("", "nixos", "nixp"));
        assert_eq!(GitHubWord::parse("nixos/"), w("", "nixos", ""));
        assert_eq!(GitHubWord::parse("GitHub.com/nixos/n"), w("GitHub.com/", "nixos", "n"));
        assert_eq!(GitHubWord::parse("a-b/c.d_e"), w("", "a-b", "c.d_e"));
        for bad in
            ["nixos", "", "/x", "-a/b", "a_b/c", "example.com/x", "a/b/c", "a/b c", "github.com/"]
        {
            assert_eq!(GitHubWord::parse(bad), None, "{bad:?}");
        }
        assert_eq!(GitHubWord::parse(&format!("{}/x", "a".repeat(40))), None);
    }

    #[test]
    fn completes_in_any_case_with_githubs_spelling() {
        assert_eq!(
            word("nixos/nixp").list_completions(&nixos()),
            ["NixOS/nixpkgs", "NixOS/Nixpkgs-Old"]
        );
        assert_eq!(
            word("NixOS/NIX").list_completions(&nixos()),
            ["NixOS/Nixpkgs-Old", "NixOS/nix", "NixOS/nixpkgs"]
        );
        assert_eq!(
            word("github.com/nixos/h").list_completions(&nixos()),
            ["github.com/NixOS/hydra"]
        );
        assert_eq!(word("nixos/").list_completions(&nixos()).len(), 4);
        assert!(word("nixos/zzz").list_completions(&nixos()).is_empty());
    }

    #[test]
    fn listings_are_kept_and_age() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("h/github/nixos");
        assert_eq!(read_listing(&path), None);
        write_listing(&path, &Listing::Repos(nixos())).unwrap();
        let kept = Kept { listing: Listing::Repos(nixos()), is_fresh: true };
        assert_eq!(read_listing(&path), Some(kept));

        write_listing(&path, &Listing::NoSuchOwner).unwrap();
        let old = SystemTime::now() - FRESH_FOR - Duration::from_secs(60);
        fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();
        let kept = Kept { listing: Listing::NoSuchOwner, is_fresh: false };
        assert_eq!(read_listing(&path), Some(kept));

        for junk in
            ["", "h github repos 2\nowner x\n", "h github repos 1\n", "h github repos 1\nx\n"]
        {
            fs::write(&path, junk).unwrap();
            assert_eq!(read_listing(&path), None, "{junk:?}");
        }
    }
}
