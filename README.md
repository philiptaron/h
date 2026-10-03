# h

Fast shell navigation for projects organized as `~/code/<domain>/<path>`, and a shared object
store that keeps the upstream history of big projects in one place.

Rewritten in Rust from [zimbatm/h](https://github.com/zimbatm/h). Asks the GitHub API for the canonical casing of `user/repo` before cloning.

## Setup

```bash
eval "$(h-shell-init [options] [code-root])"
```

Options:
- `--pushd` - use `pushd` instead of `cd`
- `--name NAME` - use NAME as the shell function name (default: `h`)
- `--store DIR` - the object store that clones borrow from and `h store` operates on
- `--git-opts "OPTIONS"` - git options for every clone, typically `-c user.name=... -c user.email=...`

Tab completion for project names is set up automatically for both bash and zsh.

All four programs (`h`, `h-shell-init`, `up`, `up-shell-init`) print their version with `-V`/`--version`.

## Usage

- `h <name>` - search for project matching `<name>` up to 3 levels deep
- `h <user>/<repo>` - cd to `~/code/github.com/<user>/<repo>`, found in any casing, or clone it (asking the GitHub API first for the canonical casing, and whether the repository is a fork)
- `h <url>` - cd to `~/code/<domain>/<path>` or clone it
- `h <term> [clone options]` - extra options go to `git clone`; `--container` clones into
  `<dir>/.bare` with a `.git` file beside it and no working tree, for projects worked on only
  through `git worktree add`
- `h store <command> ...` - the [object store](#the-object-store) commands; anything else after
  `store`, or nothing, jumps to a project named `store`

GitHub API lookups go without a token first, so public repositories never touch git's credential
helpers. Only when GitHub answers 404, as it does for a private repository, or reports a rate
limit, does `h` ask `git credential fill` for the token git keeps for github.com, with the
`credential.*` settings among the git options (so each identity's `credential.username` picks its
own), and look again with it. Nothing ever prompts: with no stored credential, or one GitHub turns
down, the lookup goes unanswered and the casing stays as typed. The token only ever goes to the
host it is kept for: an API other than `https://api.github.com` (`$H_GITHUB_API`) is asked for its
own host's credential, never github.com's.

Clones recurse into submodules unless an option says otherwise, and the identity in the git
options (`-c credential.username=...`, `user.*`) reaches the submodules' clones and their
configuration too. When GitHub says the repository is a fork, the clone gets an `upstream` remote
that can be fetched but not pushed to (its push URL is `no_push`) and the fork's remote
(`origin`, or whatever `--origin` or `clone.defaultRemoteName` calls it) becomes the default push
target. The upstream is added before the submodules are cloned, and even when one of them fails.
It is fetched with the same `--depth`, `--shallow-since`, `--shallow-exclude` and `--filter` as
the clone, and when the clone has a single branch (`--single-branch`, or any of the first three
without `--no-single-branch`), so does the upstream: its default branch. A fork's submodule given
relative to the project, as `../lib.git` is, comes from the fork's own copy when it has one (found
in the store, or else asked for once) and otherwise from beside the upstream, where git alone
would look only beside the fork.

A container clone has `origin` with ordinary remote-tracking branches and no local branches,
and its HEAD is detached at `origin/HEAD`, so `git worktree add <dir>` starts a new branch from
the default branch. Fetching does not move HEAD, so `h` moves it to `origin/HEAD` again each
time it goes to the container, unless the user has made HEAD a branch. Like `git clone`, it takes the remote's object format (SHA-1 or SHA-256)
rather than the default for new repositories. A remote with no default branch, such as an
empty one, cannot be cloned this way. Only clone options that `git fetch` shares are accepted;
`--no-tags` keeps later fetches from bringing tags too, as it does for `git clone`. Worktrees
added to a container link to it by relative paths (`worktree.useRelativePaths`), so a container
and its worktrees can be moved together; the first such worktree turns on the
`extensions.relativeWorktrees` repository extension, which git before 2.48 cannot read.

## The object store

A store is one bare repository holding many unrelated upstream repositories as remotes, each named
by the path `h` would clone it to (with any part git cannot use in a ref escaped, so `.local`
becomes `_.local`). Branches live under `refs/remotes/<name>/` and tags under `refs/tags/<name>/`,
so `github.com/NixOS/nixpkgs/master` and `github.com/torvalds/linux/v6.12` both resolve and nothing
collides, and every fetch points `<name>/HEAD` at the upstream's current default branch. An upstream
whose name another one extends, as `gitlab.com/g/proj` is extended by `gitlab.com/g/proj/sub`, keeps
its refs under `<name>/-/` instead (`gitlab.com/g/proj/-/main`), so the two never overlap; adding
the inner one moves the outer one's refs over, and they stay there if the inner one is removed
again. The store is never pushed to and never pruned, and clones made with a store configured borrow
its objects through `git clone --reference-if-able`, so a clone of a fork costs only the fork's own
commits. A clone only tells the server about the store history it shares, its own upstream's and a
fork's parent's (through `core.alternateRefsPrefixes`), so a store full of unrelated projects does
not make every clone and fetch list all of their commits first. Their submodules borrow from it too:
`h` clones them itself, with `git submodule update --init --recursive --reference <store>`, since
git would look for them in the store's `modules/` directory. A clone with all of its history goes
into the store itself first: its upstream, and a fork's parent, are added to the store and fetched
there, and the clone then borrows everything. Afterwards, the submodules it cloned that the store
lacked are put there too, from the clone itself, so nothing is downloaded twice; what fails there is
only warned about, since the clone is whole either way. A clone limited by `--depth`,
`--shallow-since`, `--shallow-exclude` or `--filter`, among its own options or the git options after
`--`, does not bring its history into the store. For a submodule added later, run `h store ingest`
(below). A clone given its own `--reference` with submodules uses only that, as git does. A store
keeps its refs in a reftable, where names that differ only in case coexist even on macOS and pruning
one ref does not rewrite the rest, so anything that reads the store's refs, including the git of a
clone that borrows from it, needs git 2.45 or later.

```bash
h store init                                  # create it
h store add NixOS/nixpkgs torvalds/linux      # add upstreams and fetch them
h store add https://gitlab.gnome.org/GNOME/gdm.git
h store remove gdm                            # forget an upstream and its refs; objects stay
h store fetch                                 # fetch everything (run this from a timer)
h store list                                  # the upstreams' names
h store show nixpkgs master:lib/default.nix   # a file, straight from the store
h store show torvalds/linux v6.12             # a commit
h store show nixpkgs 1f0e2d3:flake.nix         # commit hashes work too
h store worktree nixpkgs staging              # a cheap clone at staging, in a temporary directory
h store ingest ~/code/github.com/me/project   # put a checkout and its submodules under the store
h store maintain daily                        # commit-graph, incremental repack, prune worktrees
```

`h store ingest [DIR]` (by default, the checkout here) puts a checkout under the store, as if it had
been cloned with the store in the first place: a clone made before the store, or one whose
submodules a pull has changed. Every one of its remotes becomes an upstream in the store, whatever
it is called, and so does every remote of every submodule's repository, at every level. Submodules not yet cloned are
cloned, borrowing from the store. A gitlink `.gitmodules` names no submodule for, such as a nested
checkout committed by mistake, is no submodule to git, and is named and left as it is. The
checkout and each submodule then borrow the store's objects and give up their own copies of them
(`git repack -a -d -l --cruft`, so nothing the store lacks is lost, reachable or not). An upstream
new to the store is fetched from the checkout first, its remote-tracking branches and tags but
never its local branches, so its history is not downloaded again; the fetch from its URL that
follows brings only what changed. A shallow or partial clone is not fetched from and keeps its
objects, and its upstream is left out of the store, and named, unless the store has it already,
since fetching it there would download all the history the clone was made without. A remote the
store cannot fetch from, such as a local path or an SSH host alias like `me.github.com:owner/repo`,
is named and left out, and counts as something it could not do. A checkout that borrows from
another store already, as one made with another identity's `h` function does, is ingested into that
one, with its own identity. Like `h store fetch`, it does what it can and names what it could not.

`h store fetch` is best-effort: an upstream that cannot be fetched (gone, or no longer
accessible) keeps none of the others from being fetched. Git names each one that failed, and
the fetch then exits nonzero, so a timer running it shows the failure. `h store add` checks
every term before it changes anything, and takes an upstream it added out again when its first
fetch fails, so the store never holds one that was never fetched.

Terms given to `store` commands match upstreams already in the store first, so a name as
`h store list` prints it works, a bare name such as `nixpkgs` works when it is unambiguous, and
`nixos/nixpkgs` matches `github.com/NixOS/nixpkgs` without asking GitHub.

`h store worktree <term> <ref> [DIR]` prints the one directory it made, so scripts and hooks can
use it. The directory is a clone of that upstream alone that borrows the store's objects, with
the names an ordinary clone has: its branches as `origin/*`, `origin/HEAD` at its default branch,
and its tags as plain tags, so `git describe` and build tooling behave as usual and `git fetch`
refreshes it from the store without the network. A branch (or `HEAD`) is checked out as a local
branch tracking `origin/<branch>`, as `git clone --branch` does; a tag or commit is checked out
detached. It cannot push to the store, and removing it is just `rm -rf`. A failure leaves no
directory behind.

The `h` binary takes `--root DIR` and `--store DIR`, or `$H_CODE_ROOT` and `$H_STORE`, so
scripts and agents can run it without the shell function:

```bash
h --store ~/.cache/git/me store fetch --quiet
h --root ~/code resolve nixpkgs
```

## Claude Code worktrees

Claude Code gives agents with `isolation: "worktree"`, and `claude --worktree`, a git worktree of
their own, by default branched from `origin/<default branch>`, which for a fork is the fork's
own branch, usually behind. `h hook worktree-create` and `h hook worktree-remove` replace that
through Claude Code's WorktreeCreate and WorktreeRemove hooks, reading the hook's JSON on stdin:

- **worktree-create** adds the worktree to the clone the session is in (the main clone even from
  one of its worktrees, and the container for a container clone) at
  `<clone>/.claude/worktrees/<name>`, on the branch `worktree-<name>`, the same names Claude Code
  uses itself, and prints its path. The branch starts from the store's copy of the upstream's
  default branch, a fork's parent's or else the clone's own, as of the last `h store fetch`,
  so it is fresh without touching the network. The store is the one the clone borrows objects
  from, whatever `--store` or `$H_STORE` say, so each identity's clones use their own store.
  Without it, the branch starts from `upstream/HEAD`, then `origin/HEAD`, then HEAD. A worktree
  that is already there is handed back, and a branch left by a removed one is checked out again.
  The new worktree's submodules are cloned from that store, at every level, and those it lacks
  are put there, as `h store ingest` does, so the agent gets a tree it can build; what cannot be
  done there is only warned about, since the worktree is usable without it.
- **worktree-remove** commits whatever is uncommitted in the worktree, untracked files included,
  to its branch (with hooks and signing off for that commit), gives a detached HEAD with commits
  of its own a branch, and only then removes the worktree. Branches are never deleted. It leaves
  alone, and fails, anything that is not the top of a worktree, and a worktree that is locked or
  holds work git cannot commit, such as a repository inside it.

Run the `h` binary itself, not the shell function, from Claude Code's `settings.json`:

```json
{
  "hooks": {
    "WorktreeCreate": [
      { "hooks": [{ "type": "command", "command": "/path/to/bin/h hook worktree-create" }] }
    ],
    "WorktreeRemove": [
      { "hooks": [{ "type": "command", "command": "/path/to/bin/h hook worktree-remove" }] }
    ]
  }
}
```

## up

Also includes `up` - navigate to project root (detected via `.git`, `.hg`, `.envrc`, or `Gemfile`).
A `.git` file counts too, so worktrees, submodules and container clones are roots of their own.

```bash
eval "$(up-shell-init [--pushd])"
```

## Development

The code is a Cargo workspace:

- `crates/h-core` - pieces shared by both tools: output, argument parsing, paths, shell detection
- `crates/h-git` - resolving names, `user/repo` and URLs, GitHub lookups, cloning
- `crates/h-cli` - the `h` and `h-shell-init` binaries
- `crates/up-cli` - the `up` and `up-shell-init` binaries

```bash
nix develop     # cargo, clippy, rustfmt, plus git, bash and zsh for the tests
cargo test      # unit tests, plus end-to-end tests that run the shell functions in bash and zsh
nix build       # builds and runs the test suite in the sandbox
```

The tests never touch the network: `H_GITHUB_API` points the GitHub lookup at a local mock server.

## License

MIT - (c) 2015 zimbatm and contributors
