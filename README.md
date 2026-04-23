# grex — Cross-platform dev-environment orchestrator

![CI](https://github.com/egoisth777/grex/actions/workflows/ci.yml/badge.svg)
![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)

## What is grex?

`grex` is a pack-based, agent-native, Rust-fast orchestrator for cross-platform
dev-environment setup. It turns your machine bootstrap into a declarative,
reproducible graph of git-backed "packs" that can be synced, updated, and torn
down with a single uniform command surface.

## Pack

A **pack** is a plain git repo plus a `.grex/` contract directory. Everything
outside `.grex/` is opaque payload; everything inside `.grex/` is the pack's
declared contract (manifest, actions, pack-type metadata). Every pack is a
meta-pack by construction — zero-children just means leaf.

## Quickstart

```sh
cargo install grex
grex init
grex add https://github.com/egoisth777/grex-inst dev/grex-inst
grex sync
grex doctor
```

## CLI verbs

| Verb      | Description                                         |
|-----------|-----------------------------------------------------|
| `init`    | Initialize a grex workspace.                        |
| `add`     | Register and clone a pack.                          |
| `rm`      | Teardown and remove a pack.                         |
| `ls`      | List registered packs.                              |
| `status`  | Report drift vs lockfile.                           |
| `sync`    | Git fetch/pull (recurse by default).                |
| `update`  | Sync plus re-run install on lock change.            |
| `doctor`  | Run integrity checks.                               |
| `serve`   | Start the MCP stdio JSON-RPC server (`--mcp`).      |
| `import`  | Import a legacy `REPOS.json`.                       |
| `run`     | Run a named action across matched packs.            |
| `exec`    | Execute a shell command in pack context.            |

Universal flags on every verb: `--json`, `--plain`, `--dry-run`,
`--parallel <N>`, `--filter <EXPR>`.

## Status

M1 scaffold. See `milestone.md` for the roadmap.

## Docs

- `openspec/feat-grex/spec.md` — active feature spec
- `.omne/cfg/README.md` — design-doc index
- `progress.md` — current state + last endpoint
- `milestone.md` — phased delivery plan

## License

Licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](./LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([`LICENSE-MIT`](./LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option. See [`LICENSE`](./LICENSE) for the combined notice.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms
or conditions.
