# h

Fast shell navigation for projects organized as `~/code/<domain>/<path>`.

Rewritten in Rust from [zimbatm/h](https://github.com/zimbatm/h). Queries the GitHub API to get the canonical casing of `user/repo`.

## Setup

```bash
eval "$(h-shell-init [options] [code-root])"
```

Options:
- `--pushd` - use `pushd` instead of `cd`
- `--name NAME` - use NAME as the shell function name (default: `h`)
- `--git-opts "OPTIONS"` - git options for every clone, typically `-c user.name=... -c user.email=...`

Tab completion for project names is set up automatically for both bash and zsh.

All four programs (`h`, `h-shell-init`, `up`, `up-shell-init`) print their version with `-V`/`--version`.

## Usage

- `h <name>` - search for project matching `<name>` up to 3 levels deep
- `h <user>/<repo>` - cd to `~/code/github.com/<user>/<repo>` or clone it (queries GitHub API for correct casing)
- `h <url>` - cd to `~/code/<domain>/<path>` or clone it
- `h <term> [clone options]` - extra options go to `git clone`

Clones recurse into submodules unless an option says otherwise.

When GitHub says the repository is a fork, the clone gets an `upstream` remote that can be
fetched but not pushed to (its push URL is `no_push`) and `origin`, the fork, becomes the default
push target.

The `h` binary takes `--root DIR`, or `$H_CODE_ROOT`, so scripts and agents can run it without
the shell function:

```bash
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
