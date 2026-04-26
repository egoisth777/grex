# grex — nested meta-repo manager. Pack-based, agent-native, Rust-fast.

![CI](https://github.com/egoisth777/grex/actions/workflows/ci.yml/badge.svg)
![License: MIT OR Apache-2.0](https://img.shields.io/badge/License-MIT%20OR%20Apache--2.0-blue.svg)

## What is grex?

`grex` manages trees of git repositories as a single addressable graph. Each
node is a "pack" — a plain git repo plus a `.grex/` contract — and every pack
is a meta-pack by construction (zero children = leaf, N children = orchestrator
of N more packs, recursively). One uniform command surface (`sync`, `add`,
`rm`, `update`, `status`, `import`, `doctor`, `teardown`, `exec`, `run`,
`serve`) operates over the whole graph regardless of depth.

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
[`man/release.md` §Verified install](./man/release.md#verified-install-recommended-for-security-sensitive-environments).

Pre-built binaries ship for 5 targets: `x86_64-unknown-linux-gnu`,
`aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`,
`x86_64-pc-windows-msvc`. Anything else falls back to `cargo install grex-cli`.

Both installer one-liners resolve to the latest GitHub Release, built by
`cargo-dist` on every `v*.*.*` tag push (see [`man/release.md`](./man/release.md)).

### Man pages

Every release tarball ships a `man/` directory with one Unix man page per CLI
verb (`grex.1` plus `grex-<verb>.1`). They are auto-generated from the
`clap::Command` tree via `clap_mangen` and are a passive projection of the
CLI surface — never edit them by hand.

If you install via `cargo install grex-cli`, the `man/` directory is **not**
copied into your system man path; install manually after unpacking a release
tarball or checking out the repo:

```sh
# Linux / macOS — per-user
install -Dm644 man/*.1 -t ~/.local/share/man/man1/

# System-wide (requires sudo)
sudo install -Dm644 man/*.1 -t /usr/local/share/man/man1/
```

Then `man grex`, `man grex-sync`, etc. Fish / zsh completions are out of
scope for v1.0.0 and tracked for v1.1. See the mdBook
["Man pages" chapter](https://egoisth777.github.io/grex/man-pages.html) for
the auto-generation flow.

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

## Documentation site

The hosted documentation lives at <https://egoisth777.github.io/grex/>. It is
built from [`man/`](./man/) (the human-readable doc home — `*.1` man pages plus
authored markdown reference) by an mdBook site rooted at
[`grex-doc/`](./grex-doc/). The site deploys to GitHub Pages on every
`v*.*.*` tag push via
[`.github/workflows/doc-site.yml`](./.github/workflows/doc-site.yml).

API reference (post crates.io publish): <https://docs.rs/grex-core> /
<https://docs.rs/grex-mcp>.

Local preview:

```sh
cargo install mdbook mdbook-linkcheck --locked
cargo run -p xtask -- doc-site-prep
mdbook build grex-doc/
mdbook serve grex-doc/   # http://localhost:3000
```

### Source-of-truth design docs

- `openspec/feat-grex/spec.md` — active feature spec
- `.omne/cfg/README.md` — design-doc index (mdBook site is generated from these)
- `progress.md` — current state + last endpoint
- `milestone.md` — phased delivery plan

## Changelog

See [`CHANGELOG.md`](./CHANGELOG.md) — Keep-a-Changelog 1.1.0 format, per-release
entries categorised by Added / Changed / Deprecated / Removed / Fixed / Security.

## Versioning

See [`man/semver.md`](./man/semver.md) — what MAJOR / MINOR / PATCH mean for the
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
