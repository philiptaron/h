# h

Fast shell navigation for projects organized as `~/code/<domain>/<path>`, and a shared object
store that keeps the upstream history of big projects in one place.

Rewritten in Rust from [zimbatm/h](https://github.com/zimbatm/h).

## Setup

```bash
eval "$(h-shell-init [options] [code-root])"
```

| Option | Meaning |
| --- | --- |
| `--pushd` | use `pushd` instead of `cd` |
| `--name NAME` | name the shell function NAME (default: `h`) |
| `--store DIR` | the [object store](#the-object-store) that clones borrow from |
| `--git-opts "OPTIONS"` | git options for every clone, typically `-c user.name=... -c user.email=...` |

Tab completion for project names works in bash and zsh.

All four programs (`h`, `h-shell-init`, `up`, `up-shell-init`) print their version with
`-V`/`--version`.

## Usage

| Command | What it does |
| --- | --- |
| `h <name>` | go to the project matching `<name>`, up to 3 levels deep |
| `h <user>/<repo>` | go to `~/code/github.com/<user>/<repo>`, in any casing, or clone it |
| `h <url>` | go to `~/code/<domain>/<path>`, or clone it |
| `h <term> [clone options]` | pass extra options to `git clone` |
| `h <term> --container` | clone with no working tree, for [worktrees only](#container-clones) |
| `h store <command>` | the [object store](#the-object-store) commands |

`h store` with no command, or one it doesn't know, goes to a project named `store`.

### Cloning from GitHub

Before cloning `user/repo`, `h` asks the GitHub API for the repository's canonical casing and
whether it is a fork.

When it is a fork:

- The clone gets an `upstream` remote for the parent. It can be fetched but not pushed to (its
  push URL is `no_push`).
- The fork's own remote (`origin`, or whatever `--origin` or `clone.defaultRemoteName` names it)
  becomes the default push target.
- `upstream` is fetched as shallowly or partially as the clone (`--depth`, `--shallow-*`,
  `--filter`), and only its default branch when the clone has a single branch.
- A submodule given relative to the project, such as `../lib.git`, comes from the fork's copy if
  it has one (in the store, or else asked for once), and otherwise from beside the parent. Git
  alone would look only beside the fork.

### GitHub credentials

Lookups go without a token first, so public repositories never touch git's credential helpers.

Only when GitHub answers 404 (as for a private repository) or reports a rate limit does `h` ask
`git credential fill` for the token git keeps for github.com, and try again. The `credential.*`
settings in the git options apply, so each identity's `credential.username` picks its own token.

Nothing ever prompts. With no stored credential, or one GitHub turns down, the casing stays as
typed. The token only goes to the host it was stored for: an API other than
`https://api.github.com` (set with `$H_GITHUB_API`) gets its own host's credential, never
github.com's.

### Submodules

Clones recurse into submodules unless an option says otherwise. The identity in the git options
(`-c credential.username=...`, `user.*`) reaches the submodules' clones and their configuration
too.

### Container clones

`h <term> --container` makes a clone for projects worked on only through `git worktree add`:

- The repository is in `<dir>/.bare`, with a `.git` file beside it and no working tree.
- It has `origin` with its remote-tracking branches, and no local branches.
- HEAD is detached at `origin/HEAD`, so `git worktree add <dir>` starts a new branch from the
  default branch. `h` moves HEAD to `origin/HEAD` again each time it goes to the container, unless
  you have made HEAD a branch.
- Like `git clone`, it takes the remote's object format (SHA-1 or SHA-256).
- Worktrees link to it by relative paths (`worktree.useRelativePaths`), so the container and its
  worktrees can be moved together. The first one turns on a repository extension that git before
  2.48 can't read.

Only the clone options that `git fetch` shares are accepted. `--no-tags` keeps later fetches from
bringing tags, as with `git clone`. A remote with no default branch, such as an empty one, can't be
cloned this way.

## The object store

A store is one bare repository that holds many upstream repositories as remotes. Clones borrow
its objects, so a clone of a fork costs only the fork's own commits, and a project's history is
downloaded once however many clones and identities use it.

```bash
h store init                                  # create it
h store add NixOS/nixpkgs torvalds/linux      # add upstreams and fetch them
h store add https://gitlab.gnome.org/GNOME/gdm.git
h store remove gdm                            # forget an upstream and its refs; objects stay
h store fetch                                 # fetch everything (run this from a timer)
h store list                                  # the upstreams' names
h store show nixpkgs master:lib/default.nix   # a file, straight from the store
h store show torvalds/linux v6.12             # a commit
h store show nixpkgs 1f0e2d3:flake.nix        # commit hashes work too
h store worktree nixpkgs staging              # a cheap clone at staging, in a temporary directory
h store ingest ~/code/github.com/me/project   # put a checkout and its submodules under the store
h store maintain daily                        # commit-graph, incremental repack, prune worktrees
```

The store needs git 2.45 or later, and so does any clone that borrows from it.

### Names and refs

Each upstream is named by the path `h` would clone it to, such as `github.com/NixOS/nixpkgs`.

- Branches are under `refs/remotes/<name>/` and tags under `refs/tags/<name>/`, so
  `github.com/NixOS/nixpkgs/master` and `github.com/torvalds/linux/v6.12` both resolve.
- Every fetch points `<name>/HEAD` at the upstream's current default branch.
- Parts git can't use in a ref name are escaped: `.local` becomes `_.local`.
- When one name extends another, as `gitlab.com/g/proj/sub` extends `gitlab.com/g/proj`, the
  outer one's refs move under `<name>/-/` (`gitlab.com/g/proj/-/main`), so the two never overlap.

A GitHub repository is named as GitHub names it, in its casing and under its current owner and
name, whatever form its URL takes. It is always fetched over HTTPS. If the store has it under
another casing, or under an old name from before a rename or transfer, it is renamed in place,
branches, tags and HEAD with it, or dropped if the store has the new name as well. No objects
go either way. So `h store add nixos/nixpkgs` fixes a store that says `github.com/nixos/nixpkgs`.

- GitHub is asked only when the store doesn't already have the name spelled exactly so. When it
  can't be asked, a casing the store already has is used, and nothing is renamed.
- A clone that borrowed under the old name is pointed at the new one the next time
  `h store ingest` or the WorktreeCreate hook touches it.

Terms given to `store` commands match what the store already has first. A name as
`h store list` prints it works, a bare name like `nixpkgs` works when it is unambiguous, and
`nixos/nixpkgs` finds `github.com/NixOS/nixpkgs` without asking GitHub. Only `h store add` asks
GitHub about a `user/repo` the store has in another casing, to rename it as above.

The store is never pushed to and never pruned.

### Clones and the store

With a store configured, a clone:

1. adds its upstream, and a fork's parent, to the store and fetches them there;
2. borrows those objects (`git clone --reference-if-able`), so it downloads almost nothing itself;
3. clones its submodules borrowing from the store too, then puts any the store lacked into it,
   from the clone, so nothing is downloaded twice.

A clone only tells servers about the store history it shares (its upstream's and a fork's
parent's), so a store full of unrelated projects doesn't slow down every fetch.

A shallow or partial clone (`--depth`, `--shallow-*` or `--filter`) still borrows from the store
but doesn't add its history to it. A clone given its own `--reference` uses only that, as git
does.

### Putting existing checkouts under the store

`h store ingest [DIR]` (by default, the checkout you're in) makes a checkout look as if it had been
cloned with the store from the start. Use it for a clone made before the store, or after a pull
brings in new submodules.

- Every remote of the checkout becomes an upstream in the store, and so does every remote of
  every submodule, at every level.
- Submodules not yet cloned are cloned, borrowing from the store.
- The checkout and its submodules then drop their own copies of the store's objects
  (`git repack -a -d -l --cruft`). Nothing the store lacks is lost.
- A new upstream is fetched from the checkout first (its remote-tracking branches and tags,
  never its local branches), then from its URL, so its history isn't downloaded again.
- A shallow or partial clone isn't fetched from, and keeps its objects.
- A checkout that already borrows from another store, such as another identity's, is ingested
  into that one.

It does what it can and names what it couldn't, exiting nonzero if anything failed. It leaves out,
and names:

- a shallow or partial clone's upstream, unless the store has it already;
- a remote the store can't fetch from, such as a local path or an SSH host alias like
  `me.github.com:owner/repo`;
- a gitlink that `.gitmodules` doesn't name as a submodule, such as a nested checkout committed
  by mistake.

### Fetching

`h store fetch` keeps going when an upstream can't be fetched (it's gone, or no longer
accessible), names each one that failed, and then exits nonzero, so a timer running it shows the
failure.

`h store add` checks every term before changing anything, and takes an upstream back out if its
first fetch fails.

### Worktrees from the store

`h store worktree <term> <ref> [DIR]` makes a cheap clone of one upstream that borrows the store's
objects, and prints its directory.

- It looks like an ordinary clone: branches as `origin/*`, `origin/HEAD` at the default branch,
  and tags as plain tags, so `git describe` and build tools behave as usual.
- `git fetch` refreshes it from the store, without the network.
- A branch is checked out as a local branch tracking `origin/<branch>`; a tag or commit is
  checked out detached.
- It can't push to the store. Remove it with `rm -rf`.

## Running `h` without the shell function

The `h` binary takes `--root DIR` and `--store DIR`, or `$H_CODE_ROOT` and `$H_STORE`, so scripts
and agents can use it directly:

```bash
h --store ~/.cache/git/me store fetch --quiet
h --root ~/code resolve nixpkgs
```

## Claude Code worktrees

Claude Code gives agents with `isolation: "worktree"`, and `claude --worktree`, a worktree of
their own, branched by default from `origin/<default branch>`. For a fork, that is usually behind
its parent. `h` replaces this through Claude Code's WorktreeCreate and WorktreeRemove hooks. Run
the `h` binary itself, not the shell function, from `settings.json`:

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

### worktree-create

Adds the worktree at `<clone>/.claude/worktrees/<name>`, on the branch `worktree-<name>` (the
names Claude Code uses itself), and prints its path.

- It goes in the main clone, even when the session is in one of its worktrees, and in the
  container for a container clone.
- The branch starts from the store's copy of the upstream's default branch (a fork's parent's,
  or else the clone's own) as of the last `h store fetch`, so it is fresh without touching the
  network. The store is the one the clone borrows from, so each identity uses its own.
- Without a store, the branch starts from `upstream/HEAD`, then `origin/HEAD`, then HEAD.
- Submodules are cloned from the store, at every level, and those the store lacks are put there.
  What fails there is only warned about, since the worktree is usable without it.
- A worktree that is already there is handed back, and a branch left by a removed one is checked
  out again.
- A directory that isn't a worktree, such as one a build still running in a removed worktree
  wrote into again, is moved aside to the first free `<name>~1`, `<name>~2`, … and left there.

### worktree-remove

Removes the worktree without losing anything:

- Whatever is uncommitted, untracked files included, is committed to the worktree's branch, with
  hooks and signing turned off for that commit.
- A detached HEAD with commits of its own gets a branch.
- Branches are never deleted.

It refuses, leaving everything in place, for anything that isn't the top of a worktree, a locked
worktree, or one holding work git can't commit, such as a repository inside it.

## up

`up` goes to the root of the project you're in, found by its `.git`, `.hg`, `.envrc` or
`Gemfile`. A `.git` file counts too, so worktrees, submodules and container clones are roots of
their own.

```bash
eval "$(up-shell-init [--pushd])"
```

## Development

The code is a Cargo workspace:

- `crates/h-core` - pieces shared by both tools: output, argument parsing, paths, shell detection
- `crates/h-git` - resolving names, `user/repo` and URLs, GitHub lookups, cloning, the store
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
