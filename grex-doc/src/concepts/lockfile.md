# lockfile

`grex.lock.jsonl` — the resolved-state snapshot that pins each pack's last-synced commit, ref, install timestamp, and `actions_hash`. Companion to but distinct from the `events.jsonl` intent/audit log (see [manifest](./manifest.md)).

> Canonical source: [.omne/cfg/lockfile.md](../../.omne/cfg/lockfile.md) (SSOT, separate `grex-inst` repo). This page is the user-facing projection.

## Concept: pack.yaml = INTENT, lockfile = STATE

The two artifacts answer different questions:

| Artifact                       | Question answered                                                | Authored by      |
|--------------------------------|------------------------------------------------------------------|------------------|
| `pack.yaml` (+ `events.jsonl`) | "Which children at which paths at which refs do I want?"         | User / pack author |
| `grex.lock.jsonl`              | "Which commits am I currently at, with which actions applied?"   | `sync` / `update` |

Same intent-vs-state separation as Cargo (`Cargo.toml` / `Cargo.lock`), npm (`package.json` / `package-lock.json`), Bundler, Poetry. The lockfile lives next to its meta's manifest — see [§File location](#file-location).

### Why both are needed

- **Idempotency (skip-on-hash).** `sync` re-resolves the ref → SHA, recomputes `actions_hash`, compares to the recorded entry, short-circuits if both match. Without a lockfile, every sync would re-execute every action.
- **Drift triangulation (3-leg).** `doctor` compares **declared** (manifest) vs **recorded** (lockfile) vs **present** (disk). A 2-leg model cannot distinguish "user edited pack.yaml since last sync" from "someone hand-edited the working tree".
- **Concurrent-sync safety.** Lockfile-write happens under the manifest fd-lock; `sync` reads it once at plan phase and writes once at the end.

## File location

```text
<meta>/.grex/grex.lock.jsonl
```

**Distributed under v1.2.0+:** EACH meta owns its own `<meta>/.grex/grex.lock.jsonl`, tracking ONLY that meta's direct children. There is no global workspace lockfile — each recursion frame in [walker §Three phases](./walker.md#the-three-phases) reads and writes its OWN lockfile.

Both the lockfile (`.grex/grex.lock.jsonl`) and the event log (`.grex/events.jsonl`) live in the manifest folder `.grex/`. Their names are deliberately distinct (no shared `grex.*.jsonl` prefix) to prevent the lockfile-vs-event-log conflation that caused historical SSOT errors.

## Three "lock" artifacts — disambiguation

The codebase has three artifacts whose names contain "lock". The rule of thumb: **if it ends in `.jsonl` it carries state; if it does not, it is a mutex.**

| Artifact            | Path (v1.2.0+)                                | Purpose                                                     | Format      |
|---------------------|-----------------------------------------------|-------------------------------------------------------------|-------------|
| **THE lockfile**    | `<meta>/.grex/grex.lock.jsonl`                | Resolved-state snapshot (commit + actions_hash per pack)    | JSONL       |
| **Event log**       | `<meta>/.grex/events.jsonl`                   | Append-only history of `add`/`rm`/`update`/`sync` events    | JSONL       |
| **Manifest fd-lock**| `<meta>/.grex.lock`                           | OS-level file mutex serialising lockfile + event-log writes | Empty file  |

Other file mutexes (`<meta>/.grex.sync.lock` per-meta-sync, `<dest>.grex-backend.lock` per-repo, `<pack>/.grex-lock` per-pack) are documented in [concurrency §Five cooperating mechanisms](./concurrency.md#five-cooperating-mechanisms); none of them carry state — they exist solely for mutual exclusion.

## `LockEntry` schema

```jsonl
{"id":"warp-cfg","sha":"abc123...","branch":"main","installed_at":"2026-04-19T13:05:00Z","actions_hash":"sha256:deadbeef...","path":"warp-cfg"}
```

| Field           | Since   | Notes                                                                                                              |
|-----------------|---------|--------------------------------------------------------------------------------------------------------------------|
| `id`            | v1.0    | Pack `name:` slug; matches `Event::Add.id`.                                                                        |
| `sha`           | v1.0    | Resolved commit SHA; empty string if pack is non-git or HEAD probe failed.                                         |
| `branch`        | v1.0    | Tracked branch; null if detached.                                                                                  |
| `installed_at`  | v1.0    | RFC3339 timestamp of last successful install/sync.                                                                 |
| `actions_hash`  | v1.0    | SHA-256 over installable surface (scope per pack-type — see [manifest §actions_hash scope](./manifest.md#grexlockjsonl-resolved-state-schema)). |
| `schema_version`| v1.0    | Bumped on breaking lockfile schema change.                                                                         |
| `synthetic`     | v1.1.1  | `true` for plain-git children synthesized by the walker (semantically dead under v1.2.0+ — see below).             |
| `path`          | v1.2.0  | `Option<String>` `#[serde(default)]`, parent-meta-relative POSIX, normalised at write-time. Lookup-map key.        |

The `path` field is the lookup-map key under v1.2.0's nested-children layout. See [walker §Lockfile keying](./walker.md#lockfile-keying).

## Three readers

| Reader   | What it does with the lockfile                                                                                                                              |
|----------|-------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `sync`   | **Skip-on-hash.** Re-resolve commit, recompute `actions_hash`, compare against the prior `LockEntry`. Match → skip; mismatch → re-execute.                  |
| `doctor` | **Drift triangulation.** Joins three legs: declared (manifest fold), recorded (lockfile entry), present (disk readdir / git probe). Each pair-mismatch is a distinct drift class. |
| `ls`     | **State render.** Per-pack synced / unsynced status. `ls --long` reads SHA + `installed_at` + `actions_hash` directly without folding the event log.        |

## Path keying and v1.1.1 → v1.2.0 read-fallback

Through v1.1.1, lockfile entries were keyed by bare pack `id` (manifest `name:`), and the flat-sibling rule guaranteed `id` was unique within the single global workspace lockfile. Under v1.2.0's nested child paths, two declared children at distinct paths (e.g. `tools/foo` and `vendor/foo`) MAY share the same `name:` — a bare-`id` key would collide.

**v1.2.0 decision: path-keyed, per-meta lockfile.** Each meta owns its own lockfile tracking ONLY its direct children. Within that lockfile the in-memory index keys entries by **meta-relative** pack path (canonical relative POSIX, normalised at write-time, NFC).

### Read-time fallback for v1.1.1 lockfiles

When a v1.2.0 binary reads a v1.1.1 lockfile entry where `path: None` (deserialized via `#[serde(default)]`), the path is derived as `Some(entry.id.clone())`. This is sound because v1.1.1 enforced bare-name-only paths (the validator rejected `/`), so `id == path` for all v1.1.1 entries. The walker proceeds without rewriting the file. After the next successful sync the entry is rewritten with `path: Some(...)`, and subsequent reads bypass the fallback.

This means **v1.2.0 reads v1.1.1 lockfiles cleanly with no manual migration step required**. The library function `grex_core::lockfile::migrate_v1_1_1` and the planned `grex migrate-lockfile` CLI subcommand (v1.2.1) exist for users who want to eagerly upgrade the lockfile bytes to v1.2.0 schema (e.g. before committing to git). Both are opt-in.

### Migration path summary

| Scenario                                                  | Behaviour                                                                                              |
|-----------------------------------------------------------|--------------------------------------------------------------------------------------------------------|
| v1.2.0 binary reads v1.1.1 lockfile (no edit)             | Read-fallback: `path` derived as `id`. Sync proceeds.                                                  |
| v1.2.0 binary writes after a successful sync              | All entries written with `path: Some(...)`. Subsequent reads use the on-disk `path` directly.          |
| User wants to eagerly upgrade lockfile bytes              | `grex migrate-lockfile [--dry-run] [--workspace <path>]` (v1.2.1) — atomic temp+rename, idempotent.    |
| User downgrades v1.2.0 → v1.1.x                           | v1.1.x reader ignores `path:` field (forward-compat — unknown fields skipped); `id`-keyed lookup still works. |

## `LockEntry.synthetic` deprecation

The `synthetic: bool` field on `LockEntry` (introduced v1.1.1 to mark plain-git children synthesised by the walker) is **semantically dead under v1.2.0+**. No v1.2.0 code path sets `synthetic: true`. The field is retained on the struct for backward-compat reads — v1.1.x lockfiles continue to deserialize cleanly.

A future schema bump (post-v1.2.0) MAY drop the field. Until then, a successful v1.2.0 sync against a workspace with synthetic entries either (a) finds the path now registered via `grex add` (entry rewritten with `synthetic: false`), or (b) reports `UntrackedGitRepos` and refuses to proceed. Either way, no v1.2.0+ sync writes a fresh synthetic entry.

## Lifecycle

1. **First sync.** Walker reads `pack.yaml` graph → clones each child → runs install actions → writes one `LockEntry` per direct child of the cwd-meta into `<cwd-meta>/.grex/grex.lock.jsonl`. Sub-metas write their own lockfiles in their own `.grex/` dirs.
2. **Re-sync (no edits).** Walker re-resolves refs → for each pack, recomputes `actions_hash` and compares to the recorded entry. Match → skip; lockfile entry carried forward unchanged.
3. **Re-sync after `pack.yaml` edit.** User changes a child's `ref` or an `actions` block → next sync's `actions_hash` differs → pack re-executes → `LockEntry` rewritten with new `sha` / `actions_hash` / `installed_at`.
4. **Child removed from `pack.yaml` `children:`.** Next sync's [walker Phase 2](./walker.md#phase-2--prune-children-removed-from-manifest) reconciles the lockfile against the manifest, deletes the orphan dest (subject to prune-safety — see [force-prune](./force-prune.md)), and removes the lockfile entry.
5. **`grex doctor`.** Reads lockfile + intent-log fold + disk state → flags drift across the three legs.
6. **Lockfile-write failure at end-of-sync.** Intentionally **non-fatal**. Successful pack actions are not rolled back; the failure is recorded as a `report.event_log_warnings` entry.

## Crash recovery

Lockfile writes use **write-then-rename atomicity** (write to `<lockfile>.tmp`, fsync, rename over the original). A crash mid-write leaves either the old or the new file fully intact — never a torn JSONL. The manifest fd-lock (see [concurrency](./concurrency.md)) serialises all writes, so concurrent torn writes are also impossible.

On read, parse-failure of any line surfaces as `LockfileCorrupt(path, line_no, parse_error)`:

- Severity `Warning` under `grex doctor` (which can repair the file by replaying from the event log + a clean re-sync).
- Severity `Error` under `grex sync` (refuses to plan against a corrupt lockfile).

User remediation path: `grex doctor` → repair → re-sync.

## Cross-references

- Walker keying decision + parent-relative model: [walker](./walker.md)
- File mutexes (sync-lock, backend-lock, pack-lock, manifest-lock) + Lean4 I1: [concurrency](./concurrency.md)
- Schema field table + intent-log split + crash recovery: [manifest](./manifest.md)
- Force-prune audit log + safety contract: [force-prune](./force-prune.md)
