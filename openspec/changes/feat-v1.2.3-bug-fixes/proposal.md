# feat-v1.2.3 — bug fixes

**Status**: draft
**Milestone**: v1.2.3 (PATCH; bug fixes only, no API change, no new features)
**Depends on**: v1.2.2 (SHIPPED 2026-05-02 — main @ 92ec7fd, tag v1.2.2)
**Branch**: feat/v1.2.3

## Why now

v1.2.2 closed the `sync_meta` cycle-detection BLOCKER and shipped the Rule-8 Lean theorem `sync_meta_no_cycle_infinite_clone`. A follow-up review pass against the merged code surfaced three bugs adjacent to the cycle-detection surface — each is a localised correctness or UX defect in code that landed in v1.2.0–v1.2.2, not an architectural concern. Three regression tests were also identified as missing coverage for shapes the v1.2.2 unit tests did not exercise (diamond, longer cycle, non-top-level cycle).

The defects are small, surgical, and independent. Bundling them into a single PATCH release is preferable to six separate point releases or to deferring them into the v1.3 cycle, where they would race feature work.

## Scope

(B3 dropped post-draft: verified non-bug — CLI already uses Display via display_cycle_detected at error.rs:162-171.)

Three bug fixes (B1, B2, B4) and three regression tests (T1–T3), all confined to `crates/grex-core/src/tree/walker.rs`, the `proof/Grex/Walker.lean` model + theorem, and the workspace version metadata.

- **B1** (correctness) — `phase3_recurse` (`walker.rs:1043-1048`) early-returns on `next_depth > opts.max_depth` BEFORE the per-child cycle check inside `phase3_handle_child` runs. A cyclic manifest with cycle length greater than `max_depth` therefore truncates silently as `Ok-Truncated` instead of surfacing `TreeError::CycleDetected`. Fix: cycle check fires at the recurse edge regardless of depth; depth-cap remains a separate Ok-Truncated outcome that is checked AFTER the cycle check.
- **B2** (UX) — `pack_identity_for_child` (`walker.rs:323-326`) renders `format!("url:{url}@{ref}")` even when `ref` is `None`/empty, producing identities like `url:https://x.git@` with a dangling trailing `@`. Fix: omit the `@<ref>` suffix when ref is empty/None. Membership equality is unaffected (string-equality only; both sides see the same formatter).
- **B4** (diagnostic) — root frame seeds `visited` with `&[]` (`walker.rs:654`), so the cycle chain reported by `phase3_recurse` starts at the first child, never the root pack identity. Fix: seed `visited` with `pack_identity_for_root(meta_dir)` at the public `sync_meta` entry. The chain shape changes (now starts at root), but the safety theorem holds — adding to the initial visited set only strengthens the precondition.

Three new tests under `walker::tests`:

- **T1** — diamond (root → A, root → B, A → C, B → C). C is a shared descendant; no cycle. Walker completes Ok with no false-positive `CycleDetected`. Locks the per-child clone-of-visited invariant under the diamond-on-same-ref shape.
- **T2** — 4-node cycle (root → A → B → C → D → A). Asserts `Err(CycleDetected)` with chain length ≥ 5 (root + A + B + C + D + A). Extends v1.2.2's 3-node coverage by one hop.
- **T3** — nested-prefix cycle (root → A → B → C, with B → C → B forming an inner cycle). Outer arm is acyclic; inner B-C-B cycle. Asserts `Err(CycleDetected)` with the chain reflecting the inner loop. Locks detection at non-top-level cycles where the prefix is acyclic.

## Out of scope

The 14 deferred items from `progress.md` "Known v1.2.2+ follow-up gaps" stay deferred:

- `grex doctor --prune-quarantine` GC verb (v1.3 candidate)
- `grex doctor --restore-quarantine` recovery verb (v1.3 candidate)
- Dedicated `TreeError::QuarantineFailed` variant (MINOR-bump candidate)
- cap-std bounded recursive copy for snapshot read TOCTOU hardening (v1.3 candidate)
- `--workspace → --pack` flag rename (v1.3.0 deprecation, v1.3.1 removal)
- Stale `grex-doc/src/concepts/manifest.md` doc-debt sweep (separate doc PR)
- mdbook doc-debt sweep + rayon parallel scheduler tuning + CLI migrate-lockfile dispatcher (separate v1.2.x slices)
- SSOT side files not yet committed (`force-prune.md`, `toctou.md`, AuditKind/quarantine doc, `snapshot_recursive` axiom migration to `Bridge.lean`)

No architecture change, no rename, no refactor. v1.2.3 is bug fixes only.

## Acceptance bar

1. Lean4 theorem `Grex.Walker.sync_meta_no_cycle_infinite_clone` extended in `proof/Grex/Walker.lean` to cover (a) bounded recursion (depth cap) and (b) non-empty initial visited (root-identity seed). `lake build` green; zero `sorry`, zero `admit`. **No new axiom.** Bridge axiom counts unchanged: 9 in `Bridge.lean`, 4 in `Types.lean` (CI gate at `.github/workflows/ci.yml:218-265`).
2. Three new unit tests pass under `walker::tests`: `diamond_no_cycle`, `cycle_four_node_aborts`, `cycle_nested_prefix_aborts`.
3. The v1.2.2 unit tests (`cycle_self_loop_aborts`, `cycle_three_node_aborts`, `two_refs_same_url_not_cycle`) and `e2e_cycle_aborts` continue to pass.
4. B1 coverage: a cyclic manifest with cycle length > `max_depth` surfaces `Err(CycleDetected)` (NOT silent truncation). New unit test `cycle_under_depth_cap_still_aborts` asserts this.
5. B2 coverage: `pack_identity_for_child` on a `ChildRef` with `r#ref: None` renders `url:<url>` (no trailing `@`). New unit test `identity_omits_trailing_at_when_ref_empty` asserts this.
6. B4 coverage: a `CycleDetected` chain from a root-level cycle includes the root pack identity as its first element. Asserted via the existing v1.2.2 `cycle_self_loop_aborts` test (extended to verify chain[0] is `path:<root>`).
7. Local gates clean: `cargo fmt --all -- --check`, `cargo doc --no-deps --workspace -D warnings`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cd proof && lake build`, axiom-policy check.
8. Workspace version bumped 1.2.2 → 1.2.3 in workspace root `Cargo.toml`, `crates/xtask/Cargo.toml` path-dep pin, and `crates/xtask/tests/version_test.rs` `EXPECTED_WORKSPACE_VERSION`.
9. Man pages regenerated via `cargo xtask gen-man`.
10. CHANGELOG `[1.2.3]` entry + SSOT `.omne/cfg/history.md` v1.2.3 entry (separate SSOT repo per Rule 7).

## SemVer

PATCH (1.2.2 → 1.2.3). Bug fixes only; no public API change. `TreeError::CycleDetected` shape is unchanged. `pack_identity_for_child` is a private helper (`pub(crate)` boundary); its output format change in B2 is observable only inside the crate. Identity strings appear in the `chain` field of `TreeError::CycleDetected` user-facing output, but the change there is purely cosmetic (drops a trailing `@`).

The B4 root-identity seed changes the OBSERVABLE chain shape — operators currently parsing chain strings will see `path:<root>` as a new prefix element. This is a diagnostic improvement, not a contract change; `chain` is documented as "ordered chain of pack identities that forms the cycle" (`error.rs:48`) without a fixed-length guarantee. Maintainer call.

## Process gates (per Rule 8)

Order of operations is fixed: Lean theorem extension first, Rust impl second.

1. Extend `proof/Grex/Walker.lean` model + theorem to cover bounded recursion (B1) and non-empty initial visited (B4).
2. `lake build` green; zero `sorry`, zero `admit`.
3. THEN modify Rust code in `walker.rs`.

A code change that lands before the proof is a process violation per `.omne/schemas/rules.md` Rule 8.
