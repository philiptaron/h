# h

Fast shell navigation for projects organized as `~/code/<domain>/<path>`.

Fork of [zimbatm/h](https://github.com/zimbatm/h).

## Usage

```bash
eval "$(h --setup ~/code)"
```

- `h <name>` - search for project matching `<name>` up to 3 levels deep
- `h <user>/<repo>` - cd to `~/code/github.com/<user>/<repo>` or clone it
- `h <url>` - cd to `~/code/<domain>/<path>` or clone it

## License

MIT - (c) 2015 zimbatm and contributors
