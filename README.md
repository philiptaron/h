# h

Fast shell navigation for projects organized as `~/code/<domain>/<path>`.

Rewritten in C from [zimbatm/h](https://github.com/zimbatm/h). Depends on libcurl and cJSON for GitHub API queries.

## Setup

```bash
eval "$(h --setup [options] [code-root])"
```

Options:
- `--pushd` - use `pushd` instead of `cd`
- `--name NAME` - use NAME as the shell function name (default: `h`)
- `--git-opts "OPTIONS"` - default git clone options (can be overridden per-call)

## Usage

- `h <name>` - search for project matching `<name>` up to 3 levels deep
- `h <user>/<repo>` - cd to `~/code/github.com/<user>/<repo>` or clone it (queries GitHub API for correct casing)
- `h <url>` - cd to `~/code/<domain>/<path>` or clone it

## up

Also includes `up` - navigate to project root (detected via `.git`, `.hg`, `.envrc`, or `Gemfile`).

```bash
eval "$(up --setup [--pushd])"
```

## License

MIT - (c) 2015 zimbatm and contributors
