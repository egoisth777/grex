# CLI `--json` output

Every non-transport verb honours the global `--json` flag. When present,
the verb writes a single JSON document to stdout and suppresses the
default human-readable output. The `serve` verb is excluded — it owns
stdio for JSON-RPC framing, so `--json` is not applicable there.

This chapter is the **v1 JSON contract**. Field names are stable across
PATCH / MINOR releases; new fields may be added (readers must ignore
unknown keys per the manifest wire invariant). Breaking changes require
a MAJOR bump and a deprecation cycle — see [`semver.md`](../semver.md).

## Two envelope families

Every `--json` payload belongs to exactly one of two families. Callers
distinguish them by the presence or absence of a top-level `status` key:

| discriminant              | envelope family           | stability                                                                 |
|---------------------------|---------------------------|---------------------------------------------------------------------------|
| `"status": "unimplemented"` | **stub envelope**         | stable shape *while the verb remains unimplemented* (see below)           |
| *no* `status` key         | verb-specific shape       | stable shape per the verb's section below                                  |

A verb transitioning from unimplemented to wired is a **schema addition**,
not a replacement: the stub envelope is dropped and a verb-specific shape
takes its place. Consumers MUST branch on the presence of `status`:

```jsonc
// Pseudocode — pick the right parser per verb
if (payload.status === "unimplemented") {
  // Stub verb. Treat as "no semantic data yet" and proceed.
} else {
  // Verb-specific shape documented below.
}
```

The two families never co-exist in the same payload. `status` is reserved
for the stub envelope; no verb-specific shape will ever gain a top-level
`status` field.

## Stub envelope (unimplemented verbs)

`init`, `rm`, `status`, `update`, `run`, `exec` are still
M1 stubs. `--json` emits:

```json
{"status": "unimplemented", "verb": "init"}
```

Fields:
- `status` — always the literal string `"unimplemented"`.
- `verb` — the verb name as typed on the command line.

The stub envelope is a contract for consumers to detect unfinished verbs
without parsing the (absent) verb-specific body. When the verb is wired,
the stub envelope is removed; the verb now emits its verb-specific
shape. Exit codes are unchanged (stubs exit `0`).

## `add`

Wired. Emits an add registration report:

```json
{
  "dry_run": false,
  "id": "pack-a",
  "url": "https://example.com/pack-a.git",
  "path": "pack-a",
  "type": "scripted",
  "appended": true
}
```

Fields:
- `dry_run` — bool; mirrors the global `--dry-run` flag.
- `id` — pack id written to the manifest; currently equal to `path`.
- `url` — source URL as provided.
- `path` — workspace-relative pack path, explicit or inferred from URL.
- `type` — classified pack kind (`scripted` for git-like URLs,
  `declarative` otherwise).
- `appended` — bool; `false` only when `dry_run` is `true`.

The MCP `add` tool emits a byte-identical body.

## `ls`

Wired in v1.1.1. Walks the workspace from a root `pack.yaml` (or the
current directory when no `pack_root` is given) without cloning,
fetching, or executing anything, and emits a structured tree envelope:

```json
{
  "workspace": "/abs/path/to/workspace",
  "tree": [
    {
      "id": 0,
      "name": "rootp",
      "path": "/abs/path/to/workspace",
      "type": "meta",
      "synthetic": false,
      "children": [
        {
          "id": 1,
          "name": "alpha",
          "path": "/abs/path/to/workspace/alpha",
          "type": "scripted",
          "synthetic": true,
          "children": []
        }
      ]
    }
  ]
}
```

Fields:
- `workspace` — absolute path to the resolved workspace (the directory
  holding the root pack's `.grex/`, or the pack root itself for the
  flat-sibling layout).
- `tree[]` — root-level nodes. Currently always one entry; the array
  shape is reserved so future surfaces walking from a workspace dir
  with multiple sibling packs can extend without a schema break.
- Per node: `id` (stable in-walk depth-first counter, root = 0),
  `name`, `path` (absolute), `type` (one of `meta`, `declarative`,
  `scripted`), `synthetic` (bool — see below), `children[]`.

`synthetic: true` indicates a plain-git child whose pack manifest was
synthesised in-memory by the walker (the destination directory carries
`.git/` but no `.grex/pack.yaml`). Synthetic nodes always carry
`type: "scripted"` per the v1.1.1 design. See
[`pack-spec.md`](../concepts/pack-spec.md) §"Plain-git children" for
the full contract.

### Error envelope

```json
{"verb": "ls", "error": {"kind": "tree", "message": "..."}}
```

`kind` values: `tree` (root manifest could not be loaded), `usage`
(invalid `pack_root` argument). The verb exits `2` on error and `0`
on success.

The MCP `ls` tool emits a byte-identical successful body. The MCP
surface does NOT accept a `pack_root` parameter (workspace-confinement
invariant); the walk always starts from the server's pinned workspace.

## `sync` and `teardown`

These verbs drive the M3 Stage B pipeline. `--json` emits a
`SyncReport`-shaped document:

```json
{
  "verb": "sync",
  "dry_run": false,
  "steps": [
    {"pack": "a", "action": "file-write", "idx": 0, "result": "performed_change", "details": null},
    {"pack": "b", "action": "shell-run", "idx": 1, "result": "skipped",
     "details": {"pack_path": "b", "actions_hash": "sha256:..."}}
  ],
  "halted": null,
  "event_log_warnings": [],
  "summary": {"total_steps": 2}
}
```

`result` values: `performed_change`, `would_perform_change`,
`already_satisfied`, `noop`, `skipped`, `other`.

### Missing `<pack_root>` → usage error (exit 2)

`sync` / `teardown` without a `<pack_root>` positional emit a
verb-specific error envelope and exit `2` (the frozen usage-error exit
code from `cli.md`):

```json
{
  "verb": "sync",
  "error": {"kind": "usage", "message": "`<pack_root>` is required (directory with `.grex/pack.yaml` or the YAML file)"}
}
```

This is NOT a stub envelope — no `status` key. The usage-error branch is
distinct from the unimplemented-verb branch so callers can distinguish
"tell the user to fix their invocation" (exit 2) from "this verb has no
implementation yet" (exit 0).

### Error envelope (other failure paths)

Validation / tree / exec / halted paths share the same envelope shape:

```json
{
  "verb": "sync",
  "error": {"kind": "validation", "message": "…"}
}
```

`kind` values: `validation`, `tree`, `exec`, `usage`, `other`. The
`halted` sub-case emits a dedicated shape:

```json
{"verb": "sync", "halted": {"pack": "a", "action": "shell-run",
 "idx": 0, "error": "…", "recovery_hint": "…"}}
```

## `doctor`

Wired. Emits a `DoctorReport`:

```json
{
  "exit_code": 0,
  "worst_severity": "ok",
  "findings": [
    {"check": "manifest-schema", "severity": "ok",
     "pack": null, "detail": "", "auto_fixable": false, "synthetic": false},
    {"check": "synthetic-pack", "severity": "ok",
     "pack": "algo-leet", "detail": "OK (synthetic)",
     "auto_fixable": false, "synthetic": true}
  ]
}
```

Fields:
- `exit_code` — number; the severity-roll-up exit code the CLI also
  returns from the process.
- `worst_severity` — string; one of `ok` / `warning` / `error`. Matches
  the highest severity in `findings`.
- `findings[]` — array of per-check finding objects.

`severity` values: `ok`, `warning`, `error`.

v1.1.1+ adds `synthetic: true` to findings for synthetic plain-git
children (skipped schema validation; gitignore + drift checks still
run). The flag mirrors the `synthetic` marker on the matching `LsTree`
node and on the lockfile entry, so consumers correlating doctor
findings with `grex ls` output see the same plain-git provenance on
both surfaces.

The MCP `doctor` tool emits a byte-identical body. The MCP surface does
NOT accept `--fix` (read-only inspection only) or `--workspace`
(workspace-confinement invariant). CLI-only users retain `grex doctor
--fix` for interactive gitignore healing.

## `import`

Wired. Emits an `ImportPlan`:

```json
{
  "dry_run": true,
  "imported": [
    {"path": "pack-a", "url": "https://…", "kind": "declarative",
     "would_dispatch": true}
  ],
  "skipped": [{"path": "pack-b", "reason": "path_collision"}],
  "failed": []
}
```

Fields:
- `dry_run` — bool; mirrors whichever of `--dry-run` / global `--dry-run`
  was in effect.
- `imported[]` — entries that will be (or were) added to the manifest.
- `skipped[]` — entries excluded; `reason` is one of `path_collision`,
  `duplicate_in_input`.
- `failed[]` — entries that errored during ingest; each carries a
  human-readable `error` string.

No `summary` wrapper — callers derive counts from the three arrays. The
MCP `import` tool emits a byte-identical body. The MCP surface does NOT
accept a `workspace` parameter (workspace-confinement invariant); the
`fromReposJson` path is resolved relative to the server's workspace and
rejected if it canonicalises outside it.

## Exit codes

`--json` does not alter exit codes — callers MUST use the process exit
code as the source of truth for success / failure, not the presence of
an `error` key. The JSON payload is diagnostic detail, not the wire
signal.
