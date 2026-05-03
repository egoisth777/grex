---
slug: feat-v1-3-1-design
type: spec
status: active
last_updated: 2026-05-02
topic: cli
depends_on: [feat-v1-3-1, dogfood-findings-v1-3-0, walker, lockfile, manifest, cli]
---

# v1.3.1 — design

**Status**: active
**Spec**: [`proposal.md`](./proposal.md) · [`tasks.md`](./tasks.md)
**SSOT**: `.omne/cfg/dogfood-findings-v1-3-0.md` (bug catalogue) · `.omne/cfg/walker.md` §"dry-run semantics" · `.omne/cfg/lockfile.md` §"branch field" · `.omne/cfg/manifest.md` §"events schema v2" · `.omne/cfg/cli.md` §"cwd default" · `.omne/cfg/doctor.md` §"advisory findings"

## Why

v1.3.0 cut the `--pack` alias + behavior-contract freeze. Real-smoke harness (PR #67) caught 14 dogfood regressions; 8 are flagged as PATCH-additive against the frozen contract. v1.3.1 ships 6 of those (B2, B4, B7, B8, B12, B14) — the disjoint subset that Lean coverage + reviewer parallel-pass can clear in one cycle. The remaining two (B1, B5) need v1.4.x scope and stay out of this patch.

Selection criteria for the 6:
- Each bug is producer-local (single primary module + one test file).
- Six write-sets are disjoint at file granularity → six parallel workers per rule 14.
- B4 + B8 share `crates/grex-core/src/audit/events.rs` → bundled into a single W2+W4 worker (see §"File map").
- Two algorithm-impacting changes (B4 dry-run gate, B14 lockfile branch carry) gated by Rule 8 Lean theorems — already drafted, axiom budget unchanged.

## Architectural context

**Bug origins** (from `.omne/cfg/dogfood-findings-v1-3-0.md`):
- B2: `crates/grex/src/cli/verbs/sync.rs:42` — `--pack` arg defaults to `None`, errors out before checking cwd.
- B4: `crates/grex-core/src/tree/walker.rs:Phase 3` — `ctx.dry_run` consulted only for top-level emit, not for child clone subprocess nor `.gitignore` mutation nor lockfile write.
- B7: `crates/grex/src/cli/main.rs` tracing subscriber writes via default (`stdout`); `Op` enum lacks `Display`, so trace lines render `op=Discriminant(3)`.
- B8: `crates/grex-core/src/audit/events.rs` emits schema v1 with legacy `pack` field; `id` field absent on action_started/completed.
- B12: walker auto-writes `<parent>/.gitignore` to add the pack folder. Side-effect on parent repo, no consent.
- B14: `LockEntry::from_resolved` writes `branch: String::new()` regardless of manifest `ref:` value.

**Frozen contract status**: All 6 fixes are PATCH-additive per `.omne/cfg/freeze-v1.3.0.md`:
- B2: behavior addition (cwd default) — no existing invocation breaks.
- B4: bug fix — dry-run was always documented as side-effect-free.
- B7: bug fix — stdout pollution was never contractual.
- B8: schema_version bump from 1 → 2 — readers MUST consult schema_version field per v1.3.0 freeze. Acceptable break per maintainer 2026-05-02.
- B12: removal of undocumented mutation — no test/doc ever exercised `.gitignore` write.
- B14: bug fix — empty branch field was never the documented behavior.

## File map (6 bugs, disjoint write-sets per rule 14)

For each worker, list files to TOUCH (relative to repo root). Disjoint = no two workers write same file. After the W2/W4 bundle merge, 5 workers ship in parallel.

### W1 — B2 cwd default pack root
- `crates/grex/src/cli/verbs/sync.rs` — when `pack_root` arg absent, check `cwd/.grex/pack.yaml` exists, set `pack_root = cwd`, else fall through to existing error.
- `crates/grex/tests/cli_cwd_default.rs` — new test file. Cases: (1) cwd has `.grex/pack.yaml` → defaults; (2) cwd lacks it → existing error preserved; (3) explicit `--pack /tmp/x` overrides cwd-default.

Touchpoint sketch:
```rust
// crates/grex/src/cli/verbs/sync.rs (additive)
let pack_root = match args.pack_root {
    Some(p) => p,
    None => {
        let cwd = std::env::current_dir()?;
        if cwd.join(".grex").join("pack.yaml").is_file() {
            cwd
        } else {
            return Err(SyncError::PackRootRequired);
        }
    }
};
```

### W2+W4 — B4 dry-run gate + B8 events schema v2 (BUNDLED)

Bundled because both touch `crates/grex-core/src/audit/events.rs`. Single worker, single PR.

- `crates/grex-core/src/tree/walker.rs` — Phase 3 child entry: `if !ctx.dry_run { /* clone */ } else { /* emit DryRunWouldClone event */ }`. Also gate any `.gitignore` and lockfile mutations behind same check (cross-link with W5/W6).
- `crates/grex-core/src/audit/events.rs` —
  - bump `Event` serde to schema_version 2;
  - each event line emits `{"kind":"...","schema_version":2,"id":"<folder-name>","ref":"<manifest-ref>","ts":"..."}`;
  - drop legacy `pack` field;
  - add new variant `Event::DryRunWouldClone { id, ref_, url }`.
- `crates/grex-core/tests/walker_dry_run.rs` — new test. Asserts: dry_run=true → no clone subprocess invoked, no FS write under `<dest>/.git/`, no lockfile write, audit emits exactly N `DryRunWouldClone` events for N manifest children.
- `crates/grex-core/tests/events_schema_v2.rs` — new test. Asserts: each emitted JSONL line has `schema_version:2`, `id` non-empty, `ref` non-empty for action_started/completed; `id` equals `Path::file_name(pack_dir)`.
- `.omne/cfg/manifest.md` — SSOT update (separate repo; staged via `cd .omne/` first). Schema v2 spec.

Walker gate sketch:
```rust
// crates/grex-core/src/tree/walker.rs Phase 3
for child in &meta.children {
    if ctx.dry_run {
        ctx.audit.emit(Event::DryRunWouldClone {
            id: child.folder_name(),
            ref_: child.manifest_ref.clone(),
            url: child.url.clone(),
        });
        continue;
    }
    clone_child(child, ctx)?;          // network + FS
    write_gitignore_entry(/* killed by W5 */);
    write_lockfile_entry(child, ctx)?;
}
```

### W3 — B7 tracing → stderr + op name Display
- `crates/grex/src/cli/tracing_init.rs` — install `tracing_subscriber::fmt()` writer bound to `std::io::stderr` not `stdout`. Reject any current `MakeWriter::stdout()` usage.
- `crates/grex-core/src/op.rs` — add `impl Display for Op` rendering structured names (e.g. `sync`, `clone`, `quarantine_gc`). Replace any `format!("{:?}", op)` site that produced `Discriminant(3)`.
- `crates/grex/tests/tracing_to_stderr.rs` — extend existing test: assert stdout has zero `WARN` lines for any `grex sync` invocation. Assert tracing line contains op name (regex `op=sync` etc.) not `Discriminant`.

Display sketch:
```rust
// crates/grex-core/src/op.rs
impl std::fmt::Display for Op {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Op::Sync => "sync",
            Op::Clone => "clone",
            Op::QuarantineGc => "quarantine_gc",
            // exhaustive — Op is internal, not #[non_exhaustive]
        };
        f.write_str(s)
    }
}
```

### W5 — B12 kill .gitignore mutation + doctor advisory
- `crates/grex-core/src/tree/gitignore.rs` — DELETE the auto-write function entirely. Search for callsites; remove. Walker no longer touches `.gitignore`.
- `crates/grex-core/src/doctor/findings.rs` — add new finding kind `ParentGitTracksPackContent { pack_id, parent_dir }` with severity `Info`. Detection: `git -C <parent> ls-files --error-unmatch <pack_path>` returns 0 = tracked. Emits advisory finding; exit code unchanged.
- `crates/grex-core/tests/doctor_advisory.rs` — new test. Cases: (1) parent git tracks pack content → finding emitted; (2) pack ignored or no parent git → no finding.
- `crates/grex/tests/sync_no_gitignore_write.rs` — new test. Run `grex sync` in fixture, assert `.gitignore` byte-equal pre/post.

Doctor finding sketch:
```rust
// crates/grex-core/src/doctor/findings.rs
pub enum Finding {
    // ... existing variants ...
    ParentGitTracksPackContent { pack_id: String, parent_dir: PathBuf },
}

impl Finding {
    pub fn severity(&self) -> Severity {
        match self {
            Finding::ParentGitTracksPackContent { .. } => Severity::Info,
            // ...
        }
    }
}
```

### W6 — B14 lockfile branch carry
- `crates/grex-core/src/lockfile/writer.rs` — `LockEntry::from_resolved(child)` set `branch: child.manifest_ref.clone()` instead of empty string.
- `crates/grex-core/src/lockfile/schema.rs` — schema unchanged (field already exists, just empty).
- `crates/grex-core/tests/lockfile_branch_carry.rs` — new test. Manifest with `ref: main` → lockfile entry.branch = "main". Tag ref → branch = tag string. SHA ref → branch = SHA.

### Cross-cuts (any worker touches; resolve at merge)
- `crates/grex/Cargo.toml`, `crates/grex-core/Cargo.toml`, `crates/grex-cli/Cargo.toml`, `crates/real-smoke/Cargo.toml` — bump version `1.3.0` → `1.3.1`. Apply to all 4 crates.
- `Cargo.lock` — regen via `cargo update -w`.
- `crates/real-smoke/src/fixtures/` — assertion deltas: 6 fixtures flip from FAIL to PASS expectations (`t_b02, t_b04, t_b07, t_b08, t_b12, t_b14`).

## Lean theorems

Two new theorems extend the existing `Grex.Walker` + `Grex.Lockfile` modules. Axiom budget 9/4/0 unchanged (no new bridge axioms).

### T1 — Grex.Walker.dry_run_no_side_effects
```lean
theorem dry_run_no_side_effects
    (m : Manifest) (ctx : Ctx)
    (h : ctx.dry_run = true) :
    let trace := sync_meta_inner_model m ctx
    trace.network_calls = [] ∧
    trace.fs_writes = [] ∧
    (∀ e ∈ trace.events, e.kind = Event.dry_run_would_clone)
```
Bridge axiom (existing): `axiom rust_walker_matches_model : ...`. No new axioms expected. Proof strategy: induction on `m.children` length; base case trivial (empty trace satisfies all three conjuncts vacuously); step case relies on the `if !ctx.dry_run` gate landing in W2+W4 walker.rs.

### T2 — Grex.Lockfile.lockfile_branch_mirrors_manifest_ref
```lean
theorem lockfile_branch_mirrors_manifest_ref
    (m : Manifest) (lock : Lockfile)
    (h : lock = write_lockfile m) :
    ∀ child ∈ m.children,
      ∃ entry ∈ lock.entries,
        entry.id = child.id ∧ entry.branch = child.ref
```
Both compile on `[propext]` only or no axioms. CI axiom-stability gate accepts unchanged budget 9/4/0.

## Reviewer checks (Phase 2R)

cavecrew-reviewer + code-reviewer parallel pass on full diff. Required findings:

1. Public API surface drift = 0 (PATCH-additive). Compare exported symbols in `grex-core::lib.rs` and `grex-cli::lib.rs` pre/post.
2. No `Co-Authored-By` in any commit message or PR body (rule 13).
3. Frozen contract violations (per `.omne/cfg/freeze-v1.3.0.md`) = 0.
4. Worker desync: W2/W4 events.rs merge clean. W2/W5/W6 walker mutation gates aligned (all gated on same `if !dry_run`).
5. Test coverage delta: each worker adds at least one new test file; cumulative 6+ new test files.
6. clippy/rustfmt deltas: zero new lints.
7. Rule 8 sanity: 2 Lean theorems compile, axiom budget 9/4/0 unchanged.

## Validation gate (Phase 3)

```
cargo fmt --check --all
cargo build --workspace --all-targets
cargo build --workspace --all-targets --release
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
cargo doc --workspace --no-deps
lake build  (cwd = proof/)
python scripts/validate.py
xtask man-drift --check
xtask axiom-audit  (asserts 7 theorems on [propext]/no-axioms)
```

All exit 0 required.

## Real-smoke regression contract (Phase 4)

```
cargo run -p real-smoke -- --suite all --json
gh workflow run real-smoke.yml -f label=real-smoke
```

Both must report identical: 8 pass / 7 fail. Pass set: `t_b01, t_b02, t_b04, t_b07, t_b08, t_b09, t_b12, t_b14`. Fail set: remaining 7.

## Open risks

- **B12 removal**: operators relied on auto-add → migration note in `.omne/cfg/migration-v1.3.1.md`. Doctor advisory finding (W5) is the documented replacement path.
- **B8 schema_version bump**: v1.2.x readers cannot consume v1.3.1 logs (acceptable per maintainer 2026-05-02). Migration note + reader-compat matrix in `.omne/cfg/manifest.md` schema v2 section.
- **W2/W4 file-share**: enforced at dispatch — single bundle worker for `events.rs`. Reviewer check #4 catches any accidental split.
- **B14 SHA-ref edge case**: `branch: <40-char-sha>` is technically not a branch name. Lockfile schema doc (`.omne/cfg/lockfile.md`) clarifies field semantics: "ref-as-recorded, may be branch / tag / sha".
- **Cargo.toml version bump race**: 4 crate manifests + Cargo.lock. Single worker owns version bump after parallel work merges; W1–W6 must NOT touch version fields.
