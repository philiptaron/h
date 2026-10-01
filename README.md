# h

Fast shell navigation for projects organized as `~/code/<domain>/<path>`, and a shared object
store that keeps the upstream history of big projects in one place.

Rewritten in Rust from [zimbatm/h](https://github.com/zimbatm/h). Queries the GitHub API to get the canonical casing of `user/repo`.

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
- `h <user>/<repo>` - cd to `~/code/github.com/<user>/<repo>` or clone it (queries GitHub API for correct casing)
- `h <url>` - cd to `~/code/<domain>/<path>` or clone it
- `h <term> [clone options]` - extra options go to `git clone`; `--container` clones into
  `<dir>/.bare` with a `.git` file beside it and no working tree, for projects worked on only
  through `git worktree add`
- `h store <command> ...` - the [object store](#the-object-store) commands; anything else after
  `store`, or nothing, jumps to a project named `store`

Clones recurse into submodules unless an option says otherwise. When GitHub says the repository
is a fork, the clone gets an `upstream` remote that can be fetched but not pushed to (its push
URL is `no_push`) and the fork's remote (`origin`, or whatever `--origin` or
`clone.defaultRemoteName` calls it) becomes the default push target. The upstream is fetched
with the same `--depth`, `--shallow-since`, `--shallow-exclude` and `--filter` as the clone,
and when the clone has a single branch (`--single-branch`, or any of the first three without
`--no-single-branch`), so does the upstream: its default branch.

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
git would look for them in the store's `modules/` directory. Only the first clone does this; for a
submodule added later, pass `--reference "$H_STORE"` to `git submodule update` yourself. A clone
given its own `--reference` with submodules uses only that, as git does. A store keeps its refs in a
reftable, where names that differ only in case coexist even on macOS and pruning one ref does not
rewrite the rest, so anything that reads the store's refs, including the git of a clone that borrows
from it, needs git 2.45 or later.

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
h store worktree nixpkgs staging              # a detached checkout in a temporary directory
h store maintain daily                        # commit-graph, incremental repack, prune worktrees
```

`h store fetch` is best-effort: an upstream that cannot be fetched (gone, or no longer
accessible) keeps none of the others from being fetched. Git names each one that failed, and
the fetch then exits nonzero, so a timer running it shows the failure. `h store add` checks
every term before it changes anything, and takes an upstream it added out again when its first
fetch fails, so the store never holds one that was never fetched.

Terms given to `store` commands match upstreams already in the store first, so a name as
`h store list` prints it works, a bare name such as `nixpkgs` works when it is unambiguous, and
`nixos/nixpkgs` matches `github.com/NixOS/nixpkgs` without asking GitHub.

The `h` binary takes `--root DIR` and `--store DIR`, or `$H_CODE_ROOT` and `$H_STORE`, so
scripts and agents can run it without the shell function:

```bash
h --store ~/.cache/git/me store fetch --quiet
h --root ~/code resolve nixpkgs
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
