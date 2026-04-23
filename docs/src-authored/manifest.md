# manifest

`grex.jsonl` (intent log) and `grex.lock.jsonl` (resolved state). Both live at the workspace root. Both are newline-delimited JSON (LF on all platforms — writer normalizes).

## Two-file split

| File | Purpose | Written by |
|---|---|---|
| `grex.jsonl` | Append-only **intent** log. User actions: register a pack, remove a pack, update a ref. | `add`, `rm`, `update` verbs. |
| `grex.lock.jsonl` | Append-only **resolved** state. Actual SHA + install state after each successful sync/install. | `sync`, `update` verbs. |

Split rationale: intent is portable across machines; lockfile pins the actual state on this machine. Commit intent to git; lockfile may be committed too (for reproducible bootstrap) or gitignored (for per-machine pinning).

## `grex.jsonl` event schemas

Common envelope (all events):

```json
{"op":"<verb>","ts":"<rfc3339>","id":"<pack-id>","schema_version":"1"}
```

### `add`

```jsonl
{"op":"add","ts":"2026-04-19T10:00:00Z","id":"warp-cfg","schema_version":"1","url":"git@github.com:user/warp-cfg","path":"warp-cfg","type":"declarative","ref":"main"}
```

### `rm`

```jsonl
{"op":"rm","ts":"2026-04-19T11:00:00Z","id":"warp-cfg","schema_version":"1"}
```

### `update`

```jsonl
{"op":"update","ts":"2026-04-19T12:00:00Z","id":"warp-cfg","schema_version":"1","ref":"v0.2.0"}
```

### `sync` (optional intent marker)

```jsonl
{"op":"sync","ts":"2026-04-19T13:00:00Z","id":"warp-cfg","schema_version":"1"}
```

### Action event brackets — `action_started` / `action_completed` / `action_halted`

The sync path writes three bracketing events around each action it applies. These sit alongside (do not replace) the `sync` intent marker; readers built against v1.0 continue to parse cleanly — unknown `op` values are ignored per the forward-compat rule.

```jsonl
{"op":"action_started","ts":"2026-04-20T10:00:00Z","id":"warp-cfg","schema_version":"1","action":"symlink","idx":0}
{"op":"action_completed","ts":"2026-04-20T10:00:00Z","id":"warp-cfg","schema_version":"1","action":"symlink","idx":0,"changed":true}
{"op":"action_halted","ts":"2026-04-20T10:00:01Z","id":"warp-cfg","schema_version":"1","action":"exec","idx":1,"reason":"ExecNonZero","stderr":"<truncated to 2 KiB>"}
```

Semantics:

- `action_started` is written under the manifest lock **before** the action runs.
- `action_completed` is written under the manifest lock **after** the action returns `Ok`.
- `action_halted` is written when the action returns `Err`, carrying a compact failure reason plus (for `exec`) a stderr tail capped at 2 KiB (see [actions.md §exec](./actions.md#7-exec)).
- An `action_started` with no matching `action_completed` / `action_halted` indicates a crash mid-action. The startup recovery scan (see [concurrency.md §Recovery scan](./concurrency.md#recovery-scan)) reports these; cleanup is `grex doctor` territory (M4+).

`ManifestLock` is acquired **per-action** (not per-sync), so a long sync with many actions interleaves lock acquire/release rather than holding the global lock end-to-end.

Fold algorithm (pseudocode):

```
state = {}
for line in read_jsonl(grex.jsonl):
    match line.op:
        "add":    state[id] = Pack::from(line)
        "update": state[id].patch(line)
        "rm":     state.remove(id)
        "sync":   no-op (intent marker)
return state
```

O(N) in event count. Deterministic regardless of compaction history.

## `grex.lock.jsonl` resolved-state schema

```jsonl
{"id":"warp-cfg","sha":"abc123...","branch":"main","installed_at":"2026-04-19T13:05:00Z","actions_hash":"sha256:deadbeef..."}
```

Fields:

| Field | Required | Description |
|---|---|---|
| `id` | yes | Pack id; matches manifest `id`. |
| `sha` | yes | Git commit SHA of the pack workdir after sync. Stored as the empty string when the pack is not a git working tree (e.g. a local-only root pack) OR when the HEAD probe failed. `actions_hash` is computed with the same `commit_sha` value, so empty-SHA records are internally consistent — if a future sync successfully probes a non-empty SHA, the hash differs and the skip-on-hash short-circuit correctly re-executes the pack. Probe failures are surfaced as a `grex::walker` `tracing::warn!` line so operators see the signal without the sync aborting. Lockfile-write failures at end-of-sync are intentionally non-fatal (recorded as a `report.event_log_warnings` entry); the successful pack actions are not rolled back. |
| `branch` | no | Branch tracked; null if detached. |
| `installed_at` | yes | RFC3339 timestamp of last successful install/sync. |
| `actions_hash` | yes | SHA-256 content fingerprint of the pack's installable surface. Scope varies by pack type (see below). Used to detect whether `update` needs to re-run install logic. |

`actions_hash` scope by pack type (name retained; semantics explicitly broadened):

- `declarative`: hash of normalized `actions` array + `files/` tree.
- `meta`: hash of the serialized `children` array + each child's resolved SHA (from the child's lockfile entry). Captures the fact that a meta pack's installable surface is the set of owned children at pinned revisions.
- `scripted`: hash of normalized `actions` array (if any) + `files/` tree + SHA-256 of each hook file in `.grex/hooks/` (sorted by filename, then concatenated). Any hook edit re-triggers `update`.

Rationale for keeping the name `actions_hash`: the field's purpose — "has the installable content changed since last sync?" — is unchanged; only its per-type inputs differ. Renaming would force a lockfile schema bump for no semantic gain.

Fold for lockfile: last-line-wins per `id`.

### `type` field authority

The `type` recorded on `add` events and in lockfile entries is an **observed snapshot** of what the pack reported at that moment. The authoritative source of truth is `.grex/pack.yaml`'s `type` field (see [pack-spec.md §Validation rules](./pack-spec.md#validation-rules)). If the manifest `type` disagrees with pack.yaml on a subsequent sync, pack.yaml wins and the manifest is corrected by emitting a fresh `add`/`update` event reflecting the true type. Readers MUST NOT treat manifest `type` as normative when pack.yaml is available.

## Atomic append

Single-line append uses buffered write + `fsync`:

```rust
let mut f = OpenOptions::new().append(true).open("grex.jsonl")?;
f.write_all(line.as_bytes())?;
f.write_all(b"\n")?;
f.sync_data()?;
```

Held under fd-lock. POSIX append is atomic for writes ≤ PIPE_BUF; we enforce event size ≤ 2 KiB to stay inside.

## Compaction (temp + rename)

Periodic or on `grex doctor --compact`:

1. Acquire global fd-lock (exclusive).
2. Fold events → state map.
3. Emit minimal equivalent event set to `grex.jsonl.tmp` (one `add` per live id, tombstoned ids dropped entirely).
4. `fs::rename(grex.jsonl.tmp, grex.jsonl)` — atomic on POSIX and Windows NTFS (`MoveFileEx` with `REPLACE_EXISTING`).
5. Release fd-lock.

Invariant: `fold(pre-compaction) == fold(post-compaction)`.

Lockfile compaction mirrors intent-log compaction: last-line-wins per id → one line per id → atomic rename.

## Locking

Global RW lock via `fd-lock`:

```rust
let file = OpenOptions::new().read(true).write(true).open("grex.jsonl")?;
let mut lock = fd_lock::RwLock::new(file);
let _guard = lock.write()?;  // exclusive for append/compact
```

- Mutators (`add`, `rm`, `update`, `sync` write-phase, `doctor --compact`) take exclusive write lock.
- Readers (`ls`, `status`, `sync` read-phase) take shared read lock.

## Crash recovery (torn-line detection)

On every read:

1. Parse line-by-line.
2. If the final line fails JSON parse **AND** file does not end in `\n`, treat as torn write.
3. Truncate file to length of last valid line.
4. Emit tracing warning; continue.

Test: `tests/crash_recovery.rs` spawns a child, SIGKILL / TerminateProcess mid-append, asserts parent recovers.

## Schema versioning

Every event has `schema_version: "1"`. Breaking changes bump. Reader rejects unknown versions with actionable error pointing to `grex upgrade-schema` (post-v1 migration command).

Lockfile entries carry an implicit schema version tied to the workspace config. Separate bump cadence from intent-log schema.

## Migration from legacy `REPOS.json`

`grex import --from-repos-json <path>` reads flat `[{"url":"...","path":"..."},...]` → emits one `add` event per entry with `type` defaulted to `meta` (or user-specified via `--default-type`). Idempotent: re-running detects existing ids by `path` and no-ops.

## Example sequence

```jsonl
{"op":"add","ts":"2026-04-19T10:00:00Z","id":"warp-cfg","schema_version":"1","url":"git@github.com:me/warp-cfg","path":"warp-cfg","type":"declarative","ref":"main"}
{"op":"add","ts":"2026-04-19T10:01:00Z","id":"fonts","schema_version":"1","url":"git@github.com:me/fonts","path":"fonts","type":"meta","ref":"main"}
{"op":"update","ts":"2026-04-19T11:00:00Z","id":"warp-cfg","schema_version":"1","ref":"v0.2.0"}
{"op":"rm","ts":"2026-04-19T12:00:00Z","id":"fonts","schema_version":"1"}
```

Corresponding lock after first successful sync:

```jsonl
{"id":"warp-cfg","sha":"abc123def","branch":"main","installed_at":"2026-04-19T10:00:05Z","actions_hash":"sha256:..."}
{"id":"fonts","sha":"fff111","branch":"main","installed_at":"2026-04-19T10:01:05Z","actions_hash":"sha256:..."}
```

Fold of intent log → live set = `{warp-cfg}` (fonts tombstoned). Subsequent sync rewrites lockfile entry for warp-cfg and drops the fonts line on compaction.
