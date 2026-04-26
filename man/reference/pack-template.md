# Pack template

grex ships a reference pack at `examples/pack-template/` in the main repo.
At v1.0.0 release time, the in-tree tree is mirrored to a standalone
repo (`git@github.com:egoisth777/grex-pack-template.git`) so users can
install it via `grex add <URL>`; until then, use the in-tree form below.

## Trying the template

From a checkout of the main grex repo:

```sh
grex init
# Local (in-tree) form — works today, no external repo required:
grex add --from-path examples/pack-template
grex sync
grex doctor
```

Once grex v1.0.0+ is published, you'll also be able to install via the
standalone mirror:

```sh
# Available at v1.0.0+ release; until then use the --from-path form above.
grex add git@github.com:egoisth777/grex-pack-template.git
```

Expected behaviour: the pack creates `$HOME/.grex-pack-template/` and a
symlink inside it pointing at the pack's `files/hello.txt`. Re-running
`grex sync` is a no-op — every action is idempotent.

To undo: `grex teardown grex-pack-template` (or `grex rm grex-pack-template`
to also remove it from the workspace manifest). The directory is backed up
under `<path>.grex-bak.<ts>` before removal.

## Walkthrough of the manifest

The template is `type: declarative` — the simplest of grex's three pack
types. Its `pack.yaml` is structured as:

1. **`require`** — gate the pack. If `git` is unavailable and the OS is not
   Windows, the install aborts before any filesystem action runs.
2. **`mkdir` + `symlink`** — a single pair of actions, portable across
   linux / macos / windows via `$HOME`. grex-core's var-expansion
   synthesises `$HOME` from `%USERPROFILE%` on Windows (see
   `crates/grex-core/src/vars/mod.rs`), so no per-OS `when:` fan-out is
   required.
3. **`teardown:`** — a single `rmdir` that reverses the install. Without an
   explicit `teardown` list, grex would default to reverse-order rollback
   of `actions`, which works but is less readable.

Every action is chosen for idempotency on repeat syncs: `require` is
read-only, `mkdir` no-ops when the path exists, `symlink` no-ops when dst
already points at src.

## Structure of the in-tree copy

```
examples/pack-template/
├── .grex/
│   └── pack.yaml            # manifest (schema_version "1", type declarative)
├── files/
│   └── hello.txt            # payload referenced by the symlink action
├── README.md                # user-facing docs (Install / Structure / Customisation / Testing / Licence)
└── .gitignore               # M6 managed-block: .grex/.state/
```

The template is `type: declarative`, so it has no `.grex/hooks/` directory.
Hooks fire only for `type: scripted` packs.

## Customising the template for your own pack

1. Fork the tree into a new git repo.
2. Rename `name:` in `pack.yaml` (regex `^[a-z][a-z0-9-]*$`).
3. Replace the actions with your own — see the [actions
   reference](./actions.md) for the 7 built-in primitives.
4. If you need arbitrary shell steps that don't fit the declarative
   primitives, switch the manifest to `type: scripted` and add a
   `.grex/hooks/` directory with `setup.{sh,ps1}` / `sync.{sh,ps1}` /
   `teardown.{sh,ps1}` scripts. Hooks receive `GREX_PACK_NAME`,
   `GREX_PACK_PATH`, `GREX_PACK_OS`, and `GREX_DRY_RUN` as env vars.
5. Update the `teardown:` list to reverse your actions.
6. Publish and install with `grex add <your-url>`.

## CI validation

The in-tree copy is the canonical source and is exercised in CI by
`crates/grex/tests/pack_template_smoke.rs`. The smoke test:

- Parses `examples/pack-template/.grex/pack.yaml` via
  `grex_core::pack::parse` and asserts the top-level shape (name / type /
  schema_version / first-action is a `require` gate).
- Asserts the payload files the README promises (`.grex/pack.yaml`,
  `files/hello.txt`, `README.md`, `.gitignore`) are present on disk.
- Copies the template into a tempdir and runs `grex_core::sync::run`
  against it end-to-end, then re-runs sync to verify the second pass is
  an all-no-op.

If any check fails in CI, the template is broken — fix the in-tree copy
before the next release, since the external mirror is regenerated from it
(see the appendix below).

## Relationship to other M8 stages

- **M8-1 (cargo-dist)**: the installer scripts referenced in the template's
  README live on the main grex releases page, not on the template repo.
- **M8-2 (crates.io)**: the template has no crates.io presence — it is a
  git-installable reference pack, not a Rust crate.
- **M8-3 (mdBook)**: this chapter *is* the authoritative doc for the
  template's ownership contract.
- **M8-5 (CHANGELOG)**: every release that changes the template must note
  it in the main grex CHANGELOG entry, plus re-mirror per the appendix.

## Appendix: publishing the external mirror (release-time procedure)

Run these steps **once per major grex release** (v1.0.0, v1.1.0, v2.0.0, ...):

1. **Create the empty GitHub repo.** On github.com: new repo
   `egoisth777/grex-pack-template`, public, MIT OR Apache-2.0 licence,
   empty (no README / .gitignore / licence auto-init — we push our own).

2. **Mirror the in-tree tree into a fresh git history.** From the grex repo
   root (replace `v1.0.0` with the actual release tag):

   ```sh
   cp -r examples/pack-template /tmp/grex-pack-template
   cd /tmp/grex-pack-template
   git init -b main
   git add -A
   git commit -m "feat: initial template from grex v1.0.0"
   git remote add origin git@github.com:egoisth777/grex-pack-template.git
   git push -u origin main
   ```

   On Windows, substitute `$env:TEMP` for `/tmp` and use PowerShell-native
   `Copy-Item -Recurse`.

3. **Tag the external repo to match the grex release.**

   ```sh
   git tag -a v1.0.0 -m "grex v1.0.0"
   git push origin v1.0.0
   ```

4. **Verify end-to-end.** From a fresh workspace:

   ```sh
   grex init
   grex add git@github.com:egoisth777/grex-pack-template.git
   grex sync
   grex doctor
   ```

   Expected: all four commands exit 0; `grex doctor` reports the pack as OK.

5. **Record the first-commit SHA** in the main grex repo's `CHANGELOG.md`
   under the release entry, for traceability.

### Ownership & CODEOWNERS

- **In-tree copy** (`examples/pack-template/`) is governed by the main grex
  `CODEOWNERS` — same reviewers as the rest of the workspace.
- **External repo** (`grex-pack-template`) has its **own** `CODEOWNERS` file,
  independent of main grex. Day-to-day PRs on the external repo (typo fixes,
  user-reported issues) land directly; breaking changes to the template
  shape MUST land in the in-tree copy first, ship with the next grex
  release, and then be force-pushed over the external repo as a new commit
  history per step 2 above.
- **Never hand-edit the external repo and the in-tree copy independently.**
  The in-tree copy is canonical; the external repo is regenerated.
