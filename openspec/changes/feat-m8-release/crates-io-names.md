# M8-2 — crates.io name audit, publish order, dry-run findings

**Stage**: [feat-m8-release / M8-2](./spec.md)
**Run date**: 2026-04-22
**Branch**: `feat/m8-release`
**Remote**: `git@github.com:egoisth777/grex.git`

Dry-run-only report. No actual `cargo publish`, no `git tag`, no push.

---

## 1. Name availability

`cargo search <name> --limit 5` results:

| Name                    | Status    | Notes                                                                 |
|-------------------------|-----------|-----------------------------------------------------------------------|
| `grex`                  | **TAKEN** | `grex = "1.4.6"` — pemistahl's regex-from-test-cases tool. Squatted for this project's purpose. |
| `grex-cli`              | FREE      | No match in `cargo search`.                                           |
| `grex-core`             | FREE      | No match.                                                             |
| `grex-mcp`              | FREE      | No match.                                                             |
| `grex-plugins-builtin`  | FREE      | No match.                                                             |
| `grex-tool`             | FREE      | No match.                                                             |
| `grex-pack`             | FREE      | No match.                                                             |
| `meta-grex`             | FREE      | No match.                                                             |
| `grex-rs`               | FREE      | No match.                                                             |
| `graphex`               | TAKEN     | `graphex = "0.2.0"` — a small graph-exploration CLI helper library.   |

### Decision

Ordered fallback per `tasks.md` §2.2: `grex-cli`, `grex-rs`, `graphex`.
`grex-cli` is free, so the **binary crate publishes as `grex-cli`**. Library
crates keep their natural names — all three are free on crates.io.

- `crates/grex/Cargo.toml` — `name = "grex-cli"` (package), `[[bin]] name = "grex"` (installed binary unchanged → users keep typing `grex`).
- `crates/grex-core/Cargo.toml` — `name = "grex-core"`.
- `crates/grex-mcp/Cargo.toml` — `name = "grex-mcp"`.
- `crates/grex-plugins-builtin/Cargo.toml` — `name = "grex-plugins-builtin"`.

Reasoning:
- `cargo install grex-cli` still installs a binary called `grex` — no user-visible churn.
- Library crate names are squatting-free and match the on-disk layout.
- Folder names under `crates/` are unchanged; only the published package name differs.
- Avoids a repo-wide rename and keeps the workspace-internal path deps intact.

**Why not a full rebrand (e.g. `packctl`)?** A total rename was weighed and
rejected. The project has already baked `grex` into too many load-bearing
surfaces for a rebrand to be anything other than strictly worse than a
one-crate suffix:

- **GitHub identity.** The org is `grex-org` and the canonical repo is
  `egoisth777/grex`. Renaming requires a new org + repo, stale-redirect
  risk on every existing clone URL, and a re-do of the GitHub Pages +
  docs.rs + cargo-dist config under the new slug.
- **On-disk workspace convention.** Every pack ships a `.grex/` contract
  directory — that's the pack spec, not a skin. Renaming would break every
  existing pack (including `grex-inst`) and every user's `~/.grex`
  registry on disk.
- **M1–M7 work baked in.** Seven milestones' worth of CLI verbs, MCP tool
  names (`grex.sync`, `grex.doctor`, …), lockfile format headers, error
  codes, and Lean4 invariant proofs reference the `grex` identity. A
  rebrand is a churn multiplier across all of them with zero functional
  payoff.
- **Library crate names are free anyway.** The squat only affects the
  binary package name on crates.io — `grex-core`, `grex-mcp`, and
  `grex-plugins-builtin` are all available and already match the on-disk
  crate folders. A `-cli` suffix on the one affected crate is the smallest
  edit that preserves the install UX (`cargo install grex-cli` →
  binary `grex`).

The one-crate suffix is the surgical fix. Rebrand is a v2+ consideration
only if another crates.io or trademark conflict forces it.

---

## 2. Publish order (topo-sorted)

Based on `[dependencies]` inspection of each crate:

```
grex-core              (no internal deps)
  └── grex-plugins-builtin       (dep: grex-core)
  └── grex-mcp                   (dep: grex-core)
        └── grex-cli             (deps: grex-core, grex-mcp, grex-plugins-builtin)
```

Publish order (strict; wait ~30s between each for crates.io index refresh):

1. `grex-core`
2. `grex-plugins-builtin`
3. `grex-mcp`
4. `grex-cli`

---

## 3. Version state (before → after)

| Location                                       | Before   | After    |
|-----------------------------------------------|----------|----------|
| `[workspace.package] version`                 | `0.0.1`  | `1.0.0`  |
| `[workspace.dependencies] grex-core.version`  | `0.0.1`  | `1.0.0`  |
| `[workspace.dependencies] grex-mcp.version`   | `0.0.1`  | `1.0.0`  |
| `[workspace.dependencies] grex-plugins-builtin.version` | `0.0.1`  | `1.0.0`  |

All four crates already use `version.workspace = true`; no per-crate version
overrides existed. No conversion needed.

Workspace-internal deps use `{ path = "...", version = "1.0.0" }` form —
satisfies the crates.io "no bare path deps" rule.

---

## 4. Metadata state

Workspace-level (`[workspace.package]`):
- `version = "1.0.0"`
- `license = "MIT OR Apache-2.0"`
- `repository = "https://github.com/egoisth777/grex"`
- `homepage = "https://github.com/egoisth777/grex"`
- `readme = "README.md"`
- `authors = ["egoisth777"]`
- `keywords = ["dev-environment", "orchestrator", "pack", "mcp", "cli"]` (new)
- `categories = ["command-line-utilities", "development-tools"]` (new)

Per-crate additions (each ≤5 keywords, ≤5 categories):

| Crate                   | description | documentation | keywords | categories |
|-------------------------|:-:|:-:|:-:|:-:|
| `grex-core`             | existing | `https://docs.rs/grex-core` | `grex, manifest, scheduler, pack, plugin` | `development-tools, config` |
| `grex-mcp`              | existing | `https://docs.rs/grex-mcp` | `grex, mcp, server, tools, agent` | `development-tools, command-line-utilities` |
| `grex-plugins-builtin`  | existing | `https://docs.rs/grex-plugins-builtin` | `grex, plugin, action, pack, builtin` | `development-tools, config` |
| `grex-cli` (bin)        | existing | `https://docs.rs/grex-cli` | `dev-environment, orchestrator, pack, mcp, cli` | `command-line-utilities, development-tools` |

`license`, `repository`, `homepage`, `readme`, `authors` inherited via
`*.workspace = true`. No per-crate `exclude` / `include` needed — `cargo
package` already strips `target/`, `.git/`, and unreferenced siblings; the
largest crate (`grex-core`) packs 85 files / 994KiB uncompressed
(246KiB compressed), well under the 10MB budget.

---

## 5. Dry-run findings

Command (for the root-of-DAG crate):

```
cargo publish --dry-run -p grex-core --allow-dirty
```

### `grex-core`

```
Updating crates.io index
Packaging grex-core v1.0.0
Updating crates.io index
Packaged 85 files, 993.6KiB (245.9KiB compressed)
Verifying grex-core v1.0.0
Compiling grex-core v1.0.0 (target/package/grex-core-1.0.0)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.40s
Uploading grex-core v1.0.0
warning: aborting upload due to dry run
```

**Result: CLEAN.** No metadata warnings. No missing-field warnings. No
keyword/category count warnings. Compiles in the packaged form.

### `grex-plugins-builtin`, `grex-mcp`, `grex-cli`

```
error: failed to prepare local package for uploading
Caused by: no matching package named `grex-core` found
  location searched: crates.io index
```

**Result: NOT LOCALLY DRY-RUNNABLE (structural).** These crates declare
`grex-core = { workspace = true }` which resolves to
`{ path = "crates/grex-core", version = "1.0.0" }`. `cargo publish`
(even with `--no-verify`) consults the crates.io index to check that the
declared version exists — and it does not, because `grex-core` has never
been published. Same applies transitively to `grex-mcp` (depends on
`grex-core`) and `grex-cli` (depends on all three).

This is expected and is precisely why the publish order in §2 exists:
each crate becomes dry-runnable only after its deps are on crates.io.

Mitigation options for CI coverage (not in scope for M8-2):
- Use the `crates-io-placeholder` pattern (publish an empty `0.0.0` seed first).
- Run the dependent dry-runs only from a job triggered post-publish of `grex-core`.
- Use `cargo-release --workspace` at real-publish time — handles ordering automatically.

### Per-crate metadata lint (static review)

- `description` — present on all 4.
- `license` — `MIT OR Apache-2.0` via workspace inheritance (verified by `grex-core` dry-run packaging step).
- `repository` / `homepage` / `readme` — inherited via `*.workspace = true`.
- `keywords` — 5 each, crates.io max is 5 (OK).
- `categories` — 2 each, crates.io max is 5 (OK).
- `authors` — inherited.
- `documentation` — explicit `https://docs.rs/<crate>` per crate.

---

## 6. Renames performed

| Before          | After                         | Reason |
|-----------------|-------------------------------|--------|
| `package.name = "grex"` in `crates/grex/Cargo.toml` | `package.name = "grex-cli"` | `grex` squatted on crates.io by pemistahl regex tool. Binary name `[[bin]] name = "grex"` untouched → installed binary still `grex`. |

No folder rename, no `members` rename, no internal-dep rename. Downstream
users install via `cargo install grex-cli`; the resulting binary on PATH is
`grex`, unchanged.

---

## 7. Follow-ups for actual publish (user-owned)

Commands for the actual release (NOT RUN):

```sh
# Preconditions
git switch main
git pull --ff-only
git tag v1.0.0
# (cargo-dist picks up the tag; see M8-1)

# Manual crates.io publish (strict topo order; wait 30s between)
cargo publish -p grex-core
sleep 30
cargo publish -p grex-plugins-builtin
sleep 30
cargo publish -p grex-mcp
sleep 30
cargo publish -p grex-cli
```

Post-publish smoke:

```sh
cargo install grex-cli
grex --version    # → 1.0.0
```

---

## 8. README note (recommended follow-up)

Once `grex-cli` lands on crates.io, the main `README.md` install section
should show:

```sh
cargo install grex-cli
```

(not `cargo install grex`). Track as a separate doc edit — not in M8-2 scope.
