# m3-review-findings

Master finding list from the M3-close review series, plus mapping to the fix PRs that landed on `main`.

- **Date**: 2026-04-20
- **Baseline**: `main` at `d160c7c` (M3 Stage B close).
- **Final state**: `main` at `7ce186e` (5 fix PRs merged).
- **Test count**: 316 → 344.

## Methodology

Eight parallel reviews were run:

- **Codex adversarial passes (4)** — semver hygiene, data-integrity, concurrency, cross-platform. Prompted to find breakage, not to polish.
- **Analytical subagent passes (4)** — docs / rustdoc coverage, perf / allocations, recovery / crash-resume, security audit.

**7 of 8 returned usable synthesis.** The security audit stalled at synthesis twice (codex truncated mid-report on both retries). Security retry is filed under open carry-forwards rather than being treated as a clean pass — do not assume the review was completed.

Each review produced a file:line-cited report; the master list below is the synthesized severity grouping the reviewer saw at close.

## Master finding list

Severity legend: **CRITICAL** (correctness / data loss) · **HIGH** (wrong result under realistic input) · **MEDIUM** (bad UX / minor correctness) · **LOW** (cosmetic / edge) · **NIT** (style).

### CRITICAL

| # | Finding | Evidence |
|---|---|---|
| C1 | Concurrent `grex sync` against the same workspace could interleave manifest appends — no workspace-level lock existed | `crates/grex-core/src/sync/mod.rs` (pre-#16); concurrency review report |
| C2 | Manifest could record a successful `Sync` for an action that panicked mid-side-effect — readers had no way to detect partial apply | `crates/grex-core/src/sync/emit.rs` (pre-#15) |
| C3 | Symlink backup path: after `rename(dst → .grex.bak)` succeeded, a failed `symlink()` left the user with no original file and no new symlink | `crates/grex-core/src/execute/fs/symlink.rs` (pre-#18) |

### HIGH

| # | Finding | Evidence |
|---|---|---|
| H1 | `VarEnv` was case-sensitive on Windows → `$USERPROFILE` vs `$UserProfile` resolved differently | `crates/grex-core/src/vars/env.rs` (pre-#17) |
| H2 | `DupSymlinkValidator` compared `dst` paths byte-for-byte → duplicates that differ only in case passed validation on case-insensitive FSes | `crates/grex-core/src/pack/validate/dup_symlink.rs` (pre-#17) |
| H3 | `kind: auto` silently defaulted to `file` when `src` was missing, creating a dangling file-symlink where `directory` was required | `crates/grex-core/src/execute/fs/symlink.rs` (pre-#17) |
| H4 | Concurrent sync on the same clone dest could race the bare fetch vs the checkout | `crates/grex-core/src/git/backend/gix.rs` (pre-#16) |
| H5 | All public enums / arg structs were implicit `#[non_exhaustive]`-missing → adding a variant in M4 would be a SemVer major | `crates/grex-core/src/**` (pre-#14) |
| H6 | `ExecNonZero` carried the full stderr → event size could exceed fd-lock append atomicity ceiling | `crates/grex-core/src/execute/fs/exec.rs` (pre-#18) |

### MEDIUM

| # | Finding | Evidence |
|---|---|---|
| M1 | `Action` name was `&'static str` → plugin-provided names (heap-allocated) could not register | `crates/grex-core/src/pack/action.rs` (pre-#14) |
| M2 | No pre-run scan for stale locks / orphaned `.grex.bak` files → surfaced only on next hit | recovery review report |
| M3 | Dirty-check ran before lock acquire → TOCTOU window between check and `materialise_tree` | `crates/grex-core/src/sync/mod.rs` (pre-#16) |
| M4 | `HOME → USERPROFILE` fallback also fired in `insert` → user-explicit `HOME` insert was silently retargeted | `crates/grex-core/src/vars/env.rs` (pre-#17) |
| M5 | No `ExecResult::Skipped` variant → M4 idempotency skip would force a non-additive enum change | `crates/grex-core/src/execute/result.rs` (pre-#14) |

### LOW

| # | Finding | Evidence |
|---|---|---|
| L1 | Unicode NFC/NFD path equality not handled (macOS) | cross-platform review |
| L2 | Windows MAX_PATH: no `\\?\` prefix for long paths | cross-platform review |
| L3 | POSIX `mode` on Windows `mkdir` silently ignored — no warning | cross-platform review |
| L4 | README status line claims "M1" — stale vs actual M3-complete | docs review |
| L5 | `CONTRIBUTING.md` missing | docs review |
| L6 | PR template missing | docs review |
| L7 | ~39% rustdoc gap concentrated in `grex` CLI crate | docs review |
| L8 | Only 1 file has rustdoc code examples | docs review |
| L9 | `Arc<PackManifest>` would eliminate multiple per-action clones | perf review |
| L10 | Batched manifest appends under single lock acquire | perf review |
| L11 | Predicate cache on `ExecCtx` — repeated `cmd_available` probes | perf review |
| L12 | `Cow<str>` hot path in `vars::expand` | perf review |
| L13 | `gix` shallow-clone option exposed via `SyncOptions` | perf review |

### NIT

| # | Finding |
|---|---|
| N1 | Inconsistent `tracing` span names across sync path |
| N2 | Several test names begin with `test_` (clippy `items_after_statements` style) |

## Mapping: finding → PR → resolution

Fix PRs on `main`:

- **A = PR #14** — semver hygiene
- **B = PR #15** — data integrity (event brackets + halt context)
- **C = PR #16** — concurrency (workspace + repo fd-locks, TOCTOU closure)
- **D = PR #17** — cross-platform (VarEnv, case-folding, kind:auto)
- **E = PR #18** — recovery (backup rollback, recovery scan, stderr cap)

| # | Finding (short) | PR | Resolution |
|---|---|---|---|
| C1 | workspace-concurrent sync | C (#16) | resolved — `<workspace>/.grex.sync.lock` fail-fast |
| C2 | partial-apply undetectable | B (#15) | resolved — `ActionStarted/Completed/Halted` + `SyncError::Halted(Box<HaltedContext>)` |
| C3 | backup-then-create atomicity | E (#18) | resolved — rename-back on create failure; `SymlinkCreateAfterBackupFailed` if rollback fails |
| H1 | Win case-sensitive VarEnv | D (#17) | resolved — two-map (`inner` + ASCII-lowercase `lookup_index`) |
| H2 | DupSymlink case-sensitive | D (#17) | resolved — ASCII case-fold on Windows/macOS |
| H3 | `kind: auto` silent default | D (#17) | resolved — `ExecError::SymlinkAutoKindUnresolvable` |
| H4 | repo-concurrent race | C (#16) | resolved — `<dest>.grex-backend.lock` sibling file |
| H5 | missing `#[non_exhaustive]` | A (#14) | resolved — applied workspace-wide (list in PR description) |
| H6 | unbounded stderr in events | E (#18) | resolved — 2 KB truncation cap |
| M1 | plugin name heap-alloc | A (#14) | resolved — `Cow<'static, str>` |
| M2 | no startup recovery scan | E (#18) | resolved — informational scan (auto-cleanup deferred to `grex doctor` M4+) |
| M3 | dirty-check TOCTOU | C (#16) | resolved — revalidated after lock + immediately before `materialise_tree` |
| M4 | `HOME→USERPROFILE` in `insert` | D (#17) | resolved — fallback only in `from_os` / `from_map` |
| M5 | no `Skipped` variant | A (#14) | reserved — variant added, emission deferred to M4 lockfile idempotency |
| L1 | NFC/NFD equality | — | **deferred** (carry-forward) |
| L2 | MAX_PATH `\\?\` | — | **deferred** (carry-forward) |
| L3 | POSIX mode on Win warn | — | **deferred** (carry-forward) |
| L4 | README stale | — | **deferred** (docs carry-forward) |
| L5 | CONTRIBUTING.md | — | **deferred** (docs carry-forward) |
| L6 | PR template | — | **deferred** (docs carry-forward) |
| L7 | rustdoc gap | — | **deferred** (docs carry-forward) |
| L8 | no rustdoc examples | — | **deferred** (docs carry-forward) |
| L9–L13 | perf items | — | **deferred** (perf carry-forward; not on M4 critical path) |
| N1–N2 | nits | — | **punted** (no ticket) |

## Deferred findings (remain open)

Grouped for triage when M4 planning starts:

### Security
- **Security review retry** — codex synthesis stalled twice. Re-run with a smaller scope or a different synthesizer before claiming a clean security pass.

### Docs
- README status line (M1 → M3).
- Add `CONTRIBUTING.md`.
- Add PR template.
- Close the 39% rustdoc gap (primary offender: `grex` CLI crate).
- Add rustdoc code examples to at least the public `grex-core` surface.

### Perf
- `Arc<PackManifest>` to eliminate clones across the sync pipeline.
- Batched manifest appends under a single fd-lock acquire.
- Predicate cache on `ExecCtx` (repeated `cmd_available` etc.).
- `Cow<str>` on the `vars::expand` hot path.
- Expose `gix` shallow-clone option via `SyncOptions`.

### Platform edges (LOW)
- Unicode NFC/NFD path equality (macOS).
- Windows `\\?\` long-path prefix for MAX_PATH.
- POSIX-only `mode` field on `mkdir` should warn on Windows.

## Cross-refs

- `progress.md` — "Decisions locked during M3 review series" mirrors the decisions captured in the PR descriptions.
- `.omne/cfg/concurrency.md` — updated to document workspace + repo fd-lock contract.
- `.omne/cfg/manifest.md` — updated to document `ActionStarted` / `ActionCompleted` / `ActionHalted` event brackets.
- `.omne/cfg/actions.md` — updated to document symlink backup-rollback, `kind: auto` missing-src error, and exec stderr truncation.
