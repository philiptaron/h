# h

Fast shell navigation for projects organized as `~/code/<domain>/<path>`.

Rewritten in C from [zimbatm/h](https://github.com/zimbatm/h). Depends on libcurl and cJSON for GitHub API queries.

## Usage

```bash
eval "$(h --setup ~/code)"
```

- `h <name>` - search for project matching `<name>` up to 3 levels deep
- `h <user>/<repo>` - cd to `~/code/github.com/<user>/<repo>` or clone it (queries GitHub API for correct casing)
- `h <url>` - cd to `~/code/<domain>/<path>` or clone it

Also includes `up` - navigate to project root (detected via `.git`, `.hg`, `.envrc`, or `Gemfile`).

## License

MIT - (c) 2015 zimbatm and contributors
