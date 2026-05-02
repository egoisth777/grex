---
slug: feat-v1.2.6-treeerror-cap-std-drift-tasks
type: spec
status: active
last_updated: 2026-05-02
---

## Stage 0 — branch + dirs (DONE)
- [x] Cut feat-v1.2.6 from main @ 7718889
- [x] mkdir openspec/changes/feat-v1.2.6-treeerror-cap-std-drift

## Stage 1 — Lean theorems (rule 8 gate, MUST land before Rust)
- [ ] Extend `proof/Grex/Walker.lean` with theorem `walker_subpath_resolution_bounded_by_meta_dir`
  - Add `PathOp` inductive (open | readLink | removeFile | removeDir | readDir)
  - Add `CapOp` structure (root: Path, rel: Path, op: PathOp)
  - Add `bounded` predicate (`¬ rel.containsParentEscape`)
  - State: bounded CapOp → resolves under root (cap-std runtime contract)
  - Prove via bridge axiom `cap_std_dir_resolution_bounded_by_root` (or pure model if feasible — goal: pure)
- [ ] Lake build green; 0 sorry; 0 admit
- [ ] `#print axioms walker_subpath_resolution_bounded_by_meta_dir` shows `[propext]` only OR `[propext, cap_std_dir_resolution_bounded_by_root]` (1 new bridge axiom max — justify in `Bridge.lean` if added; current count Bridge≤11 → ≤12)
- [ ] Verify existing theorems still green (theorem extensions must not break prior proofs):
  - `sync_meta_no_cycle_infinite_clone` (v1.2.2/3)
  - `cancellation_terminates_promptly` (v1.2.4)
  - `partial_clone_cleanup_idempotent` (v1.2.5)
  - `pool_deadlock_guard_terminates` (v1.2.5)
- [ ] Document Rule 8 "simple" exemptions inline in this file:
  - TreeError variant split — additive enum variants under `#[non_exhaustive]`; pure `io::Error.kind()` routing; no concurrency invariant
  - Stale manifest.md rewrite — doc-only; zero code; zero invariant impact
  - Working-tree drift fix — `.gitignore` + `.gitattributes` are config; not algorithm

## Stage 2 — Rust implementation (parallel workers, after Stage 1 green)

### 2a — TreeError variant split (error.rs + loader.rs)
- [ ] In `crates/grex-core/src/tree/error.rs`: add 3 new `TreeError` variants under existing `#[non_exhaustive]`:
  - `ManifestPermissionDenied { path: PathBuf }`
  - `ManifestNotADir { path: PathBuf }`
  - `ManifestIo { path: PathBuf, #[source] source: std::io::Error }`
- [ ] Add `is_not_a_directory(&io::Error) -> bool` MSRV-safe helper using `raw_os_error()` (POSIX 20 / Windows 267) — avoids the Rust 1.83 `io::ErrorKind::NotADirectory` requirement (workspace MSRV stays 1.79)
- [ ] In `crates/grex-core/src/tree/loader.rs:59`: route `std::fs::read_to_string` failure via `e.kind()` into the categorised variants; fall through to `ManifestRead(String)` for unmatched kinds (preserves back-compat)
- [ ] Add rustdoc note on `ManifestRead(String)` indicating "preferred: `ManifestIo` for new code; retained for back-compat"
- [ ] Verify the 3 new Display strings render as documented in design.md §"TreeError variant split algorithm"

### 2b — cap-std snapshot hardening (walker.rs + quarantine.rs + consent.rs)
- [ ] In `crates/grex-core/Cargo.toml`: verify `cap-std = "3"` dep version pin (or current stable major); bump if needed
- [ ] In `crates/grex-core/src/tree/walker.rs:1257-1358`: migrate `remove_dir_all_symlink_aware` from `std::fs::*` to cap-std `Dir`-rooted equivalents
  - Function signature: `fn remove_dir_all_symlink_aware(meta_root: &cap_std::fs::Dir, rel: &Path) -> io::Result<()>`
  - Internal `symlink_metadata` / `read_dir` / `remove_file` / `remove_dir` calls dispatch through `meta_root`
- [ ] In `crates/grex-core/src/tree/walker.rs:355-371`: thread the `meta_root: &cap_std::fs::Dir` capability through the boundary check (already cap-std at meta entry — extend to recursion)
- [ ] In `crates/grex-core/src/tree/quarantine.rs:289+`: migrate `snapshot_recursive_copy` to receive `(src_root: &cap_std::fs::Dir, src_rel: &Path, dest_root: &cap_std::fs::Dir, dest_rel: &Path)`. Internal `copy` / `read_dir` / `read_link` / `symlink` dispatch through cap-std handles
- [ ] In `crates/grex-core/src/tree/consent.rs:229,486`: migrate `std::fs::read_dir` + `std::fs::remove_dir_all` to cap-std equivalents; thread `meta_root`
- [ ] Verify: symlinks pointing within meta-root preserved as symlinks (existing v1.2.1 behaviour); symlinks pointing OUTSIDE meta-root are rejected by cap-std with `PermissionDenied`, surfaced as `TreeError::ManifestPathEscape` (existing v1.2.0 variant — reused)
- [ ] Add module-level `#![deny(clippy::disallowed_methods)]` at `tree/walker.rs`, `tree/quarantine.rs`, `tree/consent.rs` (or via `clippy.toml` scoped pattern); update `clippy.toml` to add `std::fs::read_dir`, `std::fs::remove_dir_all`, `std::fs::remove_file`, `std::fs::remove_dir`, `std::fs::symlink_metadata`, `std::fs::copy` to the disallowed list scoped to those modules. Forces future code to use cap-std.

### 2c — Stale manifest.md rewrite (SSOT repo per Rule 7)
- [ ] In `.omne/cfg/manifest.md` (separate SSOT repo working tree): extend the `events.jsonl` event catalog (line 60-77) with the v1.0.x action-bracket events (`action_started`, `action_completed`, `action_halted`) — already described in body but missing from the catalog summary
- [ ] Append v1.2.x quarantine events to the catalog: `QuarantineStart`, `QuarantineComplete`, `QuarantineFailed` (v1.2.1) + `QuarantineRestored`, `QuarantineGCSwept` (v1.2.5). Include each event's JSONL example payload matching existing pattern
- [ ] Extend the example sequence at line 200-205 with the action-bracket events (canonical reproducer for v1.0.x action-bracket schema)
- [ ] Add a normative paragraph to the lockfile schema section (line 100-108) clarifying `commit_sha` semantics: walker-probed HEAD SHA mixed into `actions_hash`; not serialized to disk; cross-references walker.md §M4
- [ ] Run `.omne/scripts/validate.py` — exit 0 against the rewritten manifest.md (frontmatter intact, structure intact)
- [ ] `last_updated: 2026-05-02` already correct; bump if commit lands on a later date
- [ ] Commit + push in SSOT repo (separate from grex per Rule 7)

### 2d — Working-tree drift fix (`.gitignore` + `.gitattributes` + cleanup script)
- [ ] In `E:\repos\utils\grex-org\grex\.gitignore`: add patterns:
  ```
  # v1.2.0+ distributed event-log runtime artifacts (one .grex/ per meta visited)
  **/.grex/

  # cc-cfg 2026-05-01 statusline-probe fossil (path-mangling on PowerShell redirect — see openspec/changes/feat-v1.2.6-*/design.md)
  claude-statusline-probe.txt
  CUsers*claude-statusline-probe*
  ```
- [ ] Create `E:\repos\utils\grex-org\grex\.gitattributes` (new file) with eol=lf pin per design.md §"Fingerprint #3":
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
- [ ] Run `git add --renormalize .` once `.gitattributes` lands — rewrites all tracked files with the pinned eol. Verify NUL bytes (if any) surface during the renormalize and are manually removed
- [ ] Create `E:\repos\utils\grex-org\grex\scripts\cleanup-drift.ps1` (new file): one-shot `Remove-Item` for both fossil paths; idempotent (no-op if fossil absent). Document inline that this script is for one-time cleanup; future drift is prevented by the new .gitignore patterns
- [ ] Run cleanup script once locally; verify `git status` reports clean tree

### 2e — New tests
- [ ] T-E1: `tree_error_routing_per_io_error_kind` in `tree/error.rs` `#[cfg(test)] mod tests`. Construct synthetic `io::Error::from_raw_os_error(13)` (EACCES), `io::Error::from_raw_os_error(20)` (ENOTDIR on POSIX), `io::Error::from_raw_os_error(267)` (Windows ERROR_DIRECTORY), `io::Error::new(ErrorKind::Other, "fake")`. Assert `loader::load_pack_manifest` (or a thin test wrapper) routes each to the expected variant.
- [ ] T-W1: `walker_resolves_under_meta_root_capability` in `tree/walker.rs` `#[cfg(test)] mod tests`. Construct meta with a malicious symlink (`<meta>/escape -> ../../somewhere-else`); call walker; assert returns `TreeError::ManifestPathEscape` (existing variant). Verify the cap-std layer rejected the resolution with `PermissionDenied` before the walker error mapping kicked in.
- [ ] T-W2: `walker_remove_dir_all_through_cap_std` in `tree/walker.rs`. Smoke test: equivalent to pre-v1.2.6 `remove_dir_all_symlink_aware` test but with the cap-std rooted variant. Assert non-malicious removal still works (symlink preserved as symlink within root, dir removed recursively, single file removed).
- [ ] T-D1: `working_tree_drift_no_recur` in `crates/grex-core/tests/drift_norec.rs` (new file). After the cleanup script runs + `.gitignore`/`.gitattributes` land, simulate the cc-cfg statusline-probe write (with the fossil's literal mangled filename `CUsersegoisAppDataLocalTempclaude-statusline-probe.txt`) AND a `crates/grex/.grex/events.jsonl` write (touch file at relative path); assert `git status` reports a clean tree (subprocess via `git2` or `Command::new("git")`). Plus assert `git check-ignore` returns the new patterns for both filenames.

### 2f — CI gate
- [ ] Extend `.github/workflows/ci.yml` axiom-stability smoke check (added v1.2.4) to also assert `#print axioms walker_subpath_resolution_bounded_by_meta_dir`. Update the grep target string to accept the v1.2.6 expected sets (`[propext]` for the theorem, optionally `[propext, cap_std_dir_resolution_bounded_by_root]` if bridge axiom added).

### 2g — Version bumps + man pages
- [ ] Workspace `Cargo.toml` version 1.2.5 → 1.2.6 + 3 path-deps (grex-core, grex-mcp, grex-plugins-builtin)
- [ ] `crates/xtask/Cargo.toml` grex-cli path-dep 1.2.5 → 1.2.6
- [ ] `crates/xtask/tests/version_test.rs` `EXPECTED_WORKSPACE_VERSION` 1.2.5 → 1.2.6
- [ ] Regenerate man pages: `cargo xtask gen-man` — expect drift-free (no flag changes; cap-std + TreeError split + drift fix are all internal)

### 2h — CHANGELOG + history
- [ ] CHANGELOG.md: promote v1.2.5 entry to dated SHIPPED 2026-05-02 if not already; add v1.2.6 entry per design.md "Migration note for changelog"
- [ ] `.omne/cfg/history.md`: append v1.2.6 draft section (separate SSOT repo per Rule 7 — ships through grex-inst)

### 2i — v1.3.0 readiness regression (maintainer directive)
- [ ] Verify existing `e2e_v1_3_0_readiness_smoke` (added v1.2.4) in `crates/grex/tests/sync_e2e.rs` still passes — meta-pack + 1 sub-pack acyclic sync returns `Ok`, `report.errors.is_empty()`, `metas_visited >= 2`, no `CycleDetected` in any error path
- [ ] Verify `grex ls`, `grex doctor`, `grex migrate-lockfile` smoke-runs (manual or scripted) post-impl, before PR push
- [ ] Verify v1.2.5 quarantine GC + restore + retention tests (`prune_quarantine_removes_old_entries`, `restore_quarantine_replaces_dest`, `restore_refuses_existing_dest_without_force`, `restore_ambiguous_without_basename`, `sync_retain_days_triggers_gc`) still pass on top of cap-std hardened impl
- [ ] No new e2e test required for v1.3.0 readiness in v1.2.6 — the v1.2.4 smoke covers the constraint; v1.2.6 only inherits

## Stage 3 — local gates (full pre-push sequence per cfg/workflow.md Phase 2 Step 7)
- [ ] `cargo fmt --all -- --check` exit 0
- [ ] `cargo build --workspace` green
- [ ] `cargo test --workspace` (excluding `dispatch_parallel.rs` per pre-existing UAC issue)
- [ ] Verify NO regression: 380+ existing lib tests pass; v1.2.5 quarantine GC/restore tests pass; v1.2.4 cancellation tests pass; v1.2.3 `e2e_cycle_aborts` pass; v1.2.2 `same_repo_two_refs_no_cycle` pass; cycle_self_loop_aborts/three_node/four_node/nested_prefix/diamond all pass; v1.2.4 `e2e_v1_3_0_readiness_smoke` pass
- [ ] `cargo doc --no-deps --workspace -D warnings` clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean (note: new `clippy::disallowed_methods` rules MUST not produce false positives outside the gated modules)
- [ ] `cd proof && lake build` green
- [ ] axiom counts: Bridge ≤ 12 (or current+1 if 1 new cap-std bridge axiom added; goal current count), Types=4, Other=0
- [ ] Release-build sanity: `cargo build --release --workspace` exit 0
- [ ] `git status` clean — no fossil drift visible

## Stage 4 — review pass (parallel + Codex per cfg/workflow.md Phase 3, discipline 14 dispatch)
- [ ] 4-6 parallel subagent reviewers (non-conflicting write-sets — discipline 14):
  - Lean-Rust correspondence reviewer (cap-std bounded resolution invariant matches Walker.lean theorem)
  - TreeError split correctness (variant routing, MSRV-safe ENOTDIR detection, back-compat preservation)
  - cap-std migration correctness (handle threading, symlink semantics, root-bounded rejection paths)
  - manifest.md doc reviewer (events catalog completeness vs source-of-truth in `event.rs`, lockfile schema clarity, validate.py exit 0)
  - Drift fix reviewer (.gitignore patterns capture both fossils + future variants; .gitattributes eol=lf doesn't break existing tracked files; cleanup script idempotent)
  - SemVer + BC reviewer (3 new TreeError variants under `#[non_exhaustive]` — wildcard arm requirement since v1.2.0 means PATCH; cap-std internal — internal change only)
  - Idiomatic Rust reviewer (cap-std API ergonomics, error chain preservation via `#[source]`, module-level lints don't leak)
- [ ] Apply review fix-ups (separate workers — never the original writers per discipline 14)
- [ ] Codex rescue second pass; skip if no return
- [ ] Re-run gates after fix-ups

## Stage 5 — commit + PR + merge (per cfg/workflow.md Phase 4)
- [ ] Conventional Commit (NO Co-Authored-By per discipline 13): `feat(v1.2.6): TreeError variant split + cap-std hardening + drift fix`
- [ ] git push origin feat-v1.2.6
- [ ] gh pr create --base main --head feat-v1.2.6
- [ ] Watch CI: `gh pr checks <num> --watch --interval 30`
- [ ] After CI green: `gh pr merge <num> --squash --delete-branch`
- [ ] Local: git checkout main; git pull

## Stage 6 — ship (cargo publish + tag)
- [ ] cargo publish topo: grex-core → grex-mcp + grex-plugins-builtin (parallel) → grex-cli (use `--allow-dirty` if `crates/grex/.grex/events.jsonl` runtime artifact reappears post-cleanup-script — same pattern as v1.2.4/5)
- [ ] git tag -a v1.2.6 -m "v1.2.6 — TreeError variant split + cap-std hardening + drift fix (PATCH)" <merge-commit>
- [ ] git push origin v1.2.6

## Stage 7 — wrap-up (per cfg/workflow.md Phase 5)
- [ ] Append `## Endpoint (2026-05-XX, main — v1.2.6 SHIPPED)` to progress.md
- [ ] Update top "Where we are" block
- [ ] Promote draft entry in `.omne/cfg/history.md` to SHIPPED with date + commit SHA (separate SSOT repo per Rule 7)
- [ ] Commit progress.md (grex) + history.md (SSOT)
- [ ] Carry-forward list to v1.3.0: `--workspace` → `--pack` CLI rename, behavior contract freeze, MINOR cut, dead-code removal (`PackLock::acquire` sync variant, `Scheduler::permits`, `DEFAULT_MANAGED_GITIGNORE_PATTERNS` const)
- [ ] Verify drift fix held across the full feat-v1.2.6 cycle: any fresh fossil that re-appears during dev would indicate the cleanup is incomplete — investigate before marking SHIPPED

## Out of scope (defer to v1.3.0+)
- `--workspace` → `--pack` CLI rename → v1.3.0
- v1.3.0 contract freeze + MINOR cut → v1.3.0
- Full `gix-worktree-state` migration for partial-clone hardening (cap-std covers FS surface; gix covers git-protocol surface — separate axis)
- TreeError split for non-Manifest variants (e.g. `Git(GitError)` further decomposition) — keep v1.2.6 scope bounded to the M3-flagged `ManifestRead` overload
- SSOT v2 (owners.yaml, topic-reorg cfg/, lib/cfg dedup, history.md aggregator) → SSOT roadmap, separate repo
