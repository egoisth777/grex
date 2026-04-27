# pack-spec

The `.grex/` contract directory and `pack.yaml` schema v1. Normative.

## Pack definition

A **pack** is a git repository containing a `.grex/` directory at its root. grex reads and acts on the contract inside `.grex/`; everything else in the repo is opaque.

```
some-pack/                   # git repo root
├── .grex/                   # contract dir (required)
│   ├── pack.yaml            # required: pack manifest
│   ├── targets/             # optional: platform overrides
│   │   ├── windows.yaml
│   │   ├── linux.yaml
│   │   └── macos.yaml
│   ├── files/               # optional: payload files (configs, themes)
│   ├── hooks/               # optional: scripted-type escape hatch
│   │   ├── setup.sh / .ps1
│   │   ├── sync.sh / .ps1
│   │   └── teardown.sh / .ps1
│   └── .state/              # gitignored: runtime state cache
└── ...                      # opaque to grex
```

## `pack.yaml` schema v1

### Top-level fields

| Field | Type | Required | Notes |
|---|---|---|---|
| `schema_version` | string | yes | Must be `"1"`. Future reader rejects unknown. |
| `name` | string | yes | Unique within the parent workspace. Slug-like. |
| `type` | string | yes | One of `meta`, `declarative`, `scripted`. |
| `version` | string | no | Pack's own semver; not enforced by grex v1. |
| `depends_on` | list[string\|url] | no | External prerequisites. Tool **verifies** presence; does NOT clone or walk. See below. |
| `children` | list[child-ref] | no | **Owned** sub-packs. Tool clones, walks, and syncs transitively. See below. |
| `actions` | list[action] | no | Ordered action list. Meaningful for `type: declarative` (and declarative children of meta). |
| `teardown` | list[action] | no | Optional explicit teardown. If omitted, default = reverse of `actions`. |

### `children` vs `depends_on` — ownership split

The two edge types in the pack graph are **distinct** and tools must not conflate them:

- `children` — **owned** sub-packs. grex clones them into the workspace, walks into each on sync, and applies their lifecycle transitively. Children appear in the pack tree output (`grex ls`). Removing a parent teardowns its children.
- `depends_on` — **external prerequisites**. grex **verifies** the named/URL'd packs are already present and satisfied in the workspace, but does NOT clone, walk, or modify them. They do not appear under the dependent pack in the pack tree. Failure to resolve a `depends_on` entry is a **hard error at plan phase** (before any action runs).

Every pack graph therefore has two edge kinds: a `children` edge (ownership / walk) and a `depends_on` edge (verification only). Cycle detection runs over both independently.

### `children` child-ref shape

```yaml
children:
  - url: git@github.com:user/warp-themes
    path: themes         # optional; default = last URL segment
    ref: v1.2.0          # optional; branch, tag, or SHA. Default: remote HEAD.
```

Children resolve as **flat siblings** of the parent pack root: a parent at `~/code/.grex/pack.yaml` with a child `path: themes` materialises that child at `~/code/themes/.grex/pack.yaml`. The bare-name rule on `path` is enforced at plan phase since v1.1.0 — see [Validation rules](#validation-rules) for the regex and rejection shape.

### `actions` list

Each entry is a YAML object with exactly one known action key (`symlink`, `env`, `mkdir`, `rmdir`, `require`, `when`, `exec`) or a plugin-registered name. The value under the key is the action's arg-object, per that action's schema (see [actions.md](../reference/actions.md)).

### Targets / platform overrides

Files under `.grex/targets/{windows,linux,macos}.yaml` are merged **over** the base `pack.yaml` on the matching OS. Merge rules:

- Top-level scalars (`name`, `type`, `version`): override replaces.
- Lists (`actions`, `children`, `depends_on`): appended (base first, then override), unless the override sets `actions_replace: true` at top level.
- The override file follows the same schema as `pack.yaml` (minus `schema_version`; inherited).

Alternative to separate files: inline `when:` gates in `actions` (platform dispatch via the `when` action — see below).

### `files/` payload convention

Arbitrary files shipped inside the pack. Actions (e.g. `symlink`) reference them via paths **relative to the pack root**: `files/config.yaml`, `files/themes/default.toml`. grex resolves these against the pack's workdir at runtime.

### `.state/` runtime cache

Gitignored. Holds per-pack runtime cache (lock markers, resolved deps, per-platform resolution memo). `grex doctor --compact` may prune this.

## The 3 built-in pack-types

### `meta`

Nests children only. Has no own actions. Lifecycle:

- `install` = clone all children, recursively dispatch their pack-type's install.
- `sync` = git pull self, then recurse into children's sync.
- `update` = sync + dispatch children's update if lockfile SHA changed.
- `teardown` = recurse children teardown, then remove self dir (if owned).

```yaml
schema_version: "1"
name: dev-env
type: meta
children:
  - url: git@github.com:user/warp-cfg
    path: warp-cfg
  - url: git@github.com:user/fonts-pack
    path: fonts
```

### `declarative`

Runs `actions` list from `pack.yaml` in order. All actions are idempotent (or gated by `require`). May also have children.

- `install` = run `actions` top-to-bottom under the current OS.
- `sync` = git pull self, then recurse into children. `actions` re-run only if lockfile SHA changed (covered by `update`).
- `update` = sync + re-run `actions` if lockfile delta.
- `teardown` = run `teardown:` list if present; else reverse-order rollback of `actions`.

```yaml
schema_version: "1"
name: warp-cfg
type: declarative
version: "0.2.0"

actions:
  - require:
      any_of:
        - cmd_available: git
        - os: windows
      on_fail: error

  - when:
      os: windows
      actions:
        - mkdir: { path: "$HOME/.warp" }
        - symlink:
            src: files/config.yaml
            dst: "$HOME/.warp/config.yaml"
            backup: true
            normalize: true
        - env:
            name: WARP_HOME
            value: "$HOME/.warp"
            scope: user

  - when:
      os: macos
      actions:
        - symlink:
            src: files/config.yaml
            dst: "$HOME/Library/Application Support/warp/config.yaml"

teardown:
  - rmdir: { path: "$HOME/.warp", backup: true }
```

### `scripted`

Escape hatch. Runs `.grex/hooks/{setup,sync,teardown}.{sh,ps1}` on the matching OS. grex picks `.ps1` on Windows, `.sh` on Linux/macOS. If the expected hook is absent for the current OS, the lifecycle phase no-ops.

- `install` = run `hooks/setup.{sh,ps1}` with cwd = pack workdir.
- `sync` = git pull self, then run `hooks/sync.{sh,ps1}` if present.
- `update` = sync + rerun setup if lockfile delta (no-op if no setup hook).
- `teardown` = run `hooks/teardown.{sh,ps1}` if present.

Hooks receive env vars: `GREX_PACK_NAME`, `GREX_PACK_PATH`, `GREX_PACK_OS`, `GREX_DRY_RUN`.

Exit code non-zero = failure (propagates).

```yaml
schema_version: "1"
name: legacy-vim
type: scripted
# hooks/ directory ships setup.sh, setup.ps1, teardown.sh, teardown.ps1
```

## Validation rules

- `schema_version` must be exactly `"1"`.
- `type` must be one of the 3 built-ins (or a registered plugin name when the plugin is loaded).
- `type` in `.grex/pack.yaml` is the **authoritative** source of truth. Runtime manifest / lockfile entries record `type` as an **observed snapshot** only. On disagreement (manifest `type` ≠ pack.yaml `type`), pack.yaml wins and the manifest is corrected on the next sync. See [manifest.md](./manifest.md#type-field-authority).
- `name` regex: `^[a-z][a-z0-9-]*$` (letter-led; digits allowed in later positions).
- `children[].path` must be bare name (no `/` or `\`).
- Unknown top-level keys rejected unless prefixed with `x-` (user annotations).
- Unknown action keys rejected unless the plugin is registered.
- Empty lists are VALID: `actions: []`, `children: []`, `depends_on: []`, `teardown: []` all parse cleanly. Empty `actions` in a `declarative` pack is a no-op install. Empty `children` in a `meta` pack is a no-op sync. Do not reject empty lists.
- Duplicate `symlink.dst` within the same pack is a **validation error, caught at plan phase** (before execution). Two or more `symlink` actions resolving to the same absolute `dst` path abort the plan with `ActionArgsInvalid`. Cross-pack duplicates are handled by conflict detection at the workspace level (separate concern; see [concurrency.md](./concurrency.md)).
- YAML anchors (`&name`) and aliases (`*name`) are **REJECTED** during parse. Rationale: prevents billion-laughs / alias-bomb DoS. Implementation: parser config disables alias resolution, or the loader detects and errors before expansion.

`grex doctor` runs these checks on every registered pack.

## Opacity rule

grex reads only `.grex/`. It never inspects or touches content outside it. Pack authors may store anything adjacent — scripts, assets, source — and grex stays agnostic.

## Relationship to the workspace manifest

A workspace (the directory where you run `grex init`) is itself a git repo. It has its own `grex.jsonl` + `grex.lock.jsonl` tracking which packs are registered. A workspace does **not** need its own `.grex/pack.yaml` unless it is also meant to be published as a pack.
