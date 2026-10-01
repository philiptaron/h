//! Helpers shared by the integration tests.

#![allow(dead_code)]

use std::borrow::BorrowMut;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::thread;

/// An API address where nothing listens, so GitHub lookups fail fast without touching the network.
pub const UNREACHABLE_API: &str = "http://127.0.0.1:1";

/// A command with an environment insulated from the user's: no proxies, no direnv, no real
/// GitHub, no code root or store from the user's shell, and none of the variables that tie git
/// to a repository, as a hook or `git rebase --exec` would have them, `git -c` settings included.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(program);
    let git_config = ["GIT_CONFIG_PARAMETERS", "GIT_CONFIG_COUNT"];
    for var in h_git::git::LOCAL_REPO_ENV.iter().chain(&git_config) {
        cmd.env_remove(var);
    }
    for var in [
        "H_CODE_ROOT",
        "H_STORE",
        "DIRENV_DIR",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
    ] {
        cmd.env_remove(var);
    }
    cmd.env("H_GITHUB_API", UNREACHABLE_API);
    cmd
}

/// The result of running a command, with output decoded as UTF-8.
#[derive(Debug)]
pub struct Run {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub fn run(mut cmd: impl BorrowMut<Command>) -> Run {
    let Output { status, stdout, stderr } =
        cmd.borrow_mut().output().expect("failed to spawn command");
    Run {
        code: status.code(),
        stdout: String::from_utf8(stdout).unwrap(),
        stderr: String::from_utf8(stderr).unwrap(),
    }
}

/// Create each of `dirs` (relative paths) under `root`.
pub fn mkdirs(root: &Path, dirs: &[&str]) {
    for dir in dirs {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
}

/// The canonical form of `path`, which is what `getcwd` reports.
pub fn canonical(path: &Path) -> String {
    fs::canonicalize(path).unwrap().to_str().unwrap().to_string()
}

/// A fake `git` that records the arguments of every invocation, writes to stdout, and exits with
/// `$FAKE_GIT_EXIT`.
///
/// With `$FAKE_GIT_MKDIR` set, it also creates its last argument (the clone target).
pub struct FakeGit {
    pub bin_dir: PathBuf,
    pub log: PathBuf,
}

impl FakeGit {
    pub fn install(dir: &Path) -> FakeGit {
        let bin_dir = dir.join("fake-bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let git = bin_dir.join("git");
        fs::write(
            &git,
            "#!/bin/sh\n\
             { for arg; do printf '%s\\n' \"$arg\"; done; echo; } >> \"$FAKE_GIT_LOG\"\n\
             echo 'fake git stdout'\n\
             for last; do :; done\n\
             [ -n \"$FAKE_GIT_MKDIR\" ] && mkdir -p \"$last\"\n\
             exit \"${FAKE_GIT_EXIT:-0}\"\n",
        )
        .unwrap();
        fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
        FakeGit { bin_dir, log: dir.join("fake-git.log") }
    }

    /// Put the fake git first on the command's `PATH`.
    pub fn apply(&self, cmd: &mut Command) {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![self.bin_dir.clone()];
        paths.extend(std::env::split_paths(&path));
        cmd.env("PATH", std::env::join_paths(paths).unwrap());
        cmd.env("FAKE_GIT_LOG", &self.log);
    }

    /// The arguments of every invocation, in order; empty if git never ran.
    pub fn invocations(&self) -> Vec<Vec<String>> {
        let Ok(log) = fs::read_to_string(&self.log) else { return Vec::new() };
        log.split("\n\n")
            .filter(|chunk| !chunk.is_empty())
            .map(|chunk| chunk.lines().map(String::from).collect())
            .collect()
    }

    /// The arguments of the last invocation, or `None` if git never ran.
    pub fn args(&self) -> Option<Vec<String>> {
        self.invocations().pop()
    }
}

/// A minimal HTTP server standing in for the GitHub API.
pub struct MockGitHub {
    pub url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl MockGitHub {
    /// Serve `routes` of `(request path, status, body)`; anything else gets a 404.
    pub fn start(routes: &[(&str, u16, &str)]) -> MockGitHub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let routes: Vec<(String, u16, String)> =
            routes.iter().map(|(p, s, b)| (p.to_string(), *s, b.to_string())).collect();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);

        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let path = head.split_whitespace().nth(1).unwrap_or_default().to_string();
                log.lock().unwrap().push(head);

                let (status, body) = routes
                    .iter()
                    .find(|(p, _, _)| *p == path)
                    .map(|(_, s, b)| (*s, b.clone()))
                    .unwrap_or((404, r#"{"message": "Not Found"}"#.to_string()));
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} Mock\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });

        MockGitHub { url, requests }
    }

    /// The request heads received so far.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

/// Initialize a git repository with one commit at `path`.
pub fn make_git_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    fs::write(path.join("README"), "hello\n").unwrap();
    for args in
        [&["init", "-q", "-b", "main"][..], &["add", "README"], &["commit", "-q", "-m", "init"]]
    {
        let status = git_command(path.parent().unwrap())
            .current_dir(path)
            .args(args)
            .status()
            .expect("git is required for these tests");
        assert!(status.success(), "git {args:?} failed");
    }
}

/// A git command isolated from the user's and system configuration.
pub fn git_command(home: &Path) -> Command {
    let mut cmd = command("git");
    isolate_git(&mut cmd, home);
    cmd
}

/// The global git configuration file used by isolated git commands under `home`.
///
/// It starts out missing, which git treats as empty; tests add URL rewrites to it.
pub fn global_gitconfig(home: &Path) -> PathBuf {
    home.join("test-gitconfig")
}

/// Keep any git run by `cmd` away from the user's and system configuration.
pub fn isolate_git(cmd: &mut Command, home: &Path) {
    cmd.env("HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", global_gitconfig(home))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com");
}

/// Make isolated git commands under `home` fetch `url` from the local repository at `local`.
pub fn rewrite_url(home: &Path, url: &str, local: &Path) {
    let status = git_command(home)
        .args(["config", "--file"])
        .arg(global_gitconfig(home))
        .arg(format!("url.file://{}.insteadOf", local.display()))
        .arg(url)
        .status()
        .unwrap();
    assert!(status.success());
}

/// Whether `program` can be found on `PATH`.
pub fn have(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// A shell command running `script` in `root`, with `$ROOT`, `$HOME` and `$PWD` set to `root`.
///
/// Setting `$PWD` keeps the shell on the logical path even where `root` is reached through a
/// symlink (as macOS temporary directories are), instead of the shell resetting it to `getcwd`.
pub fn shell(program: &str, args: &[&str], root: &Path, script: &str) -> Command {
    let mut cmd = command(program);
    cmd.args(args)
        .arg(script)
        .current_dir(root)
        .env("HOME", root)
        .env("ROOT", root)
        .env("PWD", root);
    cmd
}

/// A non-interactive bash, ignoring the user's startup files.
pub fn bash(root: &Path, script: &str) -> Command {
    shell("bash", &["--norc", "--noprofile", "-c"], root, script)
}

/// A zsh ignoring the user's startup files.
pub fn zsh(root: &Path, script: &str) -> Command {
    shell("zsh", &["-f", "-c"], root, script)
}
