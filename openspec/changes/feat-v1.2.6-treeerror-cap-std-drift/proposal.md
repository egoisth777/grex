---
slug: feat-v1.2.6-treeerror-cap-std-drift
type: spec
status: active
last_updated: 2026-05-02
---

# feat-v1.2.6 — TreeError variant split + cap-std snapshot hardening + stale manifest doc + working-tree drift root cause

**Status**: draft
**Milestone**: v1.2.6 (PATCH per maintainer Rule 6 — internal error-taxonomy refinement + filesystem TOCTOU hardening + doc hygiene + repo hygiene; `#[non_exhaustive]` on `TreeError`/`QuarantineError` keeps the variant split additive, cap-std migration is implementation-internal, doc + .gitignore changes are non-code)
**Depends on**: v1.2.5 (SHIPPED 2026-05-02 — main @ `5aff26e`, tag `v1.2.5`, all 4 crates @ 1.2.5 live on crates.io). Quarantine GC + restore from v1.2.5 are the precondition for cap-std migration of the same code paths.
**Branch**: feat-v1.2.6

## Why now

Four v1.2.x carry-forwards converged on this release. Each is small, additive, and independent in file scope; bundling them into a single PATCH cuts review noise vs. four separate point releases.

- **TreeError variant split** — `TreeError::ManifestRead` currently bundles every `std::io::Error` raised while reading a `pack.yaml` (path missing, permission denied, IO error mid-read, EISDIR, partial read). Operators see one error message ("failed to read pack manifest: <detail>") for five different failure modes, blocking tooling that wants to surface a categorised remediation hint. v1.2.0 review notes (progress.md:249) flagged the bundling as a MINOR-bump candidate. With `TreeError` already `#[non_exhaustive]` since v1.2.0 (error.rs:18), the split is variant-additive: legacy `ManifestRead(String)` stays as the catch-all fallback and three new categorised variants (`ManifestPermissionDenied`, `ManifestNotADir`, `ManifestIo`) are added. Same pattern for `DirtyTreeRefusal` already shipped in v1.2.0 — v1.2.6 just finishes the categorisation work.

- **cap-std snapshot hardening** — `crates/grex-core/src/tree/quarantine.rs` and `crates/grex-core/src/tree/walker.rs` use ambient `std::fs::*` calls for the snapshot/recursive-copy/recursive-unlink path (quarantine.rs:289 `std::fs::copy`-based snapshot helper; walker.rs:1257-1358 the symlink-aware recursive remover). These calls reopen path components by name on every step, leaving a TOCTOU window: an attacker who can race the walker between `symlink_metadata` and the subsequent `read_dir` / `remove_file` call can swap a directory for a symlink and steer the walker outside the meta root. `walker.md` §326 mandates "hybrid TOCTOU mitigation: cap-std + openat2 internal" but the current impl only uses cap-std for the boundary check at the meta root entry, not for the recursive descent. v1.2.6 closes the gap by routing the snapshot copy + recursive remove through cap-std `Dir` handles, eliminating the per-step reopen.

- **Stale `manifest.md` doc** — `.omne/manifest.md` last_updated is 2026-05-02 but the body is stuck at v1.2.0 vintage. Three concrete drifts: (1) the JSONL example `op` enumeration omits `action_started` / `action_completed` / `action_halted` from the v1.0.x action-bracket events the doc itself describes one paragraph below (the example sequence at line 200-205 only shows `add` / `update` / `rm`); (2) the lockfile schema table at line 100-108 describes 5 fields (`id`, `sha`, `branch`, `installed_at`, `actions_hash`) but does not mention `commit_sha` semantics added in M4 (referenced in cfg/walker.md but absent here); (3) the v1.2.x quarantine event variants (`QuarantineStart`, `QuarantineComplete`, `QuarantineFailed` from v1.2.1; `QuarantineRestored`, `QuarantineGCSwept` from v1.2.5) are entirely missing from the events catalog. Operators reading the doc get a v1.2.0 picture even though the SSOT frontmatter advertises 2026-05-02 freshness.

- **Working-tree drift root cause** — Each session reopens to two persistent untracked artifacts in `git status`:
  1. `CUsersegoisAppDataLocalTempclaude-statusline-probe.txt` at the repo root.
  2. `crates/grex/.grex/events.jsonl`.
  3. Historical: `.gitignore` CRLF/NUL-byte recurrence (progress.md:199).

  Investigation finding: the statusline-probe is a fossil from a 2026-05-01 cc-cfg session that set `statusLine.command` to `echo HELLO_FROM_STATUSLINE > C:\Users\egois\AppData\Local\Temp\claude-statusline-probe.txt && echo STATUSLINE_TEST` to test status-line output capture. On Windows under PowerShell-as-default-shell, the unquoted `C:\...` path collapsed to `CUsersegoisAppDataLocalTempclaude-statusline-probe.txt` (drive colon and backslash separators stripped by the redirection parser when the path contained no surrounding quotes). The redirection landed the file in CWD instead of `%TEMP%`. The cc-cfg statusline command has since been replaced with a proper claude-hud node invocation (current `settings.json:11-14`), but the artifact persists on disk because nothing prunes it.

  The `crates/grex/.grex/events.jsonl` is a runtime artifact from `grex add` invocations made while the cwd was `crates/grex` (e.g. during `cargo run -p grex-cli -- add ...` smoke testing). The CLI writes its event log to `$CWD/.grex/events.jsonl` per cfg/manifest.md, and `crates/grex` is not in `.gitignore`'s scan list because the existing rule `**/grex.jsonl` (gitignore:9-10) only catches the v1.x event-log filename, not the v1.2.0 `<cwd>/.grex/events.jsonl` path.

  The `.gitignore` CRLF/NUL recurrence appears to stem from cross-tool edits (some editors save with CRLF on Windows, then a subsequent Linux/MSYS bash session writes LF, producing mixed line endings; NUL bytes are a separate Windows redirection-encoding artifact). v1.2.6 normalises by rewriting `.gitignore` with explicit LF endings + adding `.gitattributes` to pin the file's eol attribute.

Each item is small (<200 LOC for code items; doc + .gitignore are non-code) and partitions cleanly across parallel workers. Together they finish the v1.2.x stabilisation work before v1.3.0 takes the `--workspace`→`--pack` rename + MINOR cut.

## Scope

- **TreeError variant split** (architecture, additive) — split overloaded `TreeError::ManifestRead(String)` into:
  - `ManifestRead(String)` — retained as catch-all fallback (back-compat).
  - `ManifestPermissionDenied { path: PathBuf }` — `io::ErrorKind::PermissionDenied`.
  - `ManifestNotADir { path: PathBuf }` — `io::ErrorKind::NotADirectory` (or platform equivalent).
  - `ManifestIo { path: PathBuf, source: io::Error }` — wrap the underlying `io::Error` for inspection/log routing without forcing the caller to re-open the file.
  - `#[non_exhaustive]` on the enum already permits additive variants (error.rs:18). Ergonomic only — no behaviour change for the pre-existing fallback path. Producers in `tree/loader.rs:59` route `io::Error.kind()` into the new categorised variants when a kind matches; otherwise fall through to the fallback.
- **cap-std snapshot hardening** (architecture) — migrate `tree/quarantine.rs::snapshot_recursive_copy` and `tree/walker.rs::remove_dir_all_symlink_aware` (line 1257) from ambient `std::fs::*` to `cap_std::fs::Dir`-rooted equivalents. The meta dir's `cap_std::fs::Dir` handle (already obtained at the boundary check) is threaded through the recursion. Eliminates the TOCTOU reopen-by-name pattern. Concurrency invariant — Lean theorem `walker_subpath_resolution_bounded_by_meta_dir` required (Rule 8) covering the property "every path operation issued by the walker resolves under the meta-root capability".
- **Stale `manifest.md` rewrite** (doc) — `.omne/manifest.md` body refresh:
  - Append the v1.0.x action-bracket events to the events catalog AND extend the example sequence at line 200 to include `action_started`/`action_completed`/`action_halted`.
  - Append the v1.2.x quarantine events (`QuarantineStart`, `QuarantineComplete`, `QuarantineFailed`, `QuarantineRestored`, `QuarantineGCSwept`).
  - Document `commit_sha` semantics in the lockfile schema table (currently only in walker.md).
  - Bump `last_updated` only after content lands; pre-commit validate.py keeps the date honest. Ships through SSOT repo (Rule 7).
- **Working-tree drift fix** (repo hygiene):
  - `.gitignore` add: `**/.grex/`, `**/CUsers*claude-statusline-probe*` (defensive), and an explicit `claude-statusline-probe.txt`.
  - `.gitattributes` (new file): pin `* text=auto eol=lf` for all tracked files; explicit `.gitignore text eol=lf` to prevent CRLF/NUL recurrence.
  - One-shot cleanup script under `scripts/cleanup-drift.ps1` (or doc'd manual `git clean -fdX` invocation) to remove the existing fossil artifacts.
  - Documents the cc-cfg statusline-probe fossil cause inline so future sessions know what they are looking at.

## Out of scope

The following items remain deferred to v1.3.0 or later:

- `--workspace` → `--pack` CLI rename (deprecation alias) → v1.3.0.
- v1.3.0 contract freeze + MINOR cut → v1.3.0.
- Full `gix-worktree-state` migration for partial-clone hardening (cap-std covers FS surface; `gix` covers git-protocol surface — separate axis).
- Full SSOT v2 routing-table consolidation (lib/cfg vs cfg/ dedup, owners.yaml) → SSOT roadmap.
- TreeError split for non-Manifest variants (e.g. splitting `Git(GitError)` further) — out of scope; the v1.2.6 split is bounded to the `ManifestRead` overload that v1.2.0 review notes flagged.

No public API removal. Three new `TreeError` variants (additive, gated by `#[non_exhaustive]`). Internal cap-std migration (no public type signature change). Doc + `.gitignore` + new `.gitattributes` (non-code). All additive.

## Acceptance bar

1. **Lean obligations green.**
   - `Grex.Walker.walker_subpath_resolution_bounded_by_meta_dir` proved in `proof/Grex/Walker.lean`. Statement: every path operation `op` issued by the walker during `phase3_handle_child` resolves to a path `p` such that `p` is reachable from the meta-root `Dir` capability without traversing a `..` segment outside the capability. `lake build` green; 0 `sorry`, 0 `admit`.
   - Axiom budget: target ZERO new bridge axioms. Acceptable: 1 new bridge axiom `cap_std_dir_resolution_bounded_by_root` if the cap-std runtime contract cannot be modelled purely (Bridge 11 or 10 → 12). Goal Bridge ≤ 12 post-v1.2.6.
   - TreeError variant split = "simple" per Rule 8 (additive enum variants under `#[non_exhaustive]`; no concurrency invariant; pure routing of `io::Error.kind()`). Document the exemption in `tasks.md` Stage 1.
   - Working-tree drift fix = "simple" per Rule 8 (`.gitignore` + `.gitattributes` is config; not algorithm). Document the exemption.
   - Stale manifest.md = "simple" per Rule 8 (doc-only; no code touched).
2. **TreeError split regression test.** `tree_error_routing_per_io_error_kind` — for each of `PermissionDenied`, `NotADirectory`, generic `Other`, assert `loader::load_pack_manifest` produces the categorised `TreeError` variant (or the `ManifestRead` fallback for `Other`).
3. **cap-std hardening test.** `walker_resolves_under_meta_root_capability` — construct a meta with a malicious symlink to `../../escape/`; assert the walker rejects path resolution outside the cap-std root with a `TreeError::ManifestPathEscape` (existing variant from v1.2.0); assert no `std::fs::*` ambient call is reachable from the snapshot/remove paths via `#[deny(clippy::disallowed_methods)]` lint with `std::fs::*` blocked in those modules.
4. **Manifest.md doc test.** `.omne/scripts/validate.py` exits 0 against the rewritten manifest.md (frontmatter + structure intact). Manual review of the example JSONL block in cfg/history.md against the schema in manifest.md.
5. **Drift no-recur test.** `working_tree_drift_no_recur` — after the cleanup script runs + `.gitignore`/`.gitattributes` land, simulate the cc-cfg statusline-probe write (with the fossil's literal mangled filename) AND a `crates/grex/.grex/events.jsonl` write; assert `git status` reports a clean tree (no untracked files surface). Plus assert `git check-ignore` returns the new patterns for both.
6. **v1.3.0 readiness regression.** Existing `e2e_v1_3_0_readiness_smoke` (added v1.2.4) continues to pass: meta-pack + 1 sub-pack acyclic sync returns `Ok`, `report.errors.is_empty()`, `metas_visited >= 2`. Plus `grex ls`, `grex doctor`, `grex migrate-lockfile` smoke-runs continue to pass. Plus the v1.2.5 `partial_clone_cleanup_after_cancellation`, `prune_quarantine_removes_old_entries`, `restore_quarantine_replaces_dest`, `sync_retain_days_triggers_gc` all still pass on top of the cap-std hardened impl.
7. **No regression on the 380+ existing unit tests + the v1.2.4 `cancellation_aborts_siblings` integration test + the v1.2.3 `e2e_cycle_aborts` integration test + the v1.2.5 quarantine GC/restore tests.**
8. **Local gates clean:** `cargo fmt --all -- --check`, `cargo doc --no-deps --workspace -D warnings`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cd proof && lake build`, axiom-policy check (Bridge ≤ 12).
9. **CI gate.** `#print axioms` smoke check (added v1.2.4) extended to cover `walker_subpath_resolution_bounded_by_meta_dir`. Drift fails CI.
10. **Versioning.** Workspace version 1.2.5 → 1.2.6 in workspace `Cargo.toml`, `crates/xtask/Cargo.toml` path-dep pin, `crates/xtask/tests/version_test.rs` `EXPECTED_WORKSPACE_VERSION`. Man pages regenerated via `cargo xtask gen-man`.
11. **Changelog/history.** `CHANGELOG.md [1.2.6]` entry + `.omne/history.md` v1.2.6 entry (separate SSOT repo per Rule 7).

## v1.3.0 readiness constraint (maintainer directive)

Same constraint as v1.2.4/5: v1.2.x is stabilization on path to v1.3.0 MINOR. Per maintainer constraint:

- **Sub-pack under meta-pack** flow MUST remain functional after v1.2.6 ships.
- **Basic action commands** (`grex sync`, `grex ls`, `grex doctor`, `grex migrate-lockfile`) MUST NOT be broken.
- The existing `e2e_v1_3_0_readiness_smoke` test (codified in v1.2.4) continues to be the regression gate.

v1.2.6 is additive: TreeError split is enum-additive under `#[non_exhaustive]` (no public API removal); cap-std hardening is implementation-internal (no public type signature change); manifest.md is doc-only; .gitignore/.gitattributes is repo hygiene (no runtime code).

v1.2.x → v1.3.0 roadmap (planning, unchanged from v1.2.5):

- v1.2.4: cancellation + polish + tests + axiom CI (SHIPPED 2026-05-02)
- v1.2.5: A2 partial-clone cleanup + A3 pool deadlock guard + quarantine GC/restore + retention policy (SHIPPED 2026-05-02)
- v1.2.6: TreeError variant split + cap-std snapshot hardening + stale manifest.md doc + working-tree drift root cause (THIS PR)
- v1.3.0: `--workspace` → `--pack` CLI rename + behavior contract freeze + MINOR cut

## SemVer

PATCH (1.2.5 → 1.2.6) per maintainer Rule 6. The TreeError split adds variants under an existing `#[non_exhaustive]` enum (additive — downstream `match` arms already require a `_ =>` wildcard per the v1.2.0 stability commitment). The cap-std migration is implementation-internal (no public type signature change). The manifest.md rewrite is doc-only. The `.gitignore`/`.gitattributes` change is repo hygiene with no runtime impact. No manifest/lockfile/binary compatibility break.

Reviewer note: if a reviewer flags the TreeError split as MINOR (because some downstream might pattern-match without `_ =>`), surface to maintainer per Rule 6 — `#[non_exhaustive]` since v1.2.0 means downstream code that compiled against v1.2.0+ already includes the wildcard. Maintainer decides label.

## Process gates (per cfg/workflow.md)

Order of operations is fixed:

1. Phase 1 — OpenSpec (this proposal triplet).
2. Phase 2 — Lean theorem `walker_subpath_resolution_bounded_by_meta_dir` written and `lake build` green BEFORE any Rust impl change for cap-std hardening (Rule 8). TreeError split + manifest.md + drift fix are Rule 8 "simple" exempted (documented in tasks.md). Rust impl second.
3. Phase 3 — review (parallel reviewers + Codex pass).
4. Phase 4 — PR + merge.
5. Phase 5 — wrap-up (CHANGELOG, SSOT history entry, crates.io publish, tag).

A code change that lands before the cap-std Lean proof is a process violation per `.omne/schemas/rules.md` Rule 8. Per discipline 13, commits MUST NOT carry a `Co-Authored-By` trailer.
