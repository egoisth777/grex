# Man pages

`grex` ships a full set of Unix man pages — one root page plus one per CLI
verb. They are a **passive projection** of the `clap::Command` tree defined
in [`crates/grex/src/cli/args.rs`](https://github.com/egoisth777/grex/blob/main/crates/grex/src/cli/args.rs);
never edit the `.1` files by hand.

## What ships

14 files under `man/` at the repo root:

| Page | Covers |
|------|--------|
| `grex.1`          | Top-level binary + global flags (`--json`, `--plain`, `--dry-run`, `--filter`) |
| `grex-init.1`     | `grex init` |
| `grex-add.1`      | `grex add <url> [path]` |
| `grex-rm.1`       | `grex rm <path>` |
| `grex-ls.1`       | `grex ls` |
| `grex-status.1`   | `grex status` |
| `grex-sync.1`     | `grex sync` (parallel + `--only` + `--ref`) |
| `grex-update.1`   | `grex update [pack]` |
| `grex-doctor.1`   | `grex doctor --fix --lint-config` |
| `grex-serve.1`    | `grex serve` (MCP stdio) |
| `grex-import.1`   | `grex import --from-repos-json` |
| `grex-run.1`      | `grex run <action>` |
| `grex-exec.1`     | `grex exec <cmd> …` |
| `grex-teardown.1` | `grex teardown` |

## Generating

The generator lives in `crates/xtask/` and is invoked via the
`cargo xtask` alias configured in [`.cargo/config.toml`](https://github.com/egoisth777/grex/blob/main/.cargo/config.toml):

```sh
cargo xtask gen-man                    # write to <workspace>/man/
cargo xtask gen-man --out-dir /tmp/m   # write elsewhere
```

Internally the binary calls `clap_mangen::Man::new(cmd).render(&mut buf)` once
for the root `Command` and once per subcommand. The subcommand name is
prefixed with `grex-` so the `.TH` header reads `grex-sync(1)` instead of
`sync(1)`.

## CI drift check

CI runs a `man-drift` job on every PR (see
[`.github/workflows/ci.yml`](https://github.com/egoisth777/grex/blob/main/.github/workflows/ci.yml)):

1. `cargo run -p xtask -- gen-man`
2. `git diff --exit-code -- man/` — fails if the generated output differs
   from the committed files.

If you touch `crates/grex/src/cli/args.rs` (add a verb, rename a flag,
edit a `/// help` doc comment) you **must** re-run `cargo xtask gen-man`
and commit the regenerated `.1` files or CI will reject the PR.

## Release artifact inclusion

`man/` is listed in `[workspace.metadata.dist].include` in the root
[`Cargo.toml`](https://github.com/egoisth777/grex/blob/main/Cargo.toml),
so every `cargo-dist`-built release tarball ships the full man-page set
alongside `README.md`, `CHANGELOG.md`, and the licenses.

## Installing

See the [README "Man pages" section](https://github.com/egoisth777/grex#man-pages)
for the one-line `install -Dm644` incantation. The shell / PowerShell
installer one-liners do **not** install pages into the system man path —
manual copy is required for now.
