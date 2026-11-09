# Changelog

## 0.4.0 (2026-11-09)

Tab completion that works: for paths of any depth, in any case, and for repositories on GitHub
that are not cloned yet. Terms follow one rule wherever the checkout lives, and the store keeps
an index of the projects so neither completion nor `h <name>` walks the code root each time.

### Completion

- **Completion works again, for paths of any depth, in any case.** It only offered the last names
  of directories three levels down, matched in the case typed, so `nixos/m<Tab>`, and anything
  else with a `/`, completed to nothing. The shell functions now ask the new `h complete`, which
  completes the end of any project's path with as many names as were typed (`nixos/m<Tab>`,
  `torvalds/li<Tab>`, `scm/torvalds/<Tab>`).
  - Case is ignored, but each completion is spelled as the directory is, and those closest to the
    case typed come first: `nixos/m` offers `nixos/mobile` before `NixOS/marketing`.
  - A directory holding repositories, such as an owner, completes with a `/` and no space, ready
    for the next name. One that shares its name with its repository (`alacritty`) is offered both
    ways.
  - A name several repositories share, typed in full, completes to a longer term for each:
    `proj<Tab>` offers `x/proj` and `y/proj`, the one `h proj` goes to first. zsh steps through
    them with each Tab; bash lists them, and steps through them where Tab is bound to readline's
    `menu-complete` (with `menu-complete-display-prefix` on, the first Tab still lists).
- **`owner/<Tab>` offers the owner's repositories on GitHub.** With a store, completing
  `owner/start` or `github.com/owner/start` also offers what the owner has on GitHub that is not
  checked out, spelled as GitHub spells it, after the checkouts; choosing one clones it. zsh lists
  the two under "checked out" and "on GitHub".
  - The list is asked for with the identity's token, through GitHub's GraphQL API, so an
    organization's private and internal repositories are in it, and the identity's own private
    ones. Without a token, the REST API lists public repositories.
  - It is kept in the store (`<store>/h/github/<owner>`) for a day, and fetched again in the
    background once older. An owner GitHub does not know is kept as such, so the likes of `pub/`
    do not ask GitHub on every Tab.
  - The first completion of an owner waits up to ten seconds, with a spinner after the cursor
    when the cursor is at the end of the line. Ctrl-C stops waiting and leaves the line as it
    was; the list still arrives for the next Tab.
  - Completion lists as many of an owner's repositories as eight seconds allow, the most recently
    pushed to first. `h store maintain daily` lists every kept owner in full.
- **bash keeps `h complete`'s order** where it can (bash 4.4 and later), rather than sorting it.

### Terms

- **One rule finds every term, in any case.** A term is a project's name or the end of its path,
  with as many names as you like, and it goes to the checkout whose path ends that way wherever
  it lives: `torvalds/linux` finds `git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux`, and
  `gitlab.com/group/sub/project` works where it used to be an unknown pattern. Checkouts at any
  depth are found, where the search used to stop three levels down.
  - Every term matches in any case. One with a capital letter prefers projects in its own case;
    then the deepest wins, then one in the term's case, then the first in sorted order. Before,
    a term with a capital letter had to match exactly, so `MYPROJ` found nothing at all.
  - `user/repo` is a term like any other until no checkout matches; only then is GitHub asked
    and the repository cloned. So `h torvalds/linux` goes to git.kernel.org's checkout, and a
    GitHub `user/repo` checked out elsewhere is found without asking GitHub.
  - **A trailing `/` names the directory holding repositories**, so `h alacritty/` goes to the
    owner, where `h alacritty` goes to github.com/alacritty/alacritty.
  - **`github.com/user/repo` means GitHub's `user/repo`**, cloned when it is not checked out, as
    `user/repo` is.
- **Only repositories and the directories holding them are projects.** The search does not look
  inside a repository, nor list directories with none in them, so a virtualenv no longer puts
  `pip` and `__pycache__` among the completions, and `h pip` does not jump into it.

### The object store

- **An index of the projects.** With a store, `h` keeps what it finds under the code root in
  `<store>/h/projects`, which git ignores, so completion and `h <name>` need not walk the code
  root each time. A clone updates it, and a checkout made or removed some other way is noticed
  the first time the index cannot answer, when the code root is walked again.
- **`h store maintain` keeps h's own files fresh.** On every schedule it rewrites the index, for
  the code root it was written for; daily, it lists every kept owner's repositories on GitHub
  again, in full, however long that takes. Run from a timer without git options, it finds the
  token with the store's own `credential.*` settings. An owner that cannot be listed is named and
  keeps its old list; the rest are still done.
- **The GitHub token is asked for once a run.** `h store add` with several private repositories
  asked the credential helpers once for each.

### Under the hood

- **Tests** no longer reach the network: git in the tests may only use `file://` and the mock
  servers' plain `http://`, so a test that fell through to cloning the real torvalds/linux now
  fails at once instead of hanging. The tests' fake git is written once, which fixes a rare
  "Text file busy" failure.
- **Tests:** 298 unit and end-to-end tests.

## 0.3.6 (2026-11-03)

Shorter, clearer failures, and nothing left behind when a clone fails.

### Fixes

- **Repositories GitHub lacks even with the token are not cloned.** When GitHub answers 404 to
  a lookup made with the identity's token, the repository does not exist or that identity cannot
  see it. `h go` used to carry on and fetch it into the store and clone it, both of which failed
  at length. Now it says `<user>/<repo> not found on GitHub, even with this identity's token` and
  stops, unless there is a checkout of it already.
- **A failed clone leaves no empty directories.** git takes away the directory it was cloning
  into when it fails, but not the `<host>/<owner>` directories `h` made above it, so a mistyped
  owner left them behind. They are removed again too, as far as nothing else was put in them.
- **A failed git says what `h` could not do, not the whole command.** A failure was reported as
  git's entire command line, identity and options included, which buried what `h` was doing.
  Git has already said why by then, so `h` now says, for example, `could not clone <url>: git
  clone exited with status 128`. Set `GIT_TRACE=1` to see every git command in full.
- **A failed fetch into the store is said once.** When an upstream could not be fetched, `h`
  said it took the upstream out of the store again and then also warned that fetching into the
  store failed. The second warning is left out when the first already says it.
- **Failures to write a file name the file.** Writing `objects/info/alternates` or a container's
  `.git` file was reported as `failed to run git: Permission denied`, though git never ran. Now
  it is `could not write <path>: Permission denied`.

### Under the hood

- **Names:** functions are named as commands for what they do, as a new naming skill for the
  repository describes, and a repository that gets its objects from the store is a tenant.
- **Tests:** 258 unit and end-to-end tests.

## 0.3.5 (2026-10-26)

A git identity for clones made by running the `h` binary directly.

### Fixes

- **`$H_GIT_OPTS` gives the binary an identity.** The shell function from `h-shell-init` passes
  its `--git-opts` after `--`, but scripts, hooks and agents run the binary, which had no other
  way to get them. Their clones came out without `user.name`, `user.email`, `user.signingkey` or
  `credential.username`, and anything committed there went out unsigned or under the wrong name.
  Now, when nothing follows `--`, `h` reads the git options from `$H_GIT_OPTS`, alongside
  `$H_CODE_ROOT` and `$H_STORE`.
  - It takes the same string as `--git-opts`, quotes included, and splits it as the shell
    function would.
  - Options after `--` still win, and an empty `--` leaves the identity out.

### Under the hood

- **Tests:** 252 unit and end-to-end tests.

## 0.3.4 (2026-10-23)

A fix for Claude Code worktrees whose directory comes back after they are removed.

This is what 0.3.3 was. Its release was withdrawn when its commits were rewritten, and GitHub
does not let a release under that tag be made again. The code is the same.

### Fixes

- **A worktree whose directory was written into again after its removal can be made again.** A
  build or dev server still running in a removed worktree can recreate part of its directory, as
  `astro dev` regenerating `site/.astro/icon.d.ts` did. The WorktreeCreate hook then refused that
  name, since the directory was not a worktree. Now it moves such a directory aside to the first
  free `<name>~1`, `<name>~2`, … and says where on stderr. Nothing already there is overwritten.
  The worktree is then made as usual, from its branch if the removed one left it.
  - An empty directory is just removed.
  - A directory with a `.git` of its own is still refused, since it belongs to another repository.

### Under the hood

- **Tests:** 249 unit and end-to-end tests.

## 0.3.2 (2026-10-22)

A fix for adding an upstream to a store that already holds many others.

### Fixes

- **Adding an upstream unlike anything in the store no longer looks hung.** Git offers the
  server every commit it has until the server recognizes one, and a repository sharing nothing
  with the store recognizes none, so its first fetch offered the whole store, again in each
  round: adding `github.com/NixOS/moderation` to a store of 251 upstreams sent 91 MB and took
  over a minute. Now each upstream's fetches offer only the branches of upstreams with the same
  name, its own included. A fork or a mirror still finds the history it shares with its original
  (`github.com/philiptaron/nixpkgs` offers `github.com/NixOS/nixpkgs`'s commits), and a
  repository unlike any other offers nothing.
  - It takes git 2.55 or later; older versions fetch as before.
  - Upstreams already in the store get it from `h store init`.
  - A fork under another name than its original's downloads its whole history the first time, as
    a fresh clone would.

### Under the hood

- **Tests:** 248 unit and end-to-end tests.

## 0.3.1 (2026-10-02)

Fixes found by putting two hundred existing checkouts under the store with `h store ingest`.

### Fixes

- **Repositories with a stray gitlink**, one that `.gitmodules` names no submodule for (such as a
  nested checkout committed by mistake), get their submodules again. `h store ingest`, `h go` and
  the WorktreeCreate hook all used to stop at it, so no submodule was cloned and, with an identity
  in the git options, `h go` failed. Like git, `h` now leaves such a gitlink as it is, and says so.
- **Shallow and partial clones stay out of the store**, as 0.3.0 meant them to:
  - `--depth`, `--shallow-*` and `--filter` given among the git options after `--`, as
    `h-shell-init --git-opts` passes them, used to put the clone's upstream in the store and fetch
    all of its history there. A fork's `upstream` remote was fetched in full beside a shallow clone
    in the same way.
  - `h store ingest` of a shallow or partial checkout no longer adds its upstream to the store,
    which downloaded at once all the history the checkout was made without. It names each upstream
    it leaves out. The checkout still borrows from an upstream the store has already.
- **Store names for GitHub repositories are GitHub's own**: `github.com/NixOS/nixpkgs` however a
  remote spells it, and a renamed or transferred repository under its current name.
  - `h store ingest` used to take the casing a remote's URL was written in, and the wrong name then
    stuck, since the store matches GitHub names whatever their case.
  - An upstream already in the store under a wrong or old name is renamed in place, with its
    branches, tags and default branch. When the right name is there too, the wrong one is dropped.
    Clones that borrow from the store are fixed up as `h` comes to them. `h store add <term>`
    renames one at once.
  - GitHub is only asked when the store does not already have the name, with no token first, as
    elsewhere.
- **`ssh://user@host/...` URLs** no longer keep the user in the name: `ssh://git@github.com/o/r` is
  the store's `github.com/o/r`, fetched over HTTPS, and `h go` clones it to `github.com/o/r`.
- **`h store ingest` puts every remote in the store**, whatever it is called, for the checkout and
  for each submodule. It used to take only `origin` (or a sole remote) and `upstream`, so a second
  fork, or linux's `stable`, stayed out, and a checkout whose remotes had other names had nothing
  put in the store at all. Only the own remote's tags count as the checkout's; the store fetches
  the others' tags from their URLs.
- **`h store ingest` names the remotes it cannot put in the store**, such as an SSH host alias like
  `me.github.com:owner/repo` or a local path, and exits nonzero. Before, it skipped them without a
  word.

### Under the hood

- **Tests:** 246 unit and end-to-end tests.
- **macOS builds** with Nix's sandbox no longer fail the tests that serve a mock GitHub API on
  localhost.

## 0.3.0 (2026-10-01)

`h` gains a shared object store that keeps the upstream history of big projects in one place,
clones that understand GitHub forks and submodules, container clones for projects worked on only
through worktrees, and hooks that give Claude Code's agents worktrees from the store. The shell
functions work as before.

### Commands

- **`h` is a set of commands.** `h [--root DIR] [--store DIR] <command>` takes `go`, `resolve`,
  `store` and `hook`, with the code root and store also read from `$H_CODE_ROOT` and `$H_STORE`.
  Scripts and agents can run it without the shell function:
  - `h go <term>` jumps to the project, cloning it first if needed. It's what the shell function
    runs.
  - `h resolve <term>` prints an existing project's directory. It never clones and never contacts
    GitHub.
  - Git options for every clone go after `--`. The old `h --resolve <root> <term>` form still
    works.
- **`h-shell-init --store DIR`** makes the shell function's clones borrow from that store. Its
  store commands also run in place. `h store` on its own, or followed by anything other than a
  store command, still jumps to a project named `store`.

### Cloning

- **Submodules** are cloned by default, unless an option such as `--no-recurse-submodules`,
  `--recurse-submodules=<pathspec>`, `--bare` or `--mirror` says otherwise. The identity in the
  git options (`-c credential.username=...`, `user.*`) reaches the submodules too, both when their
  credentials are looked up and in their configuration.
- **Forks:**
  - When GitHub says a repository is a fork, the clone gets an `upstream` remote. It can be
    fetched but not pushed to, and the fork's own remote becomes the default push target, even
    when `--origin` or `clone.defaultRemoteName` gives it another name.
  - The upstream is fetched with the clone's own history limits (`--depth`, `--shallow-since`,
    `--shallow-exclude`, `--filter`). A single-branch clone gets only the upstream's default
    branch.
  - The upstream is added before any submodules, even when one of them fails.
  - A submodule given relative to the project, such as `../lib.git`, comes from your fork's copy
    when one exists, and otherwise from beside the upstream. Git alone would only look beside
    the fork.
- **Container clones:** `h <term> --container` clones into `<dir>/.bare`, with a `.git` file beside
  it and no working tree, for projects worked on only through `git worktree add`.
  - `origin` has ordinary remote-tracking branches, and there are no local branches.
  - HEAD is detached at `origin/HEAD`, and moved there again each time `h` goes to the container,
    so a new worktree branch starts from the latest fetched default branch.
  - Worktrees link to the container by relative paths, so the container can be moved with them.
  - The clone takes the remote's object format, SHA-1 or SHA-256.
  - `--no-tags` lasts for later fetches.
  - Options that `git fetch` doesn't share are refused, as are remotes with no default branch.
  - A failed clone leaves nothing behind.
- **GitHub casing:**
  - `h <user>/<repo>` finds an existing checkout in any casing, without the network.
  - GitHub is asked for the canonical casing, and whether the repository is a fork, only when
    there's something to clone.
  - The first request goes without a token, so public repositories never touch git's credential
    helpers. After a 404 (as for a private repository) or a rate limit, `h` asks `git credential
    fill` for the token git keeps for github.com, under each identity's own
    `credential.username`, and tries again.
  - Nothing ever prompts, and the token is only ever sent to the host it was stored for.

### The object store

- **One bare repository holds many upstreams as remotes.** Each is named by the path `h` would clone
  it to, such as `github.com/NixOS/nixpkgs`.
  - Branches live under `refs/remotes/<name>/` and tags under `refs/tags/<name>/`, so nothing
    collides, and `<name>/HEAD` follows each upstream's default branch.
  - When one upstream's name extends another's (`gitlab.com/g/proj` and `gitlab.com/g/proj/sub`),
    the outer one's refs move under `<name>/-/`.
  - Refs are kept in a reftable.
  - The store is never pushed to and never pruned.
- **Clones borrow from it:**
  - Clones made with a store borrow its objects, and so do their submodules, so a clone of a
    fork costs only the fork's own commits.
  - A clone with its full history puts its upstream, and a fork's parent, into the store first,
    and its submodules afterwards, so nothing is downloaded twice. Clones limited by `--depth`,
    `--shallow-*` or `--filter` stay out of it.
  - Clones tell the server only about the store history they share with it, and store fetches
    negotiate with git's skipping algorithm, so a store full of unrelated projects doesn't slow
    either down.
- **Commands:**

  | Command | Does |
  |---|---|
  | `h store init` | Create the store, with the identity from the git options |
  | `h store add <term>...` | Add and fetch upstreams. Every term is checked first, and one whose first fetch fails is taken out again |
  | `h store fetch [-q] [<term>...]` | Fetch everything, or the named upstreams, best-effort: each one that fails is named, the rest are still fetched, and the exit status is nonzero |
  | `h store remove <term>...` | Forget upstreams and their refs. Their objects stay |
  | `h store list` | The upstreams' names |
  | `h store path` | The store's directory |
  | `h store remote <term>` | The store's name for an upstream |
  | `h store show <term> <ref>[:<path>]` | A commit or a file, from a branch, tag or commit hash, with no checkout |
  | `h store worktree <term> <ref> [DIR]` | A cheap clone of one upstream with the names an ordinary clone has |
  | `h store ingest [DIR]` | Put an existing checkout under the store |
  | `h store maintain [hourly\|daily\|weekly]` | Run git's maintenance tasks, none of which deletes objects |

  - **`h store worktree`'s clone** has `origin/*` branches and plain tags, so `git describe` and
    build tooling behave as usual. `git fetch` refreshes it from the store, it can't push to the
    store, and `rm -rf` cleans it up.
  - **`h store ingest`** covers a clone made before the store, or one whose submodules changed in
    a pull. Its remotes and every submodule's repository, at every level, become upstreams.
    Missing submodules are cloned from the store, and local copies of the store's objects are
    dropped. Objects the store lacks, reachable or not, are always kept.
- **Terms:** store commands take the names `h store list` prints, URLs, `user/repo`, or a bare name
  that's unambiguous in the store. A term that isn't in the store is reported as such.
- **Unaffected by global settings:** `fetch.pruneTags=true` in your global configuration doesn't
  touch the store's tags. Neither does SHA-256 as the default for new repositories, since the store
  always uses SHA-1.

### Claude Code worktrees

`h hook worktree-create` and `h hook worktree-remove` work with Claude Code's WorktreeCreate and
WorktreeRemove hooks. See the README for the `settings.json` configuration.

- **`worktree-create`** adds a worktree to the clone the session is in, at
  `<clone>/.claude/worktrees/<name>`, on the branch `worktree-<name>`.
  - The branch starts from the store's copy of the upstream's default branch (a fork's parent's)
    as of the last fetch, without touching the network.
  - Each clone uses the store it borrows from.
  - Its submodules come from the store.
- **`worktree-remove`** commits anything uncommitted to the worktree's branch, gives a detached HEAD
  with commits of its own a branch, and only then removes the worktree. Branches are never deleted.

### Fixes

- `h` ignores `GIT_DIR`, `GIT_WORK_TREE` and git's other repository variables from its environment.
  Run from inside a git hook, it no longer works on the hook's repository instead of its own.
- A failed or rate-limited GitHub lookup no longer clones a second copy of a repository under
  different casing.
- `up` counts a `.git` file as a project root, so it works in worktrees, submodules and container
  clones.

### Requirements

- Reading a store's refs, including from the git of a clone that borrows from it, needs git 2.45 or
  later.
- A container's worktrees need git 2.48 or later.

### Under the hood

- **Tests:** 231 unit and end-to-end tests. They run real git against local repositories, with a
  mock GitHub API and fake credential helpers, and never touch the network.

## 0.2.0 (2026-10-01)

`h` and `up` are now written in Rust. The command-line interface and the shell functions they
generate are unchanged, so existing `eval "$(h-shell-init ...)"` setups keep working.

### New since 0.1.0

- **Tab completion** for bash and zsh, set up automatically by `h-shell-init`. Only the code for the
  shell running the `eval` is emitted.
- **macOS support:**
  - `h-shell-init` and `up-shell-init` find the `h` and `up` next to them, even when run from
    `$PATH`. Before, macOS fell back to `./h` in the current directory.
  - The parent shell is detected through `proc_name`, so completion works on macOS too.
- The GitHub casing lookup uses the system certificate store, through ureq and rustls, instead of
  libcurl and cJSON.

### Fixes

- `h user/repo.git`, and GitHub URLs ending in `.git`, no longer produce a `repo.git.git` clone URL.
- Name search can find directories ending in `.git`, such as bare checkouts.
- When several directories at the same depth match a name, `h` now always picks the first in sorted
  order.
- `~user/...` code roots are no longer turned into `$HOMEuser/...`.
- Paths from URLs are always joined under the code root.
- `up` no longer loops forever on a relative `$PWD`.

### Under the hood

- **Cargo workspace:**
  - `h-core`: helpers shared by both tools
  - `h-git`: resolving terms, search, GitHub lookups, cloning
  - `h-cli`: `h` and `h-shell-init`
  - `up-cli`: `up` and `up-shell-init`
- **Tests:** 110 unit and end-to-end tests. They use a mock GitHub API and a fake `git`, do one real
  local clone, and run the generated functions in bash and zsh. `nix build` runs them.
- **CI:** builds and tests on Linux and macOS, smoke-tests the installed programs, and checks
  rustfmt, clippy and nixfmt.
- **The version** now comes from `Cargo.toml` instead of `git describe`.

## 0.1.0 (2026-01-08)

First tagged release: `h` and `up` rewritten in C, from [zimbatm/h](https://github.com/zimbatm/h).

### Programs

- **`h`** jumps to a project under your code root, cloning it if needed:
  - `h <name>` searches up to three levels deep. The search ignores case unless the name has
    capitals, and deeper matches win.
  - `h <user>/<repo>` goes to `github.com/<user>/<repo>`. The GitHub API supplies the correct
    casing.
  - `h <url>` goes to `<domain>/<path>`. It accepts `https://`, `git://`, `git@host:path` and
    `gitea@host:path`.
- **`h-shell-init`** prints the `h` shell function. It takes `--pushd`, `--name NAME`,
  `--git-opts "OPTIONS"` and an optional code root (default `$H_CODE_ROOT`, else `~/src`).
- **`up`** jumps to the root of the current project, found by `.git`, `.hg`, `.envrc`, `Gemfile` or
  direnv.
- **`up-shell-init`** prints the `up` shell function. It takes `--pushd`.

All four programs accept `-V`/`--version`, and their help messages show the version.
