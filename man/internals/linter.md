# linter

Rules enforced on every PR. CI fails on any violation.

## Standard Rust tooling

| Tool | Command | Gate |
|---|---|---|
| `rustfmt` | `cargo fmt --all -- --check` | fail on any diff |
| `clippy` | `cargo clippy --all-targets --all-features -- -D warnings` | fail on any warning |
| `typos` | `typos` | fail on misspellings |
| `cargo-deny` | `cargo deny check` | license + advisory + source gates |

## Clippy configuration

`clippy.toml`:

```toml
avoid-breaking-exported-api = false
msrv = "1.82"
```

Lint levels in `src/lib.rs`:

```rust
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::dbg_macro,
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::todo,
    clippy::unimplemented,
)]
#![warn(
    clippy::pedantic,
    clippy::nursery,
    missing_docs,
)]
```

Tests and benches relax via `#![allow(clippy::unwrap_used, clippy::expect_used)]` at crate root for test binaries.

## Custom rules

### Output centralization

- **No `println!` / `eprintln!` / `print!` / `eprint!` outside `src/cli/output.rs`.**
- Enforced by `clippy::print_stdout` + `clippy::print_stderr` = deny.
- All output goes through the formatter which honors `--json` / `--plain` / TTY detection.

### Error handling discipline

- **Library modules** (`src/manifest`, `src/pack`, `src/plugin`, `src/actions`, `src/packtypes`, `src/fetchers`, `src/concurrency`): use `thiserror` typed errors. `anyhow` banned here.
- **Binary modules** (`src/cli`, `src/main.rs`, `src/mcp`): may use `anyhow`.
- No `unwrap()` / `expect()` in production paths. Startup-only paths may `expect()` with a human-meaningful message if the invariant is unrecoverable (e.g. inventory registry empty = developer bug).

### No direct shell-spawning outside `actions/exec`

- `tokio::process::Command` and `std::process::Command` allowed ONLY in `src/actions/exec.rs`, `src/packtypes/scripted.rs`, and `src/fetchers/git.rs` (for CLI fallback).
- Any other file invoking `Command` fails lint.
- Enforced by CI grep rule:

```bash
if grep -rn 'process::Command' src/ --include='*.rs' \
    | grep -vE '^src/(actions/exec|packtypes/scripted|fetchers/git)\.rs'; then
  echo "shell invocation outside allowed modules"; exit 1
fi
```

### Path rules (ported from legacy `.scripts/test.py`)

- **No hardcoded absolute paths** in source, config, or embedded strings.
  - Banned: `C:\`, `D:\`, `E:\`, `/home/`, `/Users/`, `/mnt/`, `/opt/`.
  - CI grep:

    ```bash
    if grep -rn -E '([A-Z]:\\|/home/|/Users/|/mnt/|/opt/)' src/ --include='*.rs'; then
      echo "hardcoded path detected"; exit 1
    fi
    ```

- **No `~` in source strings.** Home expansion lives in a `PackCtx::env` helper using `dirs::home_dir()`.
- **No string concatenation with path separators.** Use `std::path::PathBuf` + `push()`/`join()`. Clippy's `path_buf_push_overwrite` helps.

### Manifest rules (runtime + lint)

- `pack.yaml` `children[].path` MUST be bare name. Enforced at parse by `pack::schema::validate()` and at doctor-time by `grex doctor`.
- `grex.jsonl` event `path` field likewise bare. No drive letters anywhere in manifest.

### Plugin trait discipline

- Every module under `src/actions/` MUST contain exactly one `impl ActionPlugin`.
- Every module under `src/packtypes/` MUST contain exactly one `impl PackTypePlugin`.
- Every module under `src/fetchers/` MUST contain exactly one `impl Fetcher`.
- Enforced by code review + presence of `inventory::submit!` block.

## Shim rules — N/A

Legacy `.scripts/` had Python-specific shim rules (no `shutil.rmtree`, no `subprocess.run(shell=True)`, etc.). Rust has no direct analogue:

- `std::fs::remove_dir_all` is cross-platform — no native-script indirection needed.
- Shell invocation is already gated by the "no shell-spawning outside allowed modules" rule above.
- Symlinks use `std::os::{unix,windows}::fs` directly.

## CI job

`.github/workflows/lint.yml` (or a job in `ci.yml`):

```yaml
lint:
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v4
    - uses: dtolnay/rust-toolchain@stable
      with: { components: rustfmt, clippy }
    - run: cargo fmt --all -- --check
    - run: cargo clippy --all-targets --all-features -- -D warnings
    - uses: crate-ci/typos@master
    - uses: EmbarkStudios/cargo-deny-action@v1
    - name: hardcoded paths
      run: |
        ! grep -rn -E '([A-Z]:\\|/home/|/Users/|/mnt/|/opt/)' src/ --include='*.rs'
    - name: shell invocation scope
      run: |
        ! grep -rn 'process::Command' src/ --include='*.rs' \
          | grep -vE '^src/(actions/exec|packtypes/scripted|fetchers/git)\.rs'
```

## Pre-commit hook

`.grex/hooks/pre-commit`:

```bash
#!/usr/bin/env bash
set -e
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

Activated by `grex init` via `git config core.hooksPath .grex/hooks`.
