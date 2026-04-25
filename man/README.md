# `man/` — generated Unix man pages for grex

This directory holds the auto-generated `troff` man pages for the `grex`
CLI surface. They are a **passive projection** of the `clap::Command`
tree built in [`crates/grex/src/cli/args.rs`](../crates/grex/src/cli/args.rs)
and rendered by `clap_mangen` via the `xtask gen-man` subcommand:

```sh
cargo run -p xtask -- gen-man            # write to <workspace>/man
cargo run -p xtask -- gen-man --out-dir /tmp/m
```

**Never edit these files by hand** — the `man-drift` CI job diffs the
committed `.1` files against a freshly generated set on every PR and
fails on any mismatch.

## What is grex?

`grex` is a nested meta-repo manager. Pack-based, agent-native,
Rust-fast. See the [project README](../README.md) for the full pitch.

## Index (15 pages)

One root page plus one page per CLI verb. Names follow the man
convention `grex-<verb>(1)`.

| Page                | One-line                                                           |
| ------------------- | ------------------------------------------------------------------ |
| `grex.1`            | Root page — synopsis, global flags, subcommand index.              |
| `grex-init.1`       | Initialize a grex workspace.                                       |
| `grex-add.1`        | Register and clone a pack.                                         |
| `grex-rm.1`         | Teardown and remove a pack.                                        |
| `grex-ls.1`         | List registered packs.                                             |
| `grex-status.1`     | Report drift vs lockfile.                                          |
| `grex-sync.1`       | Git fetch and pull (recurse by default).                           |
| `grex-update.1`     | Sync plus re-run install on lock change.                           |
| `grex-doctor.1`     | Run integrity checks (`--fix`, `--lint-config`).                   |
| `grex-serve.1`      | Start the MCP stdio server.                                        |
| `grex-import.1`     | Import legacy `REPOS.json`.                                        |
| `grex-run.1`        | Run a named action across packs.                                   |
| `grex-exec.1`       | Execute a shell command in pack context.                           |
| `grex-teardown.1`   | Tear down a pack tree (reverse of `sync` / `install`).             |
| `grex-help.1`       | Print this message or the help of the given subcommand(s).         |

## Installing locally

The `man/` directory ships in every release tarball but is **not**
copied into your system man path by `cargo install grex-cli`. Install
manually:

```sh
# Per-user (Linux / macOS)
install -Dm644 man/*.1 -t ~/.local/share/man/man1/

# System-wide (requires sudo)
sudo install -Dm644 man/*.1 -t /usr/local/share/man/man1/
```

Then `man grex`, `man grex-sync`, etc.

## Rendered HTML site

A browsable HTML version of these pages lives alongside the rest of the
grex narrative docs. Rendered HTML site: see `grex-doc/` (added in PR
#TBD). Until that lands, render any single page locally with:

```sh
man -l man/grex.1                    # interactive pager
groff -mandoc -Thtml man/grex.1 > grex.1.html
```

## Drift contract

If the `man-drift` CI job fails, regenerate locally and commit the diff:

```sh
cargo run -p xtask -- gen-man
git add man/
git commit -m "chore(man): regenerate after CLI surface change"
```

The xtask source lives at [`crates/xtask/src/main.rs`](../crates/xtask/src/main.rs).
