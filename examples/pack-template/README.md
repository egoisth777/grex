# grex-pack-template

Reference pack for [grex](https://github.com/egoisth777/grex) v1.0.0. Shows the
minimal-but-complete shape of a pack: a `.grex/pack.yaml` manifest plus an
opaque payload directory. Dual-licensed MIT OR Apache-2.0, matching the
main grex repo.

## What this pack does

On `grex sync`, the pack runs three idempotent actions under the user's home:

1. `require` — verifies `git` is on PATH (or the OS is Windows).
2. `mkdir` — creates `$HOME/.grex-pack-template/` if absent.
3. `symlink` — links `files/hello.txt` to `$HOME/.grex-pack-template/hello.txt`.

Running `grex sync` a second time is a no-op: the directory already exists and
the symlink already points at the same source. `grex teardown
grex-pack-template` removes both (backing up the directory under
`<path>.grex-bak.<ts>`).

## Install

Lead with the in-tree form — works today, no external repo required:

```sh
# Local (in-tree) form — always available:
grex add --from-path examples/pack-template

# Or, equivalently:
grex add "file://$(pwd)/examples/pack-template"
grex sync
```

The clone form below becomes available once grex v1.0.0 ships:

```sh
# Available at v1.0.0+ release; until then use the file:// form above.
grex add git@github.com:egoisth777/grex-pack-template.git
```

See the mdBook "Pack template" chapter (source:
[`man/reference/pack-template.md`](../../man/reference/pack-template.md))
for the ownership / publishing contract between the in-tree copy and the
external mirror.

## Structure

```
examples/pack-template/
├── .grex/
│   └── pack.yaml            # manifest (type: declarative)
├── files/
│   └── hello.txt            # payload referenced by the symlink action
├── README.md                # this file
└── .gitignore               # grex-managed block demo (.grex/.state/)
```

Everything outside `.grex/` is opaque payload from grex's point of view — only
`.grex/` is contract. `files/` is the conventional payload location referenced
by actions (the `symlink.src` field is pack-relative).

This template is `type: declarative`, so it has no `.grex/hooks/` directory.
Hooks fire only for `type: scripted` packs — see the customisation steps
below if you need arbitrary shell steps.

## Customisation

Fork this directory into a new git repo to publish your own pack:

1. Rename `name:` in `.grex/pack.yaml` (slug-like, must match
   `^[a-z][a-z0-9-]*$`).
2. Replace the actions in the manifest. The 7 action primitives are
   `symlink`, `env`, `mkdir`, `rmdir`, `require`, `when`, `exec`. See
   [`.omne/cfg/actions.md`](../../.omne/cfg/actions.md) in the main grex repo
   (published at https://egoisth777.github.io/grex/actions.html).
3. If you need arbitrary shell steps that don't fit the declarative
   primitives, switch `type: declarative` to `type: scripted` and add a
   `.grex/hooks/` directory with `setup.{sh,ps1}` / `sync.{sh,ps1}` /
   `teardown.{sh,ps1}` scripts. Hooks receive `GREX_PACK_NAME`,
   `GREX_PACK_PATH`, `GREX_PACK_OS`, and `GREX_DRY_RUN` as env vars.
4. Update the teardown list to reverse your actions (or remove the
   `teardown:` block to accept grex's default reverse-order rollback).

## Testing

Before publishing, validate the manifest:

```sh
# Parse + validation only (no execution)
grex doctor --lint-config --path ./examples/pack-template
```

A dry-run sync shows the plan without executing it:

```sh
grex sync --dry-run
```

## Licence

Dual-licensed under **MIT OR Apache-2.0**, matching the main grex repo. See
the main repo's `LICENSE-MIT` and `LICENSE-APACHE` for full text.
