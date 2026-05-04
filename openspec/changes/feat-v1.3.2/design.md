---
slug: feat-v1-3-2-design
type: spec
status: active
last_updated: 2026-05-03
topic: v1-3-2-patch
depends_on: [feat-v1-3-2, dogfood-findings-v1-3-0, lockfile, manifest, walker, pack-spec, freeze-v1-3-0]
---

# v1.3.2 — design

**Status**: active
**Spec**: [`proposal.md`](./proposal.md) · [`tasks.md`](./tasks.md)
**SSOT**: `.omne/dogfood-findings-v1-3-0.md` (bug catalogue) · `.omne/lockfile.md` §"File location" + §"Three lock artifacts" · `.omne/manifest.md` §"events.jsonl event schemas" · `.omne/walker.md` §"Walker model" + §"Untracked git policy" + §"`LockEntry.synthetic` deprecation" · `.omne/pack-spec.md` §"v1.2.0 — declarative nested paths (option c)" · `.omne/concurrency.md` §"Five cooperating mechanisms" · `.omne/var/freeze-v1.3.0.md`

## Why

Three v1.3.0 dogfood bugs cluster around **runtime-vs-SSOT contract drift** — the SSOT design docs already define the correct behavior; the v1.3.0 runtime ships behavior that disagrees. v1.3.2 closes the gap. Selection criteria for the bundle:

- All three are pure runtime catch-up (no contract change). Frozen contracts in `freeze-v1.3.0.md` remain intact.
- Write-sets are disjoint at file granularity → 3 parallel workers per rule 14.
- Two are simple-exempt (B6 subtractive serde, B11 path-string change). One (B13) needs a Lean theorem before any Rust change per rule 8 — Phase 2a is the gate.
- Bundling them in one PATCH ship matches v1.3.1's "critical bundle" ship pattern — minimum coordination overhead, max regression coverage.

## Architectural context

**Bug origins** (from `.omne/dogfood-findings-v1-3-0.md` and grep over `.omne/`):

- **B6:** lockfile writer still serializes `LockEntry.synthetic` field for v1.2.0+ entries. Per `walker.md §LockEntry.synthetic deprecation` and `pack-spec.md §v1.2.0 — Synthesis policy — RETIRED in v1.2.0`, the field is retired but kept on the struct for backward-compat reads (v1.1.x lockfiles continue to deserialize). The writer should never emit `synthetic: true` under v1.2.0+. Dogfood found `synthetic: true` on 6/7 children — direct contradiction of the SSOT.
- **B11:** three lock artifacts (per-pack pack-lock, workspace sync-lock, per-repo backend-lock) land at workspace / pack root instead of inside `.grex/`. Per `lockfile.md §File location` and `manifest.md`, both stateful files (`grex.lock.jsonl`, `events.jsonl`) live under `<meta>/.grex/`. Per `concurrency.md §Five cooperating mechanisms`, the file mutexes do not currently follow the same rule — that is the dogfood finding. The maintainer locked the v1.3.2 fix as a hard-cut path move.
- **B13:** walker / manifest loader rejects `child.path` containing `/`. Per `pack-spec.md §v1.2.0 — declarative nested paths (option c)`, slash paths are SHIPPED behavior since v1.2.0; the runtime is BEHIND the spec. Loader gate + walker normalisation must accept.

**Frozen contract status (re-checked against `freeze-v1.3.0.md`):**

| Frozen surface | v1.3.2 status |
|---|---|
| Lockfile v1.2.0 schema (FROZEN) | INTACT. B6 removes a deprecated optional field that v1.2.0 already retired; `#[serde(default)]` keeps reads of legacy lockfiles round-trip clean. **No `schema_version` bump.** |
| `sync` exit codes (FROZEN) | INTACT. No exit-code change. |
| `sync` JSON envelope shape (FROZEN, additive ok) | INTACT. No envelope change. |
| `Event` enum + `Unknown` fallback (FROZEN-FORWARD-COMPAT) | INTACT. No new event variant; B11 doesn't touch event log; B6/B13 don't touch event variants. |
| `ExecCtx::workspace` (FROZEN) | INTACT. |
| Quarantine retention default (FROZEN) | INTACT. |
| `--workspace` flag (DEPRECATED) | UNCHANGED. v1.4.0 removal still on track. |
| `workspace` JSON envelope key (DEPRECATED) | UNCHANGED. |
| `SyncParams::workspace` MCP (DEPRECATED) | UNCHANGED. |
| `PackLock::acquire` sync, `Scheduler::permits`, `DEFAULT_MANAGED_GITIGNORE_PATTERNS` (DEPRECATED-DEFERRED) | UNCHANGED. |
| Plugin-API trait surface (UNSTABLE) | UNCHANGED. |
| MCP method shapes | UNCHANGED. |
| `grex doctor --json` envelope | UNCHANGED (B11 may add an advisory but is deferred to v1.3.3 — out-of-scope). |

**In-bounds path move precedent.** The v1.3.1 ship classified two changes as "in-bounds that look like freeze touches but are not" (`freeze-v1.3.0.md §"In-bounds changes that look like freeze touches but are not"`): SCHEMA_VERSION 1→2 hard-cut and lockfile `branch` carry. v1.3.2's B11 follows the same precedent — lockfile schema FROZEN at v1.2.0 shape; **location** is not in the freeze table.

## File map (3 bugs, disjoint write-sets per rule 14)

For each worker, list files to TOUCH (relative to repo root). Disjoint = no two workers write the same file. **All paths below are best-effort sketches — confirm during Phase 2 walk against the actual crate layout.**

### W1 — B6 retire `LockEntry.synthetic` writer

- `crates/grex-core/src/lockfile/entry.rs` (or `crates/grex-core/src/lockfile/mod.rs`) — drop `synthetic: bool` from the writer payload. Annotate the struct field `#[serde(default, skip_serializing_if = "is_false")]` (additive serde-only; no struct-level breaking change).
- `crates/grex-core/src/lockfile/writer.rs` — any callsite that constructs `LockEntry { synthetic: ..}` drops the field assignment. Confirm during Phase 2 walk; B14's earlier B14 fix touched this same file but the `synthetic` write may live elsewhere.
- (Reader side) — if any reader expects `synthetic == true` to drive a code branch, switch to `synthetic: bool` (default false) consumption + remove the read site (paired with the walker's already-retired synthesis branch). Confirm during Phase 2; per `walker.md §LockEntry.synthetic deprecation` the field "remains on the struct for backward-compat reads" — implies no live reader; verify.
- New test: `crates/grex-core/tests/lockfile_no_synthetic.rs` — assert serialized lockfile JSON for any v1.2.0+ sync output contains NO `"synthetic"` key. Round-trip test asserts a v1.1.x lockfile with `synthetic: true` deserializes cleanly (forward read preserved).

Touchpoint sketch:
```rust
// crates/grex-core/src/lockfile/entry.rs
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LockEntry {
    pub id: String,
    pub sha: String,
    pub branch: String,
    pub installed_at: String,
    pub actions_hash: String,
    pub schema_version: String,
    // RETIRED: synthetic. Read-tolerated via #[serde(default)] for v1.1.x lockfiles.
    #[serde(default, skip_serializing_if = "<helper>")]
    pub synthetic: bool,
    pub path: Option<String>,
}
```

If the v1.2.0+ writer never sets `synthetic = true`, the `skip_serializing_if` predicate effectively elides the field on every emit. Alternative: remove the field from `LockEntry` entirely and rely on a v1.1.x compat shim deserializer that ignores unknown keys — confirm with maintainer before Phase 2.

### W2 — B11 lockfile-location migration

Three artifacts move under `.grex/`:

| Artifact | Old path | New path | Module |
|---|---|---|---|
| Per-pack pack-lock | `<pack_workdir>/.grex-lock` | `<pack_workdir>/.grex/.grex-lock` | `crates/grex-core/src/concurrency/packlock.rs` (best-effort; confirm Phase 2 — possibly `crates/grex-core/src/fs/lock.rs`) |
| Workspace sync sidecar | `<workspace>/.grex.sync.lock` | `<workspace>/.grex/.grex.sync.lock` | `crates/grex-core/src/sync/workspace_lock.rs` (best-effort; v1.3.1 B4 dry-run gate already touched `open_workspace_lock`) |
| Per-repo backend lock | `<dest>.grex-backend.lock` (sibling, v1.3.0) | `<parent_meta>/.grex/locks/<child-path>.backend.lock` | `crates/grex-core/src/backend/git/lock.rs` (best-effort; confirm Phase 2) |

**Per-repo backend lock — design rationale (current truth):**
The parent meta-pack manages child clone operations (parent declares child in manifest, parent invokes the clone). The backend lock therefore lives in the parent's `.grex/` namespace, not adjacent to or inside the child dest. Path: `<parent_meta>/.grex/locks/<child-path>.backend.lock` where `<child-path>` is the manifest-declared path verbatim (e.g. `tools/foo` → `<parent>/.grex/locks/tools/foo.backend.lock`). Intermediate directories auto-created on first lock acquisition. Properties:

- Persists regardless of dest state — survives `rm -rf <dest>` (manual or cooperative).
- Pre-clone safe — parent's `.grex/` exists before any child clone (chicken-egg fixed).
- Path-keyed identity per `manifest.md §v1.2.0 keying` — no collision when two children share `name:` at distinct paths.
- Centralized — `ls .grex/locks/` shows all backend-lock state in a tree mirroring the dest structure.
- No slug encoding required — filesystem natively supports nested paths.

Affected files (best-effort; confirm during Phase 2 walk):
- `crates/grex-core/src/concurrency/packlock.rs` — per-pack lock-path constant or builder (unchanged target: `<pack_workdir>/.grex/.grex-lock`).
- `crates/grex-core/src/sync/workspace_lock.rs` — workspace sidecar lock-path (unchanged target: `<workspace>/.grex/.grex.sync.lock`).
- `crates/grex-core/src/backend/git/lock.rs` — per-repo backend lock-path; rewrite to compute `<parent_meta>/.grex/locks/<child-path>.backend.lock` from the child's manifest-declared path. Acquisition site must `mkdir -p` intermediate dirs.
- `crates/grex-core/src/doctor/findings.rs` — if `grex doctor` reports lock paths in any finding (e.g. stale-lock detection per `cli.md` "stale `.grex-lock` files"), update path strings + add stale-lock scan under `<parent>/.grex/locks/`.
- `crates/grex/src/cli/verbs/doctor.rs` — same as above if rendered in CLI output.
- New test: `crates/grex-core/tests/lockfile_under_grex.rs` — drives a sync, asserts (a) per-pack lock at `<pack_workdir>/.grex/.grex-lock`, (b) workspace lock at `<workspace>/.grex/.grex.sync.lock`, (c) per-repo backend lock at `<parent_meta>/.grex/locks/<child-path>.backend.lock`. Negative test asserts NO sibling `<dest>.grex-backend.lock`, NO inside-dest `<dest>/.grex/.grex-backend.lock`, NO workspace-root or pack-root paths.

**Hard-cut readers.** No fallback to old workspace-root path — the open path takes the new location ONLY. Justification: v1.3.1 B8 SCHEMA_VERSION hard-cut precedent (no field deployments to migrate). The maintainer locked this posture 2026-05-03.

**Operator migration recipe** (one-liner, surface in `migration-v1.3.2.md` per rule 16):

```bash
# At each meta workspace root:
mkdir -p .grex
mv .grex.sync.lock .grex/ 2>/dev/null
# At each pack workdir (only if grex sync was previously interrupted):
test -f .grex-lock && mkdir -p .grex && mv .grex-lock .grex/
# At each clone dest (only stale orphans):
find . -maxdepth 2 -name '*.grex-backend.lock' -exec rm -v {} +
```

Operators with a clean tree (no in-flight sync, no stale orphan lock files) need no action — the new sync writes to the new path on first invocation.

**Open question to maintainer:** retain the bare `.grex-lock` / `.grex.sync.lock` / `.grex-backend.lock` filenames inside `.grex/`, or drop the leading dot (since they no longer need to hide at workspace root)? Proposal: retain bare names for diff minimality and visual hint (the leading dot still signals "internal lock file" to operators inspecting `.grex/` directly).

**Open question to maintainer:** the per-repo backend lock currently sits as a SIBLING file (`<dest>.grex-backend.lock` adjacent to `<dest>`) so the lock survives a `<dest>` wipe. Moving inside `<dest>/.grex/` means the lock disappears if the dest is destructively wiped. Proposal: accept, because v1.2.5 quarantine semantics + v1.3.1 dry-run gates make destructive wipes cooperative. Confirm before Phase 2c.

### W3 — B13 walker / manifest loader slash-path support

Walker normalisation flow (per `pack-spec.md §v1.2.0 — declarative nested paths (option c)` + `walker.md §Walker model`):

1. **Manifest loader gate.** Current loader rejects any `child.path` containing `/`. Replace with per-segment validation: split on `/`, validate each segment against the bare-name regex `^[a-z][a-z0-9-]*$`, reject `..`, leading `..`, absolute paths, escaping symlinks (already-checked invariants — preserve).
2. **Walker dest resolution.** Per `pack-spec.md` and `walker.md §Walker model`: `dest = current_meta.join(<parent-relative-path>)` — parent-relative join, each recursion frame uses its own meta as anchor. Walker MUST canonicalise the resolved path back inside `current_meta` (rejection of escape via `..` and symlinks already required).
3. **Lockfile keying.** Per `manifest.md §v1.2.0 — keying under nested child paths (decided: path-keyed)`, lockfile entries key by **meta-relative POSIX path** (forward-slash separator, normalized at write-time). Slash paths land in the same key shape — bare-name children keep their existing keys; `tools/foo` keys as `"tools/foo"`.
4. **Cycle-detection visited set.** Per `walker.md` Lean theorem `sync_meta_no_cycle_infinite_clone`, the walker tracks a `visited: Vec<String>` (or analogous) of pack identities. Slash-path ids must participate cleanly — the `String` already carries `/` characters; verify no callsite assumes bare names.

**Maintainer-locked invariant (2026-05-03):** *"the walker never recurses into a folder lacking `.grex/`"* — i.e. an unmanaged subdir terminates descent. This is the safety boundary that prevents infinite descent under slash paths AND under any disk-driven traversal. Walker is **manifest-graph-driven**, not filesystem-driven (per `walker.md §Architectural orientation`); a slash path that resolves to `tools/foo` STILL terminates at any intermediate folder lacking `.grex/`. The walker does not auto-discover undeclared nested git repos at intermediate folders. Encode this invariant explicitly in the B13 Lean theorem.

Affected files (best-effort; confirm during Phase 2 walk):
- `crates/grex-core/src/manifest/loader.rs` (or `crates/grex-core/src/pack/manifest.rs`) — slash-path validation gate.
- `crates/grex-core/src/tree/walker.rs` — Phase 1 dest resolution; Phase 3 child entry; cycle-detection `visited` if it uses path-strings (else bare names — confirm).
- `crates/grex-core/src/tree/graph_build.rs` (or wherever `PackNode` is constructed) — propagate slash-path through to lockfile keying.
- New test: `crates/grex-core/tests/slash_paths.rs` — feed a manifest with `path: tools/foo` + `path: courses/cpp/cpp-grammar`. Assert: walker resolves dest, manifest loader accepts, lockfile keys by slash-path, intermediate `.grex`-less folder terminates descent.
- Negative test: `..`, absolute paths, symlink-escapes still reject (preserve existing tests).

Touchpoint sketch:
```rust
// crates/grex-core/src/manifest/loader.rs (illustrative)
fn validate_child_path(p: &str) -> Result<()> {
    if p.is_empty() || p.starts_with('/') || p.contains('\\') {
        return Err(InvalidPath::AbsoluteOrBackslash);
    }
    for segment in p.split('/') {
        if segment.is_empty() || segment == ".." || segment == "." {
            return Err(InvalidPath::EscapeOrEmpty);
        }
        if !BARE_NAME_REGEX.is_match(segment) {
            return Err(InvalidPath::InvalidSegment);
        }
    }
    Ok(())
}
```

## B13 Lean obligation (REQUIRED, rule 8 gate)

Phase 2a deliverable. Phase 1 (this OpenSpec) states the obligation; the theorem MUST `lake build` green BEFORE any Rust change in W3 lands.

### Candidate name + signature

Three candidate names. The maintainer ratifies the final form before Phase 2a Lean dispatch.

**Candidate A — `Grex.Walker.slash_path_walker_terminates`** (preferred):

```lean
-- Walker terminates on slash-paths because every recursion frame either (a) reaches
-- a registered meta (.grex/pack.yaml present) or (b) hits an unmanaged subdir
-- (.grex/ absent) and STOPS. No infinite descent.
theorem slash_path_walker_terminates
    (m : Manifest) (ctx : Ctx)
    (h_paths_valid : ∀ c ∈ m.children, slash_segments_bare_name c.path) :
    ∃ steps : Nat, sync_meta_inner_model m ctx terminates_in steps
```

**Candidate B — `Grex.Walker.slash_path_id_canonical`**:

```lean
-- Slash-path ids participate in cycle detection: distinct paths produce distinct
-- ids; identical paths under one parent meta are forbidden by the manifest validator.
theorem slash_path_id_canonical
    (m : Manifest)
    (h_paths_valid : ∀ c ∈ m.children, slash_segments_bare_name c.path) :
    ∀ c1 c2 ∈ m.children, c1.path = c2.path → c1.id = c2.id
```

**Candidate C — extend `Grex.Walker.sync_meta_no_cycle_infinite_clone`** over a richer `id` space:

```lean
-- Generalise the existing cycle-no-infinite-clone theorem so the visited set
-- tolerates ids containing `/` characters. Same invariant, broader id alphabet.
theorem sync_meta_no_cycle_infinite_clone_with_slash_ids
    (m : Manifest) (visited : List String)
    (h_paths_valid : ∀ c ∈ m.children, slash_segments_bare_name c.path) :
    sync_meta_inner_model_terminates m visited
```

**Recommended:** Candidate A (`slash_path_walker_terminates`) — cleanest capture of the maintainer's "walker never recurses into a folder lacking `.grex/`" invariant + the slash-path termination property in one theorem. Candidate C is a fallback if extending the existing theorem is cheaper than introducing a new one. Candidate B is narrowest (id canonicalisation only) and may not capture the safety boundary — preferred only if the maintainer wants a separate, smaller obligation.

**Bridge axioms:** none expected (existing `axiom rust_walker_matches_model` covers the link). Axiom budget unchanged at **9 bridge / 4 types / 0 model**.

**Proof strategy (Candidate A):** strong induction over the depth of the manifest graph. Base case: empty `children` list, trivial termination. Step case: each child either (a) has a `.grex/pack.yaml` and recursion proceeds with strictly smaller depth, or (b) lacks `.grex/` and recursion stops at this frame (no further descent). The maintainer-locked unmanaged-subdir invariant is the inductive cut.

CI axiom-stability gate extends from **7 theorems** (post-v1.3.1) to **8 theorems** post-v1.3.2. Axiom audit asserts theorem #8 lands on `[propext]` only or no axioms.

## Cross-cuts (any worker touches; resolve at merge)

- `crates/grex-cli/Cargo.toml`, `crates/grex-core/Cargo.toml`, `crates/grex-mcp/Cargo.toml`, `crates/grex-plugins-builtin/Cargo.toml` — bump version `1.3.1` → `1.3.2`. Apply to all 4 crates.
- `Cargo.lock` — regen via `cargo update -w`.
- Any rustdoc on lockfile writer / per-pack-lock module mentioning the old path → update inline.
- `xtask man-drift` may regenerate man pages if any path string surfaces in CLI help; confirm Phase 3.

## Reviewer checks (Phase 2R)

cavecrew-reviewer + code-reviewer parallel pass on full diff. Required findings:

1. Public API surface drift = 0 (PATCH-additive). Compare exported symbols in `grex-core::lib.rs` and `grex-cli::lib.rs` pre/post.
2. No `Co-Authored-By` in any commit message or PR body (rule 13). No AI/Claude/Anthropic mentions.
3. Frozen contract violations (per `freeze-v1.3.0.md`) = 0. Cross-check the table at top of this design.
4. Worker desync: B6 / B11 / B13 write-sets verified disjoint at file granularity. Single agent reviewer pass over all 3 worker outputs to catch desynchronization (rule 14).
5. Test coverage delta: each worker adds at least one new test file; cumulative 3+ new test files. `slash_paths.rs` covers both positive (slash accepts) and negative (escape rejects).
6. clippy/rustfmt deltas: zero new lints.
7. Rule 8 sanity: B13 Lean theorem compiles, axiom budget 9/4/0 unchanged. B6/B11 simple-exempt justifications match scope (no algorithm impact).
8. Hard-cut B11 path move: NO fallback open-path code in any of the 3 lock writers. Reviewer audits the open path and confirms only the new path is attempted.

## Validation gate (Phase 3)

```
cargo fmt --check --all
cargo build --workspace --all-targets
cargo build --workspace --all-targets --release
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
cargo doc --workspace --no-deps
lake build  (cwd = proof/)
python .omne/scripts/validate.py
cargo run -p xtask -- gen-man  (man-drift --check)
cargo run -p xtask -- axiom-audit  (asserts 8 theorems on [propext]/no-axioms)
```

All exit 0 required. Carry-forward env-only fails (`dispatch_parallel`, `pack_type_dispatch` Windows UAC os err 740) acceptable per v1.3.1 precedent if documented in PR body.

## Real-smoke regression contract (Phase 4)

Real-smoke harness baseline UNCHANGED at 2 pass / 13 fail until the `gix` HTTPS feature gap closes (separate v1.3.x infra item). v1.3.2 verification relies on **in-process tests** added in W1 / W2 / W3 — same posture as v1.3.1.

When the smoke harness lands HTTPS support, the v1.3.2 fixture deltas would target slash-path fixtures (B13) + lock-location fixtures (B11). Out-of-scope for this ship.

## Open risks

- **Real-smoke gix HTTPS gap unchanged.** In-process tests are the regression lock. Logged as orthogonal infra item per v1.3.1 endpoint.
- **B11 stale-lockfile orphans.** Operators upgrading from v1.3.0/1.3.1 may have stale lock files at old locations. Mitigation: migration recipe in `migration-v1.3.2.md` (Phase 5 / rule 16 SSOT update). Doctor advisory finding is a candidate for v1.3.3 (deferred, scope-tight here).
- **B11 backend-lock inside-`<dest>` placement.** Discussed above; awaiting maintainer ratification.
- **B13 cycle-detection edge case.** Slash-path ids must not collide between distinct children. Path-keyed lockfile (per `manifest.md §v1.2.0 keying`) already disambiguates; the B13 Lean theorem must formalise. Reviewer check #7 validates.
- **B6 hidden reader.** If any v1.2.0+ code path still consumes `synthetic`, B6 must remove that read site too. Phase 2 walk MUST grep for `synthetic` across `crates/grex-core/` before W1 lands.
- **W1/W2/W3 file-share risk.** `lockfile/writer.rs` could appear in both W1 (B6 field removal) and W2 (B11 path move) unless the path-string lives in a separate module. Phase 2 walk confirms; if shared, bundle into one worker per rule 14.

## Risk register summary

| Risk | Mitigation | Owner |
|---|---|---|
| Real-smoke gix HTTPS gap | In-process tests; deferred infra | v1.3.x infra lane |
| Stale lock orphans | Migration recipe + (optional) v1.3.3 doctor advisory | Phase 5 SSOT update |
| Backend-lock placement | Maintainer ratification before Phase 2c | Maintainer |
| Slash-path id collision | B13 Lean theorem | Phase 2a Lean worker |
| Hidden `synthetic` reader | Phase 2 grep before W1 lands | W1 worker |
| Worker file-share | Phase 2 walk; bundle if shared | Dispatcher |
