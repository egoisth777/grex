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

## Install

Pick one of three paths. All three land the same `grex` binary (v1.0.0+).

### 1. `cargo install` (crates.io)

```sh
cargo install grex-cli
```

The crate is published as `grex-cli`; the installed binary is `grex`. If
pemistahl's unrelated `grex` (regex-from-test-cases tool) is already on
your PATH, pass `--force` to `cargo install grex-cli` or rename the
existing binary first to avoid a silent overwrite.

### 2. Shell installer (Linux / macOS)

```sh
curl -LsSf https://github.com/egoisth777/grex/releases/latest/download/grex-cli-installer.sh | sh
```

### 3. PowerShell installer (Windows)

```powershell
powershell -c "irm https://github.com/egoisth777/grex/releases/latest/download/grex-cli-installer.ps1 | iex"
```

Both installer one-liners are a **convenience path — they do NOT verify
attestations**. For a verified install (SLSA build provenance via
`gh attestation verify`), see
[`docs/release.md` §Verified install](./docs/release.md#verified-install-recommended-for-security-sensitive-environments).

Pre-built binaries ship for 5 targets: `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`,
`x86_64-pc-windows-msvc`. Anything else falls back to `cargo install grex-cli`.

Both installer one-liners resolve to the latest GitHub Release, built by
`cargo-dist` on every `v*.*.*` tag push (see [`docs/release.md`](./docs/release.md)).

## Quickstart

```sh
grex init
grex add https://github.com/egoisth777/grex-inst dev/grex-inst
grex sync
grex doctor
```

### Try with the reference pack template

To see a complete, working pack shape end-to-end, install the reference
template from the in-tree fixture:

```sh
grex add "file://$(pwd)/examples/pack-template"
grex sync
```

The template ships as [`examples/pack-template/`](./examples/pack-template/)
in-tree. Once grex v1.0.0 ships, a standalone mirror repo will be
published and you can install via the clone form:

```sh
# Available at v1.0.0+ release; until then use the file:// form above.
grex add git@github.com:egoisth777/grex-pack-template.git
grex sync
```

See the [mdBook "Pack template" chapter](https://egoisth777.github.io/grex/pack-template.html)
for the ownership / publishing contract.

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

## Documentation

- Hosted mdBook site: <https://egoisth777.github.io/grex/> <!-- TODO: verify at M8-1 tag cut (URL resolves once Pages is enabled on the repo) -->
- API reference (post-M8-2 publish): <https://docs.rs/grex-core> / <https://docs.rs/grex-mcp>
- Local build: `bash docs/build.sh` (or `docs\build.ps1` on Windows) — requires
  `cargo install mdbook --locked`.

### Source-of-truth design docs

- `openspec/feat-grex/spec.md` — active feature spec
- `.omne/cfg/README.md` — design-doc index (mdBook site is generated from these)
- `progress.md` — current state + last endpoint
- `milestone.md` — phased delivery plan

## Changelog

See [`CHANGELOG.md`](./CHANGELOG.md) — Keep-a-Changelog 1.1.0 format, per-release
entries categorised by Added / Changed / Deprecated / Removed / Fixed / Security.

## Versioning

See [`docs/semver.md`](./docs/semver.md) — what MAJOR / MINOR / PATCH mean for the
four public surfaces (manifest schema, CLI surface, MCP tool surface, `pack.yaml`
schema) plus the deprecation policy.

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
