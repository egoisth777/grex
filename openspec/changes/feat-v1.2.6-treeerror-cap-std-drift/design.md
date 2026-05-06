---
slug: feat-v1.2.6-treeerror-cap-std-drift-design
type: design
status: active
last_updated: 2026-05-02
---

# feat-v1.2.6 — design

**Status**: active
**Spec**: [`proposal.md`](./proposal.md) · [`tasks.md`](./tasks.md)
**SSOT**: `.omne/walker.md` §"TOCTOU mitigation" (canonical cap-std contract) · `.omne/manifest.md` (target of the doc rewrite) · `.omne/toctou.md` (cap-std vs openat2 boundary doc) · `proof/Grex/Walker.lean` (Rule 8 obligation for cap-std hardening)

## Why

v1.2.5 closed the headline cleanup + concurrency gaps (A2 partial-clone cleanup, A3 deadlock guard, quarantine GC/restore). Four carry-forwards remain — each independently small, none gating the v1.3.0 cut on its own:

- **TreeError variant split (M3 review carry-forward, MED).** progress.md:249 records `Dedicated TreeError::QuarantineFailed variant (currently bucketed into DirtyTreeRefusal) — MINOR bump candidate`. The same pattern applies to `ManifestRead(String)` which buckets every `io::Error.kind()` into one stringified detail. v1.2.0 marked `TreeError` `#[non_exhaustive]` precisely so this kind of split could ship under PATCH.
- **cap-std snapshot hardening (M3 carry-forward).** progress.md:250 records `cap-std bounded recursive copy for snapshot read TOCTOU hardening — v1.3 candidate`. walker.md §326 declared the hybrid TOCTOU strategy ("cap-std + openat2 internal") but the recursive descent in walker.rs:1257-1358 + quarantine.rs:289 still uses ambient `std::fs::*`. The boundary check at the meta root is cap-std; the descent is not. v1.2.6 closes the gap one minor early so v1.3.0's contract freeze inherits a clean cap-std story.
- **Stale manifest.md.** SSOT frontmatter says 2026-05-02; body is v1.2.0 vintage with three concrete drifts (action-bracket events missing from example sequence; lockfile schema missing `commit_sha`; v1.2.x quarantine event variants entirely absent from the events catalog).
- **Working-tree drift.** Two persistent untracked artifacts surface in every session's `git status`. Investigation (this design doc, §"Working-tree drift root cause") fingerprints both.

Each item is small and partitions across parallel workers without write-set conflict. Together they ship as a single PATCH that finishes the v1.2.x stabilisation story.

## Architectural context

**TreeError taxonomy** (`crates/grex-core/src/tree/error.rs`) is `#[non_exhaustive]` since v1.2.0 (line 18). 12 variants today: `ManifestNotFound`, `ManifestRead`, `ManifestParse`, `Git`, `CycleDetected`, `PackNameMismatch`, `ChildPathInvalid`, `LegacyLockfileDetected`, `UntrackedGitRepos`, `DirtyTreeRefusal`, `ManifestPathEscape`. The `ManifestRead(String)` variant overloads four distinct `io::Error.kind()` cases into one stringified detail. Producer is `tree/loader.rs:59` (`std::fs::read_to_string` failure) — a single bridge point between the OS error taxonomy and the walker's domain taxonomy.

**Walker FS surface** spans three modules:
- `tree/walker.rs:1257-1358` — `remove_dir_all_symlink_aware` recursive remove.
- `tree/walker.rs:355-371` — boundary-check `symlink_metadata` (already cap-std-fronted at the meta root).
- `tree/quarantine.rs:289+` — `snapshot_recursive_copy` helper.
- `tree/loader.rs:59` — `std::fs::read_to_string(<manifest>)`.
- `tree/consent.rs:229,486` — `read_dir` + `remove_dir_all` for consent-walk and force-prune.

cap-std crate provides `Dir` (a directory capability — opaque file descriptor on POSIX, `HANDLE` on Windows) with methods that operate relative to the held handle: `Dir::open`, `Dir::read_dir`, `Dir::remove_dir_all`, etc. Path resolution is bounded by the root the `Dir` was opened from; `..` segments that would escape return `std::io::ErrorKind::PermissionDenied`. Already a workspace dep (`Cargo.toml` per crates/grex-core/Cargo.toml mention in walker.md §326). v1.2.6 threads the meta-root `Dir` handle through the recursion instead of stopping at the boundary check.

**`.omne/manifest.md`** is the canonical events.jsonl + grex.lock.jsonl schema doc. Drift sources documented in proposal §"Why now".

## TreeError variant split algorithm

```rust
// crates/grex-core/src/tree/error.rs (additive — does NOT remove ManifestRead)
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum TreeError {
    // ... existing 11 variants unchanged ...

    /// Manifest existed but the OS denied read access (UNIX EACCES,
    /// Windows ERROR_ACCESS_DENIED). Operator-actionable: chmod / icacls.
    #[error("permission denied reading pack manifest at `{path}`")]
    ManifestPermissionDenied { path: PathBuf },

    /// Manifest path resolved to a non-directory entry where a directory
    /// was expected (or vice-versa). Distinct from `ManifestNotFound` —
    /// the path exists but has the wrong type. Surfaces as ENOTDIR /
    /// ERROR_DIRECTORY on the producer side.
    #[error("manifest path `{path}` is not a directory (or has wrong type)")]
    ManifestNotADir { path: PathBuf },

    /// Generic IO failure reading a manifest, preserving the
    /// underlying `io::Error` for log routing without forcing the
    /// caller to re-open the file. The catch-all path before the
    /// loader falls through to `ManifestRead(String)` for kinds that
    /// don't match a categorised variant.
    #[error("I/O error reading pack manifest at `{path}`: {source}")]
    ManifestIo { path: PathBuf, #[source] source: std::io::Error },
}
```

Producer routing (in `tree/loader.rs:59`):

```rust
let raw = match std::fs::read_to_string(&manifest_path) {
    Ok(s) => s,
    Err(e) => {
        return match e.kind() {
            io::ErrorKind::NotFound => {
                Err(TreeError::ManifestNotFound(manifest_path.clone()))
            }
            io::ErrorKind::PermissionDenied => {
                Err(TreeError::ManifestPermissionDenied { path: manifest_path.clone() })
            }
            io::ErrorKind::NotADirectory => {  // Rust 1.83+ stable
                Err(TreeError::ManifestNotADir { path: manifest_path.clone() })
            }
            _ => Err(TreeError::ManifestIo {
                path: manifest_path.clone(),
                source: e,
            }),
        };
    }
};
```

`ManifestRead(String)` is retained as a deprecated catch-all (rustdoc note: "prefer `ManifestIo` for new code; this variant is preserved for back-compat"). New producers route via `kind()`. Existing producers in `tree/loader.rs` are updated; the variant stays in the enum because v1.2.0+ downstream consumers may have matched it explicitly.

### Edge cases

- **Rust MSRV.** `io::ErrorKind::NotADirectory` stabilised in Rust 1.83 (workspace MSRV is 1.79 per Cargo.toml:14). Either bump MSRV to 1.83 (CI implications) OR detect ENOTDIR via `e.raw_os_error()` mapping (POSIX 20, Windows 267) under MSRV 1.79 with a helper. Decision: detect via raw_os_error helper to keep MSRV pinned. (One-line `is_not_a_directory(&e)` helper in `tree/error.rs`.)
- **Error chain preservation.** `ManifestIo { source }` uses `#[source]` so `io::Error::source()` chain remains walkable. `tracing` consumers get the structured field automatically.
- **Display format stability.** New variants have new operator-facing strings. Producers that previously emitted `ManifestRead("permission denied (os error 13)")` will now emit `ManifestPermissionDenied { path }`. Operator-facing string changes; external scrapers MUST not rely on the literal string.

### Idempotence

Variant routing is pure: same `io::Error.kind()` always produces the same `TreeError` variant. No memoization or shared state.

## cap-std snapshot hardening algorithm

The migration replaces three call sites that today use ambient `std::fs::*` with cap-std `Dir`-rooted equivalents. The meta-root `Dir` is opened once at the boundary check (already happens) and threaded through the recursion.

```rust
// crates/grex-core/src/tree/walker.rs (BEFORE v1.2.6)
fn remove_dir_all_symlink_aware(path: &Path) -> io::Result<()> {
    let meta = match std::fs::symlink_metadata(path) { ... };
    if meta.file_type().is_symlink() {
        return std::fs::remove_file(path);
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(path)? {
            remove_dir_all_symlink_aware(&entry?.path())?;
        }
        return std::fs::remove_dir(path);
    }
    std::fs::remove_file(path)
}

// AFTER v1.2.6 — root-bounded by `meta_root` cap-std handle
fn remove_dir_all_symlink_aware(meta_root: &cap_std::fs::Dir, rel: &Path) -> io::Result<()> {
    let meta = match meta_root.symlink_metadata(rel) { ... };
    if meta.file_type().is_symlink() {
        return meta_root.remove_file(rel);
    }
    if meta.is_dir() {
        for entry in meta_root.read_dir(rel)? {
            let entry = entry?;
            // Path is relative to meta_root; cap-std rejects `..` escapes.
            let child_rel = rel.join(entry.file_name());
            remove_dir_all_symlink_aware(meta_root, &child_rel)?;
        }
        return meta_root.remove_dir(rel);
    }
    meta_root.remove_file(rel)
}
```

Same pattern for `quarantine.rs::snapshot_recursive_copy`: receives `(src_root: &cap_std::fs::Dir, src_rel: &Path, dest_root: &cap_std::fs::Dir, dest_rel: &Path)` instead of two `&Path`. Internal `read_dir` + `copy` calls dispatch through the cap-std handles.

### Edge cases

- **Cross-meta operations.** Quarantine snapshots from `<meta>/<dest>` to `<meta>/.grex/trash/<ts>/<basename>`. Both src and dest live under the same meta root, so a single `meta_root: &Dir` handle suffices for both.
- **Symlink that targets within the root.** A symlink within meta-root pointing to another path within meta-root is preserved AS A SYMLINK (existing v1.2.1 quarantine semantics). cap-std `read_link` + `symlink` reproduce the link in the dest. Not dereferenced.
- **Symlink that targets outside the root.** Pre-cap-std behaviour: walker silently followed and operated on the target. Post-cap-std: cap-std rejects with `PermissionDenied` because the target resolves outside the root capability. The caller surfaces this as `TreeError::ManifestPathEscape` (the v1.2.0-shipped variant — reused, no new variant).
- **MSRV / cap-std version.** Pin to `cap-std = "3"` (current workspace dep version per `crates/grex-core/Cargo.toml` — verify in tasks.md). API stable since 1.0.
- **Windows long-path handling.** cap-std internally handles Windows path-prefix `\\?\` extension; the walker doesn't need to touch it.

### Idempotence

`remove_dir_all_symlink_aware` is idempotent at the per-call boundary: rerunning on a non-existent path returns Ok. Each step is "remove if exists" semantics consistent with `std::fs::remove_dir_all`.

## Stale manifest.md rewrite

Three concrete drifts (per proposal §"Why now"):

1. **Add `action_started` / `action_completed` / `action_halted` to the example sequence** at line 200-205. Currently shows 4 events (`add`, `add`, `update`, `rm`); extend with the action-bracket events that the doc itself describes one paragraph earlier (line 60-77). The example becomes the canonical reproducer for the v1.0.x action-bracket schema.
2. **Add `commit_sha` semantics to the lockfile schema table** at line 100-108. Currently lists 5 fields (`id`, `sha`, `branch`, `installed_at`, `actions_hash`). Add an "Internal" column entry for `commit_sha` (the walker-probed HEAD SHA used to mix into the actions_hash; not serialized to disk but referenced by walker.md §M4 — clarify the boundary). Decision: don't add a new on-disk field (would be a schema bump); just add a normative paragraph after the table explaining that `actions_hash` is computed against the live `commit_sha` and that this is why ref drift invalidates the skip-on-hash short-circuit.
3. **Add v1.2.x quarantine events to the events catalog** at line 60-77. Currently catalogues `add`/`rm`/`update`/`sync`/`action_started`/`action_completed`/`action_halted`. Append:
   - `QuarantineStart { ts, dest, basename }` (v1.2.1)
   - `QuarantineComplete { ts, dest, basename }` (v1.2.1)
   - `QuarantineFailed { ts, dest, basename, reason }` (v1.2.1)
   - `QuarantineRestored { ts, basename, dest }` (v1.2.5)
   - `QuarantineGCSwept { meta, pruned_count, retained_count }` (v1.2.5)

   Include each event's JSONL example payload (matches the existing pattern).

`last_updated: 2026-05-02` already correct. Frontmatter unchanged. SSOT validate.py runs against the rewritten file.

### Why "simple" per Rule 8

Doc-only rewrite. Zero code. Zero invariant impact. Rule 8 exemption applies — no Lean obligation.

## Working-tree drift root cause

### Fingerprint #1 — `CUsersegoisAppDataLocalTempclaude-statusline-probe.txt`

**Source**: 2026-05-01 cc-cfg session (`E:\repos\cfg\cmn\cc-cfg`) set `settings.json::statusLine.command` to:

```json
"command": "echo HELLO_FROM_STATUSLINE > C:\\Users\\egois\\AppData\\Local\\Temp\\claude-statusline-probe.txt && echo STATUSLINE_TEST"
```

The intent was to probe whether the statusline subprocess could write to the user's TEMP dir. Failure mode: when invoked from a session whose `defaultShell: powershell` was active, PowerShell parsed the `>` redirection differently than bash. The path `C:\Users\egois\AppData\Local\Temp\claude-statusline-probe.txt` was tokenized without preserving the colon + backslash separators because the redirection occurred BEFORE PowerShell quoting kicked in (statusline commands are dispatched via cmd.exe's CreateProcess, then re-tokenized by the shell). Result: the literal filename `CUsersegoisAppDataLocalTempclaude-statusline-probe.txt` (drive colon + path separators stripped) was written to whichever directory the statusline subprocess inherited as CWD — that's the project root for grex-cli sessions. The fossil persisted on disk after settings.json was reverted to a proper claude-hud invocation.

**Fix**: (a) `git rm --cached` (already untracked, no-op); (b) `.gitignore` add a defensive pattern matching the mangled filename: `claude-statusline-probe.txt` and `CUsers*claude-statusline-probe*`; (c) one-shot `Remove-Item` invocation for the existing fossil. The mangled-filename pattern is preserved as defensive armour in case the fossil regenerates from another machine's cc-cfg session.

### Fingerprint #2 — `crates/grex/.grex/events.jsonl`

**Source**: runtime artifact from `grex add` invocations made from `crates/grex/` as CWD. The CLI writes `$CWD/.grex/events.jsonl` per cfg/manifest.md (the v1.2.0 distributed event log). Existing `.gitignore` rule `**/grex.jsonl` (line 9-10) matches the v1.x filename `grex.jsonl` but NOT the v1.2.0 `<dir>/.grex/events.jsonl` path. progress.md:382 records this as parked (`crates/grex/.grex/` test artifact — add to crates/grex/.gitignore during Stage 1n test fixtures`) but the parked work didn't ship.

**Fix**: `.gitignore` add `**/.grex/` (catches `events.jsonl` + `grex.lock.jsonl` + `trash/` under any nested `.grex/` runtime dir). This is broader than just `events.jsonl` but matches operator expectation — `.grex/` is always runtime-owned.

### Fingerprint #3 — `.gitignore` CRLF/NUL recurrence

**Source**: cross-tool edits between Windows editors (CRLF default) and MSYS bash sessions (LF default). When a file is rewritten with mixed line endings, git normalizes inconsistently. NUL bytes appear when PowerShell redirects through a Unicode-encoded pipe without explicit `-Encoding ascii`/`utf8`. progress.md:199 records this as carry-forward.

**Fix**: new `.gitattributes` file at the repo root pinning:

```
* text=auto eol=lf
.gitignore text eol=lf
*.md text eol=lf
*.rs text eol=lf
*.toml text eol=lf
*.yaml text eol=lf
*.yml text eol=lf
*.lean text eol=lf
*.sh text eol=lf
*.ps1 text eol=lf
```

Plus a `git add --renormalize .` invocation documented in tasks.md to rewrite all tracked files with the pinned eol. Fossil NUL bytes (if any present in tracked files) surface during the renormalize and require manual removal.

### Why "simple" per Rule 8

Repo hygiene + config files only. No runtime code touched. No invariant impact. Rule 8 exemption applies — no Lean obligation for the drift fix.

## Lean spec extension

### Theorem: `Grex.Walker.walker_subpath_resolution_bounded_by_meta_dir`

```lean
namespace Grex.Walker

-- A Path operation is one of: open, read_link, remove_file, remove_dir, read_dir.
inductive PathOp where
  | open | readLink | removeFile | removeDir | readDir

-- A capability-rooted operation: (root, relative_path, op). Resolution must
-- not escape `root` via `..` segments.
structure CapOp where
  root : Path        -- the cap-std Dir capability the op runs under
  rel  : Path        -- the relative path argument
  op   : PathOp

-- `bounded` predicate: rel does not contain any `..` segment that would
-- climb above root (cap-std runtime contract).
def bounded (op : CapOp) : Prop :=
  ¬ op.rel.containsParentEscape

theorem walker_subpath_resolution_bounded_by_meta_dir
    (op : CapOp) (h : bounded op) :
    -- The cap-std runtime guarantees `op` resolves to a path under `op.root`.
    -- Bridge axiom: cap_std_dir_resolution_bounded_by_root.
    op.resolves_under op.root := by
  exact cap_std_dir_resolution_bounded_by_root op h

end Grex.Walker
```

The full proof reduces "no `..` escape in rel + cap-std dir capability → resolution under root" to the cap-std runtime contract. Bridge axiom: `cap_std_dir_resolution_bounded_by_root` — asserts the cap-std crate's documented behaviour ("operations through a Dir capability are bounded by the root the Dir was opened from").

### Axiom budget

Target ZERO new axioms. Conservative budget:

- cap-std hardening: 1 new bridge axiom possible (`cap_std_dir_resolution_bounded_by_root`). Goal: avoid by modeling cap-std purely as a "Dir capability rejects rel paths with `..` escape" predicate, no runtime bridge needed.
- TreeError split: 0 new axioms (Rule 8 exempt — no Lean obligation).
- Manifest.md doc rewrite: 0 new axioms (Rule 8 exempt — doc only).
- Drift fix: 0 new axioms (Rule 8 exempt — config only).

**Worst case: Bridge.lean grows to 12 (one new cap-std axiom). Best case: stays at current count (10 or 11 depending on v1.2.5 outcome). Hard ceiling: 12.**

## Files touched

**Rust (production):**

- `crates/grex-core/src/tree/error.rs` — add 3 new `TreeError` variants (`ManifestPermissionDenied`, `ManifestNotADir`, `ManifestIo`) under existing `#[non_exhaustive]`. Add `is_not_a_directory(&io::Error) -> bool` helper for MSRV-safe ENOTDIR detection.
- `crates/grex-core/src/tree/loader.rs` — route `std::fs::read_to_string` failure via `io::Error.kind()` into the new categorised variants; fall through to `ManifestRead` for unmatched kinds.
- `crates/grex-core/src/tree/walker.rs` — migrate `remove_dir_all_symlink_aware` (line 1257) and the boundary `symlink_metadata` (line 355-371) to cap-std `Dir`-rooted equivalents. Thread `meta_root: &cap_std::fs::Dir` through `phase3_handle_child` and recursion.
- `crates/grex-core/src/tree/quarantine.rs` — migrate `snapshot_recursive_copy` (line 289+) to cap-std `Dir`-rooted equivalent. Same `meta_root` threading.
- `crates/grex-core/src/tree/consent.rs` — migrate `read_dir` (line 229) + `remove_dir_all` (line 486) to cap-std equivalents. Thread `meta_root`.
- `crates/grex-core/Cargo.toml` — verify `cap-std` dep version pin; bump if needed (target current stable major).

**Rust (tests):**

- `crates/grex-core/src/tree/error.rs` (`#[cfg(test)] mod tests`) — `tree_error_routing_per_io_error_kind` for each `io::ErrorKind` variant.
- `crates/grex-core/src/tree/walker.rs` (`#[cfg(test)] mod tests`) — `walker_resolves_under_meta_root_capability` (malicious symlink rejected); `walker_remove_dir_all_through_cap_std` (smoke-equivalent of pre-v1.2.6 behaviour for non-malicious inputs).
- `crates/grex-core/tests/drift_norec.rs` (new file) — `working_tree_drift_no_recur` integration test.

**Rust (lints):**

- `crates/grex-core/src/tree/walker.rs` + `quarantine.rs` + `consent.rs` — module-level `#![deny(clippy::disallowed_methods)]` with `clippy.toml` adding `std::fs::*` to the disallowed list scoped to those modules. Forces future code to use cap-std.

**Lean:**

- `proof/Grex/Walker.lean` — extend with `PathOp` + `CapOp` + `bounded` + `walker_subpath_resolution_bounded_by_meta_dir` theorem.
- `proof/Grex/Bridge.lean` — possibly 1 new axiom `cap_std_dir_resolution_bounded_by_root` (Bridge count → 12 max).

**Doc / SSOT (separate repo per Rule 7):**

- `.omne/manifest.md` — rewrite per §"Stale manifest.md rewrite" above.

**Repo hygiene:**

- `.gitignore` — add `**/.grex/`, `claude-statusline-probe.txt`, `CUsers*claude-statusline-probe*`.
- `.gitattributes` (new file) — eol=lf pin per §"Working-tree drift root cause".
- `scripts/cleanup-drift.ps1` (new file) — one-shot Remove-Item for the two fossil paths; idempotent.

**CI:**

- `.github/workflows/ci.yml` — extend `#print axioms` smoke check to cover `walker_subpath_resolution_bounded_by_meta_dir`. Update grep target to accept the new bridge axiom name if added.

**Versioning:**

- `Cargo.toml` (workspace root): `version = "1.2.5"` → `"1.2.6"`.
- `crates/xtask/Cargo.toml`: `grex-cli = { ... version = "1.2.5" }` → `"1.2.6"`.
- `crates/xtask/tests/version_test.rs`: `EXPECTED_WORKSPACE_VERSION` → `"1.2.6"`.
- 3 internal path-deps (grex-core, grex-mcp, grex-plugins-builtin) bumped to 1.2.6.

**Manpages:**

- `cargo xtask gen-man` to regenerate. Expected: no flag changes (TreeError split is internal); cap-std migration is internal; drift fix is repo-config. Man pages should be drift-free post-version-bump.

**Changelog/history:**

- `CHANGELOG.md` — append `[1.2.6] - 2026-05-XX` section.
- `.omne/history.md` — append v1.2.6 entry (separate repo per Rule 7).

## Acceptance criteria

1. `cd proof && lake build` exits 0; zero `sorry`, zero `admit`. Axiom counts: Bridge ≤ 12 (one new cap-std axiom permitted; goal stay at current count), Types = 4, Other = 0.
2. `#print axioms walker_subpath_resolution_bounded_by_meta_dir` shows `[propext]` only or `[propext, cap_std_dir_resolution_bounded_by_root]`.
3. `cargo build --workspace`, `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo doc --no-deps --workspace -D warnings`, `cargo clippy --workspace --all-targets -- -D warnings` all exit 0. (`dispatch_parallel.rs` integration test continues to be excluded per pre-existing Windows UAC os error 740.)
4. New tests pass: `tree_error_routing_per_io_error_kind`, `walker_resolves_under_meta_root_capability`, `working_tree_drift_no_recur`. Plus the 380+ existing lib tests, the v1.2.4 cancellation test, the v1.2.3 e2e cycle test, the v1.2.5 quarantine GC/restore tests, and the v1.2.4 `e2e_v1_3_0_readiness_smoke` all continue to pass.
5. CI smoke check at `.github/workflows/ci.yml` enforces axiom dependency stability for all five headline theorems on every PR (`sync_meta_no_cycle_infinite_clone`, `cancellation_terminates_promptly`, `partial_clone_cleanup_idempotent`, `pool_deadlock_guard_terminates`, `walker_subpath_resolution_bounded_by_meta_dir`).
6. SemVer label: PATCH (1.2.5 → 1.2.6). Per Rule 6 the maintainer has the call; technical reasoning supports PATCH because TreeError split is enum-additive under existing `#[non_exhaustive]` (downstream wildcard arms shipped since v1.2.0); cap-std migration is internal (no public type signature change); manifest.md is doc; .gitignore/.gitattributes are repo hygiene.
7. v1.3.0 readiness: existing `e2e_v1_3_0_readiness_smoke` MUST pass. Asserts: returns `Ok`, `report.errors.is_empty()`, `metas_visited >= 2`, no `CycleDetected` variant in any error path. Plus `grex ls`, `grex doctor`, `grex migrate-lockfile` smoke-runs (manual or scripted) post-impl, before PR push.
8. `git status` after the cleanup script runs is clean (no untracked files surface). `git check-ignore` returns the new patterns for both fossil filenames.

## Migration note for changelog

```
## [1.2.6] — 2026-05-XX

### Added

- Three new `TreeError` variants for finer-grained manifest-read failure
  routing: `ManifestPermissionDenied { path }`, `ManifestNotADir { path }`,
  `ManifestIo { path, source }`. Existing `ManifestRead(String)` retained
  as the catch-all fallback for unmatched `io::ErrorKind` cases.
  Additive under existing `#[non_exhaustive]` (no downstream impact).

### Changed

- Walker filesystem surface (`walker.rs::remove_dir_all_symlink_aware`,
  `quarantine.rs::snapshot_recursive_copy`,
  `consent.rs::read_dir`+`remove_dir_all`) now routes through cap-std
  `Dir` capability handles instead of ambient `std::fs::*` calls.
  Eliminates the per-step path-reopen TOCTOU window. No public API change;
  the cap-std root is opened internally at the meta boundary.

### Internal

- `clippy::disallowed_methods` lint added at the `tree::walker`,
  `tree::quarantine`, `tree::consent` module level to block future
  ambient `std::fs::*` regression.

### Tests

- New unit tests `tree_error_routing_per_io_error_kind`,
  `walker_resolves_under_meta_root_capability`.
- New integration test `working_tree_drift_no_recur`.

### Repo hygiene

- `.gitignore`: added `**/.grex/`, `claude-statusline-probe.txt`,
  `CUsers*claude-statusline-probe*` patterns to catch v1.2.0 distributed
  event-log runtime artifacts and the cc-cfg statusline-probe fossil.
- `.gitattributes` (new): pins `eol=lf` for all tracked files to prevent
  CRLF/NUL recurrence on cross-tool edits.
- `scripts/cleanup-drift.ps1` (new): one-shot fossil cleanup script.

### Notes

- No public API change. New TreeError variants are additive under
  `#[non_exhaustive]`. cap-std migration is implementation-internal.
- `.omne/manifest.md` rewritten in the SSOT repo (separate from grex
  per Rule 7) — events catalog now lists v1.0.x action-bracket events
  + v1.2.x quarantine events; lockfile schema clarifies `commit_sha`
  semantics.
```
