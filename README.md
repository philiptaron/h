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

Clones recurse into submodules unless an option says otherwise. When GitHub says the repository
is a fork, the clone gets an `upstream` remote that can be fetched but not pushed to (its push
URL is `no_push`) and `origin`, the fork, becomes the default push target.

## The object store

A store is one bare repository holding many unrelated upstream repositories as remotes, each
named by the path `h` would clone it to. Branches live under `refs/remotes/<name>/` and tags
under `refs/tags/<name>/`, so `github.com/NixOS/nixpkgs/master` and
`github.com/torvalds/linux/v6.12` both resolve and nothing collides. The store is never pushed
to and never pruned, and clones made with a store configured borrow its objects through
`git clone --reference-if-able`, so a clone of a fork costs only the fork's own commits.

```bash
h store init                                  # create it
h store add NixOS/nixpkgs torvalds/linux      # add upstreams and fetch them
h store add https://gitlab.gnome.org/GNOME/gdm.git
h store fetch                                 # fetch everything (run this from a timer)
h store list                                  # the upstreams' names
h store show nixpkgs master:lib/default.nix   # a file, straight from the store
h store show torvalds/linux v6.12             # a commit
h store worktree nixpkgs staging              # a detached checkout in a temporary directory
h store maintain daily                        # commit-graph, incremental repack, prune worktrees
```

Terms given to `store` commands match upstreams already in the store first, so a bare name such
as `nixpkgs` works when it is unambiguous, and `nixos/nixpkgs` matches `github.com/NixOS/nixpkgs`
without asking GitHub.

The `h` binary takes `--root DIR` and `--store DIR`, or `$H_CODE_ROOT` and `$H_STORE`, so
scripts and agents can run it without the shell function:

```bash
h --store ~/.cache/git/me store fetch --quiet
h --root ~/code resolve nixpkgs
```

## up

Also includes `up` - navigate to project root (detected via `.git`, `.hg`, `.envrc`, or `Gemfile`).

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
