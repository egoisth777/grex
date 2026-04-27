# migration — from `REPOS.json` + `.scripts/` to `grex`

Users on the legacy Python `.scripts/` meta-repo migrate by running `grex import --from-repos-json ./REPOS.json`. Both systems can coexist during transition.

## Legacy source system

```
repo/
├── .scripts/
│   ├── init.py  add.py  rm.py  sync.py  track.py  test.py
│   ├── lib/
│   └── hooks/
├── REPOS.json        # [{url, path}, ...]
└── .gitignore        # hand-curated, sub-repo dirs appended
```

`REPOS.json` shape:

```json
[
  {"url": "https://github.com/grex-org/grex-tui.git",   "path": "grex-tui"},
  {"url": "https://github.com/grex-org/grex-core.git",  "path": "grex-core"}
]
```

Legacy shell native scripts (`.ps1`/`.sh`) are irrelevant in grex — Rust `std::fs` + built-in actions replace them.

## Import command

```
grex import --from-repos-json ./REPOS.json
```

Behavior:

1. Read + parse `REPOS.json`. Validate `url` and `path` (bare name) on every entry.
2. For each entry not already in `grex.jsonl` (by `path`), emit an `add` event with `type: meta` (or `--default-type <...>`).
3. For each entry already present with matching URL, skip.
4. For each entry with same `path` but different `url`, abort unless `--force` (then emit `update`).
5. Optionally `--migrate-gitignore`: rewrite `.gitignore` to use the managed-block format, preserving pre-existing lines outside the managed region.

```
grex import --from-repos-json ./REPOS.json --migrate-gitignore
```

Idempotent: re-running is a no-op once imported.

## Disk-scan variant

```
grex import --scan
```

Walks workspace root one level deep, detects directories with `.git/` not yet in `grex.jsonl`. For each, reads `git config --get remote.origin.url`, emits an `add` event. Skips entries without a remote.

Combinable with `--from-repos-json`: both sources processed, deduplicated by `path`.

## Pack type for imported entries

Legacy `REPOS.json` carries no pack type info. Default assumption:

- If the imported dir contains a `.grex/pack.yaml`, use its declared `type`.
- Else use `--default-type` (flag), which defaults to `meta` (safe: meta packs have no actions, so no surprise side effects on first install).

User can later convert to `declarative` or `scripted` by adding a `.grex/pack.yaml` in the imported pack's own repo.

## Coexistence during transition

Both systems can run against the same workspace if:

- `.scripts/` remains in place unmodified.
- `grex.jsonl` is added alongside `REPOS.json`.
- `.gitignore` is in managed-block format and lists every `path` from BOTH sources.

`grex doctor` in coexistence mode:

- Warns (non-fatal) if `REPOS.json` has entries missing from `grex.jsonl`.
- Warns if `.scripts/` is still present while `grex.jsonl` exists.
- Suggests running `grex import --from-repos-json` or retiring `.scripts/`.

## Disambiguation rules

Same `path` in both sources:

| `REPOS.json` | `grex.jsonl` | Action |
|---|---|---|
| present | absent | `add` event emitted |
| present (url A) | present (url A) | no-op |
| present (url A) | present (url B) | error, abort without `--force` |
| absent | present | no-op |
| present | tombstoned | skip, log info |

`--force` resolves URL conflicts by emitting `update` from the `REPOS.json` value.

## Path rule transition

Legacy `REPOS.json` required bare `path` (no separators). v1 `grex` preserves this. Nested paths (e.g. `packs/foo`) are deferred to v1.x; will require path-normalization + collision detection.

## Retirement of `.scripts/`

Post-migration:

1. Verify `grex ls` matches expected pack list.
2. Run `grex sync --parallel 8`.
3. Delete `.scripts/` via `git rm -r .scripts/`.
4. Delete `REPOS.json`.
5. `git config core.hooksPath .grex/hooks` (grex installs these on `init`).
6. Commit.

`grex doctor` after retirement should exit 0 on clean workspace.

## Rollback

Nothing in `grex import` mutates `.scripts/` or `REPOS.json`. Rollback = delete `grex.jsonl` + `grex.lock.jsonl` + revert `.gitignore` (if `--migrate-gitignore` used). No data loss path.
