---
slug: feat-v-1-3-3
type: spec
status: active
last_updated: 2026-05-07
topic: ux-polish-bundle
depends_on: [feat-ssot-mount-rename]
---

# feat-v1.3.3 — UX polish bundle (B3 + B5 + B10)

## § Why

Three v1.3.x backlog bugs all live on the user-ergonomics axis and share a single release window:

- **B3** — `grex sync --pack .` (and `--workspace .`) currently rejects `.` as a path argument. Users in a pack directory must spell out the absolute or relative path, breaking flow.
- **B5** — `grex doctor` does not surface `.gitignore`-drift between parent repo and pack contents. Users discover untracked-pack-content drift only on next `git status` surprise.
- **B10** — `grex add <url>` cannot pin a git ref at registration time. Users must run `add`, then hand-edit the manifest `ref:` field, then run `sync` to actually pull the pinned commit. Three steps for a one-step intent.

All three are additive UX fixes. No public-API contract drift. v1.3.x dogfood feedback identified all three; v1.3.3 retires the UX-axis subset of the v1.3.x backlog.

## § Proposal

Ship three behavior changes inside one release.

### B3 — `--pack .` / `--workspace .` cwd shorthand

`.` resolves to **process cwd** (per Q2 — no walk-up). Argument parsing in `crates/grex/src/cli/` accepts `.` as a literal cwd alias, then dispatches the same code path as an explicit path. If cwd is not a valid pack root, surface the existing "not a pack root" error with the resolved cwd in the message.

### B5 — `grex doctor` `.gitignore`-aware drift check

Per Q3: warn-only (info severity, never fails doctor). Detect parent-repo `.gitignore` rules that do NOT correctly track pack content (i.e. the pack lives somewhere the parent gitignore wants to ignore, OR the pack lives somewhere not covered by ignore rules but should be per project convention). At end of `grex doctor` run, emit a summary block prompting the user to add the pack to `.gitignore` (or remove an over-broad rule). Other doctor checks unchanged.

### B10 — `grex add --ref <git-ref>` flag + folder FA + Lean4 obligations

`grex add` accepts `--ref <git-ref>` flag. Syntax per OQ1:
- `--ref main` (branch only)
- `--ref a3f9c1d` (commit only, 7-char min)
- `--ref main@a3f9c1d` (branch + commit, `@` delimiter)
- omitted (default to `main` branch HEAD)

Folder layout:
```
<parent-pack>/
  <reponame>/
    <refdir>/
```

`<refdir>` resolved by an 8-cell folder-FA over `(B, C, U)` Boolean triple (B=branch present, C=commit present, U=URL already tracked). Full transition table in `design.md §B10`. Per OQ4, FA encoded in Lean4 in `proof/` folder. Per OQ5, 7-char SHA + add-time uniqueness check, extend prefix on collision. Three theorems ship in v1.3.3:

1. `ref_fa_total` — folder FA total over the 8-cell domain.
2. `ref_folder_injective` — distinct add-action inputs yield distinct refdirs within same `<reponame>/`.
3. `dup_safe` — reject-action inputs cause zero FS mutation and zero manifest mutation.

**Implementation order: Lean4 proofs land BEFORE B3/B5/B10 code.** Locks contracts before the executable surface lands.

## § Out of scope

- **B1, B9, B15** — deferred to v1.3.4 cleanup bundle.
- **gix HTTPS feature** — deferred to v1.4.0 (per Q5; blocks 13 real-smoke fixtures, too large for v1.3.3 patch window).
- **`grex sync` ref-advance semantics for branch checkouts** — orthogonal to B10 add-time pinning. Defined elsewhere; not redefined here.
- **`--branch` / `--commit` separate flags** (OQ1 alternative) — single `--ref` flag with `@` delimiter chosen.
- **`detached/` namespace folder prefix** (OQ2 / OQ3 alternative) — dissolved; cells 5/6 use `main@<commit-short>`.

## § Risks

- **B3 cwd ambiguity.** Users running `grex sync --pack .` from a non-pack directory get an error message that points at cwd. Mitigation: error message resolves `.` to the absolute cwd path so the surprise is visible.
- **B5 false positives.** `.gitignore` drift detection may misclassify intentionally-ignored packs (e.g. CI-only mirrors). Mitigation: warn-only severity (per Q3); summary prompt asks user to confirm intent rather than asserting wrongness.
- **B10 7-char SHA collision in large repos.** Collisions possible (per OQ5). Mitigation: add-time uniqueness check extends prefix until unique; manifest persists the extended SHA. Lean theorem 2 (`ref_folder_injective`) formalizes injectivity post-extension.
- **B10 manifest schema interaction.** `--ref` writes `ref:` field at registration; existing `add`-then-edit users see no breaking change because manifest field name + format are unchanged. Pre-existing manifests with `ref:` already set are honored on re-add (cells 4, 6, 8 dedup branch).
- **Lean4-first ordering risk.** If theorems reveal the FA design is unsound, B10 code rework lands inside the same release window. Mitigation: 8-cell table is small + finite — exhaustive case analysis tractable.

## § SemVer verdict

**PATCH bump v1.3.2 → v1.3.3.** Additive UX, no contract drift. New `--ref` flag is purely additive (omitted = pre-v1.3.3 behavior preserved). `--pack .` shorthand is purely additive (existing path arguments unchanged). `grex doctor` summary prompt is additive (existing doctor checks + exit code unchanged in non-drift case; warn-only severity in drift case so doctor still exits 0). 4 published crates retain their v1.3.x compatibility surface.

## § Acceptance criteria

1. **B3** — `grex sync --pack .` and `grex sync --workspace .` resolve `.` to process cwd. No walk-up. If cwd is not a pack root, error message includes resolved absolute cwd. Pre-existing path argument behavior unchanged.
2. **B5** — `grex doctor` emits a summary block at end of run when parent `.gitignore` does not track pack content correctly. Severity = warn (info). Doctor exits 0 in drift-detected case. Summary prompt instructs user on next step (add pack to `.gitignore` or remove over-broad rule).
3. **B10 — flag + FA** — `grex add --ref <ref>` accepts the four syntactic forms (branch, commit, `branch@commit`, omitted). Folder layout `<parent-pack>/<reponame>/<refdir>/` produced per the 8-cell FA in `design.md §B10`. `<reponame>` strips `.git` suffix. `<branch>` slashes → underscores. `<commit-short>` = 7-char min, extended on collision per OQ5.
4. **B10 — manifest** — registration writes resolved commit SHA into manifest `ref:` field. No floating refs persisted. Re-add of identical `(url, ref)` is idempotent (silent reject, exit 0). Re-add producing collision warns + rejects (exit 1) or warns + adds sibling per FA cells 4, 6, 8.
5. **B10 — Lean4** — `proof/Grex/RefFa.lean` (or equivalent path under `proof/`) builds clean. `ref_fa_total`, `ref_folder_injective`, `dup_safe` all green. `lake build` exits 0. Axiom budget unchanged from v1.3.2 baseline (or budget delta documented in `inst/cfg/proof/`).
6. **B10 — implementation order** — git history shows Lean4 commits land before B3/B5/B10 Rust commits on the branch.
7. **Test suite** — `cargo test --workspace` exits 0. Existing 1009 tests stay green; new tests for B3/B5/B10 added under appropriate `crates/*/tests/`.
8. **Real-smoke** — v1.3.x real-smoke harness stays green. No new fixtures required (gix HTTPS deferred per Q5).
9. **Manual smoke** — three operator scenarios pass:
   - `cd <pack-dir> && grex sync --pack .` — resolves correctly.
   - `cd <parent-with-ignored-pack> && grex doctor` — emits drift summary.
   - `grex add <url> --ref main@a3f9c1d` — produces `<reponame>/main@a3f9c1d/` folder with manifest entry.
10. **Doc + changelog** — `CHANGELOG.md` entry under `[1.3.3]` lists B3 + B5 + B10. Bumped Cargo.toml versions for all 4 published crates.
