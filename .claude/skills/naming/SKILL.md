---
name: naming
description: How to name functions, methods, types and locals in h. Use when adding, renaming or reviewing any fn, method, struct or variable in this repository, or when a name reads oddly in review.
---

# Naming in h

## Functions and methods are commands

A function's name is an imperative verb phrase. Read a call aloud as an order to its receiver
or first argument, "X, <name> Y!", and it should say what the call does:

- `repo.run(&args)`: "repo, run args!"
- `store.fetch(&names)`: "store, fetch names!"
- `repo.resolve_commit("HEAD")`: "repo, resolve the commit HEAD!"

A noun name fails the test, and can mislead: `repo.commit("HEAD")` reads as "make a commit"
when it only looks one up. A name that hides an effect is the worst case. A function that may
create a branch is `ensure_branch`, not `branch`.

### Where the rule applies

- **Everything that does I/O**, meaning it runs git, touches files, talks to the network or
  prints, is named for what it does: `capture`, `read_status`, `find_base`, `ensure_in_store`,
  `check_out_worktree`, `report_fetch`.
- **Pure computations** that only derive a value from their arguments may keep Rust's noun style
  for getters (no `get_` prefix): `config_args`, `history_opts`, `remote_name`, `refspecs`.

### Exceptions

- Trait methods and conventional constructors and conversions keep their standard names: `fmt`,
  `default`, `new`, `from_*`, `as_*`, `into_*`, `main`.
- Predicates are questions, not commands: `is_complete`, `has_refs`, `exists`. Spell them with
  `is_`/`has_` (or a third-person verb such as `checks_out`, `extends`) so they read as
  questions, and never as a bare noun or adjective (`complete`, `single_branch`).

## Words

- **Tenant**: a repository that gets its objects from the store, which its
  `objects/info/alternates` names, and keeps only the objects the store lacks. Clones made with a
  store are tenants. `h store ingest` makes an existing checkout and its submodules tenants.
- **Upstream**: a remote held in the store, named by the path `h` would clone it to
  (`github.com/NixOS/nixpkgs`).
- Keep **borrow** as the verb for what alternates do, which is git's own word, but don't name
  types or functions after it (`Borrower`, `borrow()`), since in Rust it means something else.
- Avoid nouns that already mean something in Rust or git for new types: `Ref`, `Owner`,
  `Remote`, `Clone`, `Source` unless it is exactly git's sense.

## Locals

- A `Git` for one repository is bound once per function as `repo`:
  `let repo = git::at(dir);`. Use `store` for the store's own (`let store = self.git();`) and a
  role name (`clone`) when two are in play. Don't call it `git`, which reads like the module.
- Bind it once and reuse it rather than building `git::at(dir)` for each call.
