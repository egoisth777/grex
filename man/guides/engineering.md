# engineering

Cargo workspace setup, feature flags, CI matrix, release pipeline, versioning policy.

## Cargo workspace

Single crate `grex` (lib + bin). No sub-crates in v1. `grex-plugin-api` splits out in v2 for ABI-stable plugin authoring.

`Cargo.toml` (root):

```toml
[workspace]
members = ["grex"]
resolver = "2"

[workspace.package]
edition      = "2024"
rust-version = "1.82"
license      = "Apache-2.0 OR MIT"
repository   = "https://github.com/grex-org/grex"
```

`grex/Cargo.toml`:

```toml
[package]
name         = "grex"         # fallback: "grex-cli" if crates.io taken
version      = "0.1.0"
description  = "Cross-platform dev-environment orchestrator. Pack-based, agent-native, Rust-fast."
readme       = "README.md"
keywords     = ["dev-env", "pack", "meta-repo", "mcp", "cli"]
categories   = ["command-line-utilities", "development-tools"]

[[bin]]
name = "grex"
path = "src/main.rs"

[features]
default           = ["git-backend-gix"]
git-backend-gix   = ["dep:gix"]
git-backend-git2  = ["dep:git2"]
simd-json         = ["dep:simd-json"]
tui               = ["dep:ratatui", "dep:crossterm"]     # v2
sqlite            = ["dep:rusqlite"]                     # v2
lean4             = []                                    # marker; CI-only proof job

[dependencies]
tokio              = { version = "1", features = ["full"] }
clap               = { version = "4", features = ["derive"] }
serde              = { version = "1", features = ["derive"] }
serde_json         = "1"
serde_yaml         = "0.9"
anyhow             = "1"
thiserror          = "2"
tracing            = "0.1"
tracing-subscriber = "0.3"
comfy-table        = "7"
owo-colors         = "4"
fd-lock            = "4"
async-trait        = "0.1"
num_cpus           = "1"
inventory          = "0.3"
gix                = { version = "0.66", optional = true }
git2               = { version = "0.19", optional = true }
simd-json          = { version = "0.14", optional = true }
ratatui            = { version = "0.28", optional = true }
crossterm          = { version = "0.28", optional = true }
rusqlite           = { version = "0.32", optional = true, features = ["bundled"] }

[dev-dependencies]
proptest           = "1"
tempfile           = "3"
assert_cmd         = "2"
predicates         = "3"
criterion          = "0.5"
```

Versions pinned at scaffold (M1); refreshed at release (M8).

## Build

| Command | Purpose |
|---|---|
| `cargo build` | dev |
| `cargo build --release` | optimized |
| `cargo build --all-features` | exercise optional features |
| LTO: `[profile.release] lto = "thin"`, `codegen-units = 1` | release speed |

## Test

| Command | Scope |
|---|---|
| `cargo test` | unit + integration default features |
| `cargo test --all-features --workspace` | full matrix |
| `cargo test -p grex --test crash_recovery` | single integration file |
| `cargo bench` | criterion (M2 onward) |

## Lint

```
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
typos
cargo deny check
```

Details in [linter.md](./linter.md).

## CI matrix (`.github/workflows/ci.yml`)

```yaml
name: ci
on: [push, pull_request]
jobs:
  test:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
        toolchain: [stable, beta]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: ${{ matrix.toolchain }}
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all -- --check
      - run: cargo clippy --all-targets --all-features -- -D warnings
      - run: cargo test --all-features --workspace
  lean:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: leanprover/lean-action@v1
      - run: cd lean && lake build
  deny:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: EmbarkStudios/cargo-deny-action@v1
```

## Release pipeline

Tool: `cargo-dist` for cross-compiled release binaries.

Targets:

- `x86_64-unknown-linux-gnu`
- `aarch64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`
- `aarch64-pc-windows-msvc`

Flow:

1. Bump `version` in `Cargo.toml`.
2. Update `CHANGELOG.md`.
3. `git tag vX.Y.Z` + push tag.
4. `release.yml` (cargo-dist-generated) builds artifacts + creates GitHub Release.
5. `cargo publish -p grex` to crates.io.
6. Verify `cargo install grex` clean install on all three OSes (smoke).

## Versioning policy

- **Crate semver** `MAJOR.MINOR.PATCH`. Post-v1:
  - PATCH: bug fix, no API change.
  - MINOR: additive (new verb, flag, action, pack-type, MCP method).
  - MAJOR: any removal, rename, or semantic change of the 8 stable APIs.
- **Manifest schema** (`grex.jsonl` `schema_version` field) — versioned independently. Breaking bump → reader rejects with actionable error pointing to `grex upgrade-schema`.
- **Lockfile schema** — versioned independently (separate cadence from intent log).
- **`pack.yaml` schema_version** — independent. v1 packs must remain readable by any v1.x.
- **MCP method catalog** — tied to CLI verb surface; additions emit `notifications/methods_changed`.

## Toolchain pin

`rust-toolchain.toml`:

```toml
[toolchain]
channel    = "1.82"
components = ["rustfmt", "clippy"]
```

## External tooling required

- `git` CLI (fallback).
- `lake` + `lean` (CI-only, for proof job).
- `cargo-dist` (release-pipeline only).
- `typos` + `cargo-deny` (CI).

## Security

- `cargo deny check` enforces license + advisory gates.
- `#![forbid(unsafe_code)]` at crate root; narrow exceptions via `#[allow(unsafe_code)]` per-module where absolutely needed (fd-lock integration, Windows symlink APIs).
- Supply-chain: consider `cargo vet` in v1.x once stable.
- No shell invocation outside the `actions::exec` module.

## Observability

- `tracing` throughout.
- `tracing-subscriber` wired at binary entry; CLI `-v/-vv/-vvv` controls filter.
- Structured fields: `pack_path`, `action`, `op`, `duration_ms`, `result`.
- v1.x may add on-disk JSON log sink for `grex doctor` retrospection.

## License

Decision locked at M7. Current preference: dual MIT OR Apache-2.0 (Rust-community convention). Alternative single-license choice acceptable if legal reviewer prefers.
