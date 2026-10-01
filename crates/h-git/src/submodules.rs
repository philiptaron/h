//! Submodules of a clone that borrows objects from a store, or that is a fork.
//!
//! `git clone --recurse-submodules --reference-if-able <store>` passes the store on to submodules
//! only as `submodule.alternateLocation=superproject`, which looks for each submodule at
//! `<store>/modules/<name>`, the layout of a checkout's `.git`. A store never has it, so every
//! submodule says it "cannot add alternate", is downloaded in full, and the setting stays in the
//! clone. So such a clone is made without submodules, and the step `git clone` would have run
//! after checkout, `git submodule update --init --recursive`, is run with `--reference <store>`.
//! A fork's clone is made the same way, so that its upstream is added before its submodules.
//!
//! Either way the identity the clone is made with (`-c credential.username=…`, `user.*`) goes to
//! the submodules' clones too, which `git clone -c` alone does not do.

use std::ffi::{OsStr, OsString};
use std::io::IsTerminal;
use std::path::Path;

use crate::git::{self, GitError};
use crate::resolve::store_upstream;
use crate::store::Store;

/// Long clone options that take their value as the next argument when it is not given with `=`.
const VALUE_OPTIONS: &[&str] = &[
    "--template",
    "--reference",
    "--reference-if-able",
    "--origin",
    "--branch",
    "--revision",
    "--upload-pack",
    "--depth",
    "--shallow-since",
    "--shallow-exclude",
    "--separate-git-dir",
    "--ref-format",
    "--config",
    "--server-option",
    "--filter",
    "--bundle-uri",
    "--jobs",
];

/// Short clone options that take a value, attached (`-bmain`) or as the next argument.
const VALUE_FLAGS: &[u8] = b"obucj";

/// What the options of a clone say about its submodules, read as `git clone` reads them: in order,
/// the last of an option and its negation winning.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SubmoduleOptions {
    /// The pathspecs given to `--recurse-submodules` (`.` when it has none), or `None` when no
    /// option decides. `--no-recurse-submodules` leaves none.
    pub recurse: Option<Vec<String>>,
    /// `--bare` or `--mirror`, which h takes to mean no submodules unless asked for them.
    pub bare: bool,
    /// `-n` or `--no-checkout`: nothing is checked out, so no submodule is cloned either.
    pub no_checkout: bool,
    /// The clone was given its own `--reference` or `--reference-if-able`.
    pub references: bool,
    pub dissociate: bool,
    /// `--shallow-submodules`.
    pub shallow: bool,
    /// `-j` or `--jobs`.
    pub jobs: Option<String>,
    /// `--remote-submodules`.
    pub remote: bool,
    pub filter: Option<String>,
    /// `--also-filter-submodules` or its negation, which override `clone.filterSubmodules`.
    pub also_filter: Option<bool>,
    pub single_branch: Option<bool>,
    /// `--depth`, `--shallow-since` or `--shallow-exclude`, which imply `--single-branch`.
    pub deepen: bool,
    pub quiet: bool,
    pub progress: Option<bool>,
    pub ref_format: Option<String>,
}

impl SubmoduleOptions {
    /// Read the clone options `opts`, given to `git clone` in this order.
    pub fn parse<'a>(opts: impl IntoIterator<Item = &'a OsString>) -> SubmoduleOptions {
        let mut parsed = SubmoduleOptions::default();
        let mut iter = opts.into_iter().map(|opt| opt.to_string_lossy());
        while let Some(opt) = iter.next() {
            let (name, value) = if opt.starts_with("--") {
                match opt.split_once('=') {
                    Some((name, value)) => (name.to_string(), Some(value.to_string())),
                    None if VALUE_OPTIONS.contains(&&*opt) => {
                        (opt.to_string(), iter.next().map(|v| v.into_owned()))
                    }
                    None => (opt.to_string(), None),
                }
            } else if let [b'-', flag, rest @ ..] = opt.as_bytes()
                && VALUE_FLAGS.contains(flag)
            {
                let value = match rest {
                    [] => iter.next().map(|v| v.into_owned()),
                    rest => Some(String::from_utf8_lossy(rest).into_owned()),
                };
                (format!("-{}", *flag as char), value)
            } else {
                (opt.to_string(), None)
            };
            parsed.apply(&name, value);
        }
        parsed
    }

    fn apply(&mut self, name: &str, value: Option<String>) {
        match name {
            "--recurse-submodules" | "--recursive" => {
                self.recurse.get_or_insert_with(Vec::new).push(value.unwrap_or_else(|| ".".into()))
            }
            "--no-recurse-submodules" | "--no-recursive" => self.recurse = Some(Vec::new()),
            "--bare" | "--mirror" => self.bare = true,
            "-n" | "--no-checkout" => self.no_checkout = true,
            "--reference" | "--reference-if-able" => self.references = true,
            "--dissociate" => self.dissociate = true,
            "--no-dissociate" => self.dissociate = false,
            "--shallow-submodules" => self.shallow = true,
            "--no-shallow-submodules" => self.shallow = false,
            "-j" | "--jobs" => self.jobs = value,
            "--remote-submodules" => self.remote = true,
            "--no-remote-submodules" => self.remote = false,
            "--filter" => self.filter = value,
            "--no-filter" => self.filter = None,
            "--also-filter-submodules" => self.also_filter = Some(true),
            "--no-also-filter-submodules" => self.also_filter = Some(false),
            "--single-branch" => self.single_branch = Some(true),
            "--no-single-branch" => self.single_branch = Some(false),
            "--depth" | "--shallow-since" | "--shallow-exclude" => self.deepen = true,
            "-q" | "--quiet" => self.quiet = true,
            "-v" | "--verbose" | "--no-quiet" | "--no-verbose" => self.quiet = false,
            "--progress" => self.progress = Some(true),
            "--no-progress" => self.progress = Some(false),
            "--ref-format" => self.ref_format = value,
            _ => {}
        }
    }

    /// The pathspecs of the submodules to clone: those of `--recurse-submodules`, or all of them
    /// when no option decides, unless the clone is bare. Sorted and without duplicates, as `git
    /// clone` writes them to `submodule.active`.
    pub fn pathspecs(&self) -> Vec<String> {
        let mut pathspecs = match &self.recurse {
            Some(pathspecs) => pathspecs.clone(),
            None if self.bare => Vec::new(),
            None => vec![".".into()],
        };
        pathspecs.sort();
        pathspecs.dedup();
        pathspecs
    }

    /// Whether the clone checks out a working tree, after which `git clone` updates submodules.
    pub fn checks_out(&self) -> bool {
        !self.bare && !self.no_checkout
    }

    /// Whether `opt` is one that `git clone --no-recurse-submodules` rejects, because it only
    /// means something for submodules, which are then cloned by [`update_args`] instead.
    pub fn is_submodule_only(opt: &OsStr) -> bool {
        opt == "--also-filter-submodules" || opt == "--no-also-filter-submodules"
    }
}

/// `-c key=value` arguments for git itself, one pair for each of `config`.
pub fn config_args(config: &[(OsString, OsString)]) -> Vec<OsString> {
    let mut args = Vec::new();
    for (key, value) in config {
        let mut setting = key.clone();
        setting.push("=");
        setting.push(value);
        args.extend([OsString::from("-c"), setting]);
    }
    args
}

/// Clone the submodules of the new clone at `path`, borrowing from `store` when there is one, as
/// `git clone --recurse-submodules` would have after checking out, with `identity` (the clone's
/// `-c` pairs) in effect for every submodule's clone, nested ones included.
pub fn update(
    path: &Path,
    store: Option<&Path>,
    opts: &SubmoduleOptions,
    identity: &[(OsString, OsString)],
) -> Result<(), GitError> {
    // Settings `git clone` consults when it clones submodules.
    let pattern = r"^(submodule\.stickyrecursiveclone|clone\.filtersubmodules)$";
    let settings = git::output(Some(path), &["config", "--type=bool", "--get-regexp", pattern])
        .unwrap_or_default();
    let enabled = |key: &str| settings.lines().any(|line| line == format!("{key} true"));
    if enabled("submodule.stickyrecursiveclone") {
        git::run(Some(path), &["config", "submodule.recurse", "true"])?;
    }
    let filter = opts.also_filter.unwrap_or_else(|| enabled("clone.filtersubmodules"));
    let progress = opts.progress.unwrap_or_else(|| !opts.quiet && std::io::stderr().is_terminal());
    // The `-c` settings reach the submodules' clones, nested ones included, through the
    // environment.
    let mut args = config_args(identity);
    if store.is_some() {
        args.extend(negotiation_args(path));
    }
    args.extend(update_args(opts, store, filter, progress));
    git::run(Some(path), &args)
}

/// The `-c` that has submodules borrowing from a store negotiate with the skipping algorithm,
/// unless the user chose one for the repository at `path` or everywhere.
///
/// Each submodule's clone offers the server every commit in the store as one it has, newest
/// first, until the server recognizes one: all of them for a submodule the store lacks, and for
/// one whose history is older than the rest of the store. The skipping negotiator gives up on
/// unrelated history quickly, and still finds the submodule's own commits in the store.
pub(crate) fn negotiation_args(path: &Path) -> Vec<OsString> {
    if git::output(Some(path), &["config", "--get", "fetch.negotiationAlgorithm"]).is_ok() {
        return Vec::new();
    }
    ["-c", "fetch.negotiationAlgorithm=skipping"].map(OsString::from).into()
}

/// Write the `-c` pairs `identity` into the configuration of every submodule checked out in
/// the checkout at `path`, nested ones included, as `git clone -c` writes them into the
/// superproject, so that later fetches and commits there use them too.
pub fn write_identity(path: &Path, identity: &[(OsString, OsString)]) -> Result<(), GitError> {
    if identity.is_empty() {
        return Ok(());
    }
    let dirs = git::output(Some(path), &["submodule", "foreach", "--quiet", "--recursive", "pwd"])?;
    for dir in dirs.lines().filter(|dir| !dir.is_empty()) {
        for (key, value) in identity {
            git::run(
                Some(Path::new(dir)),
                &[OsString::from("config"), key.clone(), value.clone()],
            )?;
        }
    }
    Ok(())
}

/// The submodules `.gitmodules` in the checkout at `path` names, as `(name, url)`.
pub fn gitmodules(path: &Path) -> Vec<(String, String)> {
    let file = path.join(".gitmodules");
    if !file.is_file() {
        return Vec::new();
    }
    let args = [OsString::from("config"), "--file".into(), file.into(), "-z".into()];
    let args = [&args[..], &["--get-regexp".into(), r"^submodule\..*\.url$".into()]].concat();
    let out = git::output(Some(path), &args).unwrap_or_default();
    // NUL-terminated `<key>\n<value>` entries, since submodule names may contain spaces.
    out.split('\0')
        .filter_map(|entry| {
            let (key, url) = entry.split_once('\n')?;
            let name = key.strip_prefix("submodule.")?.strip_suffix(".url")?;
            Some((name.to_string(), url.to_string()))
        })
        .collect()
}

/// Whether `url`, from `.gitmodules`, is relative to the superproject's own remote, as
/// `../lib.git` is: git resolves those only when they start with `./` or `../`.
pub fn is_relative(url: &str) -> bool {
    url.starts_with("./") || url.starts_with("../")
}

/// Whether git takes `url` for a local path rather than a URL or `host:path`.
fn is_local_not_ssh(url: &str) -> bool {
    match (url.find(':'), url.find('/')) {
        (None, _) => true,
        (Some(colon), Some(slash)) => slash < colon,
        (Some(_), None) => false,
    }
}

/// Resolve the relative submodule URL `url` against `remote_url`, the URL of the superproject's
/// remote, as git does (`relative_url` in git's `remote.c`): each `../` takes off the last part
/// of the remote's URL, so `../lib.git` beside `https://github.com/up/app.git` is
/// `https://github.com/up/lib.git`, and beside `git@host:up/app.git` is `git@host:up/lib.git`.
/// `None` where git gives up, when there is nothing left to take off.
pub fn relative_url(remote_url: &str, url: &str) -> Option<String> {
    if !is_local_not_ssh(url) || url.starts_with('/') {
        return Some(url.to_string());
    }
    let mut remote = remote_url.strip_suffix('/').unwrap_or(remote_url).to_string();
    let relative = is_local_not_ssh(&remote) && !remote.starts_with('/');
    if relative && !remote.starts_with("./") && !remote.starts_with("../") {
        remote.insert_str(0, "./");
    }
    let mut colon = false;
    let mut rest = url;
    loop {
        if let Some(after) = rest.strip_prefix("../") {
            rest = after;
            if let Some(i) = remote.rfind('/') {
                remote.truncate(i);
            } else if let Some(i) = remote.rfind(':') {
                remote.truncate(i);
                colon = true;
            } else if relative || remote == "." {
                return None;
            } else {
                remote = ".".into();
            }
        } else if let Some(after) = rest.strip_prefix("./") {
            rest = after;
        } else {
            break;
        }
    }
    let mut out = format!("{remote}{}{rest}", if colon { ":" } else { "/" });
    if rest.ends_with('/') {
        out.pop();
    }
    Some(out.strip_prefix("./").map(String::from).unwrap_or(out))
}

/// Whether a repository answers at `url`, asked once with `git ls-remote` and `identity` in
/// effect, so that the right credentials are offered. Nothing ever prompts, as with h's GitHub
/// lookups: a repository that wants credentials no helper has counts as missing.
fn answers(url: &str, identity: &[(OsString, OsString)]) -> bool {
    let mut args = config_args(identity);
    let rest = ["-c", "credential.interactive=false", "ls-remote", "--quiet", url, "HEAD"];
    args.extend(rest.map(OsString::from));
    git::command(None, &args)
        .env("GIT_ASKPASS", "")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// In a fork's checkout at `path`, point each submodule that `.gitmodules` names by a relative
/// URL at the fork's copy of it when there is one, and otherwise at the upstream's.
///
/// Git resolves `../lib.git` against the fork's own remote, at `own_url`, so a fork whose
/// owner did not fork `lib` too has a submodule that cannot be cloned. Here the fork's copy wins
/// when `store` already has it, without the network, or when it answers at its URL; otherwise
/// `lib` comes from beside the upstream, at `upstream_url`. The choice is written as
/// `submodule.<name>.url` in the superproject's configuration, as `git submodule init` writes
/// it, before the submodules are cloned. A submodule already cloned, or whose URL was set to
/// something else, is left alone. Nested submodules resolve against their own superproject,
/// which is then the copy chosen here. Returns the submodules that come from the upstream.
pub fn resolve_fork_urls(
    path: &Path,
    own_url: &str,
    upstream_url: &str,
    store: Option<&Store>,
    identity: &[(OsString, OsString)],
) -> Result<Vec<(String, String)>, GitError> {
    let git_dir = git::output(Some(path), &["rev-parse", "--absolute-git-dir"])?;
    let modules = Path::new(git_dir.trim()).join("modules");
    let mut from_upstream = Vec::new();
    for (name, url) in gitmodules(path).into_iter().filter(|(_, url)| is_relative(url)) {
        let (Some(fork), Some(upstream)) =
            (relative_url(own_url, &url), relative_url(upstream_url, &url))
        else {
            continue;
        };
        let key = format!("submodule.{name}.url");
        let set = git::output(Some(path), &["config", "--get", &key]).ok();
        let untouched = set.as_deref().is_none_or(|set| set.trim() == fork);
        if fork == upstream || modules.join(&name).exists() || !untouched {
            continue;
        }
        let in_store = || {
            let found = store_upstream(&fork).and_then(|(name, _)| store?.find(&name));
            found.is_some()
        };
        if in_store() || answers(&fork, identity) {
            git::run(Some(path), &["config", &key, &fork])?;
        } else {
            git::run(Some(path), &["config", &key, &upstream])?;
            from_upstream.push((name, upstream));
        }
    }
    Ok(from_upstream)
}

/// The `git submodule update` arguments that clone submodules after checkout as `git clone`
/// does, but borrowing objects from `store` when there is one.
///
/// `filter_submodules` is whether the clone's filter applies to submodules too
/// (`--also-filter-submodules`, or `clone.filterSubmodules`), and `progress` whether to show
/// progress, which `git clone` decides from its own verbosity and terminal.
pub fn update_args(
    opts: &SubmoduleOptions,
    store: Option<&Path>,
    filter_submodules: bool,
    progress: bool,
) -> Vec<OsString> {
    let mut args: Vec<OsString> =
        ["submodule", "update", "--require-init", "--recursive"].map(OsString::from).into();
    if let Some(store) = store {
        args.push("--reference".into());
        args.push(store.into());
    }
    if opts.dissociate {
        args.push("--dissociate".into());
    }
    if opts.shallow {
        args.push("--depth=1".into());
    }
    if let Some(jobs) = &opts.jobs {
        args.push(format!("--jobs={jobs}").into());
    }
    if progress {
        args.push("--progress".into());
    }
    if opts.quiet {
        args.push("--quiet".into());
    }
    if opts.remote {
        args.push("--remote".into());
        args.push("--no-fetch".into());
    }
    if let Some(format) = &opts.ref_format {
        args.push(format!("--ref-format={format}").into());
    }
    if let Some(filter) = opts.filter.as_ref().filter(|_| filter_submodules) {
        args.push(format!("--filter={filter}").into());
    }
    let single_branch = opts.single_branch.unwrap_or(opts.deepen);
    args.push(if single_branch { "--single-branch" } else { "--no-single-branch" }.into());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(list: &[&str]) -> SubmoduleOptions {
        let opts: Vec<OsString> = list.iter().map(OsString::from).collect();
        SubmoduleOptions::parse(&opts)
    }

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter().map(|a| a.into_string().unwrap()).collect()
    }

    #[test]
    fn recurses_into_everything_unless_an_option_decides() {
        assert_eq!(parse(&[]).pathspecs(), ["."]);
        assert_eq!(parse(&["--depth", "1"]).pathspecs(), ["."]);
        for opts in [&["--bare"][..], &["--mirror"], &["--no-recurse-submodules"]] {
            assert!(parse(opts).pathspecs().is_empty(), "{opts:?}");
        }
        assert_eq!(parse(&["--recursive"]).recurse, Some(vec![".".into()]));
        // Pathspecs collect, sorted and once each; a negation clears them.
        let opts = parse(&["--recurse-submodules=b", "--recursive=a", "--recurse-submodules=b"]);
        assert_eq!(opts.pathspecs(), ["a", "b"]);
        let opts = parse(&["--recurse-submodules=a", "--no-recursive", "--recurse-submodules"]);
        assert_eq!(opts.pathspecs(), ["."]);
        // An explicit request wins even in a bare clone, which then just has nothing to update.
        let opts = parse(&["--bare", "--recursive"]);
        assert_eq!((opts.pathspecs(), opts.checks_out()), (vec![".".to_string()], false));
    }

    #[test]
    fn values_of_other_options_are_not_mistaken_for_options() {
        let opts = parse(&["--branch", "--recursive", "-o", "--bare", "-c", "--no-checkout"]);
        assert_eq!(opts, SubmoduleOptions::default());
        let opts = parse(&["--jobs", "3", "-j5", "--filter", "blob:none", "-q"]);
        assert_eq!(opts.jobs.as_deref(), Some("5"));
        assert_eq!(opts.filter.as_deref(), Some("blob:none"));
        assert!(opts.quiet);
        // `--recurse-submodules` takes its pathspec only after `=`.
        let opts = parse(&["--recurse-submodules", "lib"]);
        assert_eq!(opts.pathspecs(), ["."]);
    }

    #[test]
    fn reads_what_the_submodule_step_needs() {
        let opts = parse(&[
            "--reference=/other",
            "--dissociate",
            "--shallow-submodules",
            "--remote-submodules",
            "--filter=tree:0",
            "--also-filter-submodules",
            "--shallow-since=2020-01-01",
            "-v",
            "--progress",
            "--ref-format=reftable",
            "-n",
        ]);
        assert!(opts.references && opts.dissociate && opts.shallow && opts.remote);
        assert!(opts.deepen && !opts.quiet && !opts.checks_out());
        assert_eq!(opts.also_filter, Some(true));
        assert_eq!(opts.progress, Some(true));
        assert_eq!(opts.ref_format.as_deref(), Some("reftable"));
        let opts = parse(&["-q", "--verbose", "--no-filter", "--no-shallow-submodules"]);
        assert!(!opts.quiet && !opts.shallow && opts.filter.is_none());
    }

    #[test]
    fn plain_update_borrows_from_the_store() {
        let args = update_args(&parse(&[]), Some(Path::new("/store")), false, false);
        assert_eq!(
            strings(args),
            [
                "submodule",
                "update",
                "--require-init",
                "--recursive",
                "--reference",
                "/store",
                "--no-single-branch"
            ]
        );
    }

    #[test]
    fn resolves_relative_urls_as_git_does() {
        for (remote, url, want) in [
            ("https://github.com/up/app.git", "../lib.git", Some("https://github.com/up/lib.git")),
            ("https://github.com/up/app.git/", "../lib.git", Some("https://github.com/up/lib.git")),
            ("https://h/a/b.git", "../../c.git", Some("https://h/c.git")),
            ("https://h/a/b.git", "./c", Some("https://h/a/b.git/c")),
            ("https://h/a/b.git", "../c/", Some("https://h/a/c")),
            ("git@github.com:up/app.git", "../lib.git", Some("git@github.com:up/lib.git")),
            ("git@host:app.git", "../lib.git", Some("git@host:lib.git")),
            ("/srv/app.git", "../lib.git", Some("/srv/lib.git")),
            ("srv/app", "../lib", Some("srv/lib")),
            ("app", "../../lib", None),
            ("x", "https://other/lib.git", Some("https://other/lib.git")),
        ] {
            assert_eq!(relative_url(remote, url).as_deref(), want, "{remote} + {url}");
        }
        assert!(is_relative("../lib.git") && is_relative("./lib"));
        assert!(!is_relative("lib") && !is_relative("https://h/lib") && !is_relative("/lib"));
    }

    #[test]
    fn without_a_store_update_borrows_nothing() {
        let args = strings(update_args(&parse(&[]), None, false, false));
        assert_eq!(
            args,
            ["submodule", "update", "--require-init", "--recursive", "--no-single-branch"]
        );
    }

    #[test]
    fn config_becomes_git_options() {
        let config = [("user.name", "Me Too"), ("credential.username", "me")]
            .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        assert_eq!(
            strings(config_args(&config)),
            ["-c", "user.name=Me Too", "-c", "credential.username=me"]
        );
    }

    #[test]
    fn update_follows_the_clone_options() {
        let opts = parse(&[
            "--dissociate",
            "--shallow-submodules",
            "-j",
            "4",
            "-q",
            "--remote-submodules",
            "--ref-format=reftable",
            "--filter=blob:none",
            "--depth=1",
        ]);
        assert_eq!(
            strings(update_args(&opts, Some(Path::new("/store")), true, true)),
            [
                "submodule",
                "update",
                "--require-init",
                "--recursive",
                "--reference",
                "/store",
                "--dissociate",
                "--depth=1",
                "--jobs=4",
                "--progress",
                "--quiet",
                "--remote",
                "--no-fetch",
                "--ref-format=reftable",
                "--filter=blob:none",
                "--single-branch"
            ]
        );
        // The filter reaches submodules only when asked to; --depth implies --single-branch
        // unless told otherwise.
        let opts = parse(&["--filter=blob:none", "--depth=1", "--no-single-branch"]);
        let args = strings(update_args(&opts, Some(Path::new("/store")), false, false));
        assert!(!args.iter().any(|a| a.starts_with("--filter")), "{args:?}");
        assert_eq!(args.last().unwrap(), "--no-single-branch");
    }
}
