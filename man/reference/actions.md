# actions

7 Tier 1 action primitives. Grounded in observed real-world script patterns (see [goals.md](../concepts/goals.md) grounded-reality table). Each is a native Rust built-in registered as an `ActionPlugin` at compile time.

## Action invocation shape

In `pack.yaml`:

```yaml
actions:
  - <action-name>:
      <arg>: <value>
      ...
```

Or for actions that take a bare argument object:

```yaml
actions:
  - mkdir: { path: "$HOME/.warp" }
```

grex parses each entry, looks up the action by key in the registry, and dispatches to its `ActionPlugin::execute`.

## Variable expansion

Action args support env-var interpolation: `$HOME`, `$USER`, `$APPDATA`, `$LOCALAPPDATA`, `${NAME}`. Expansion is done by grex in the `PackCtx::env` resolver — native-per-platform:

- POSIX: standard `$VAR`, case-sensitive.
- Windows: `$VAR` works, plus `%VAR%` for legacy paths. `$HOME` maps to `%USERPROFILE%` (fallback applied on `VarEnv::from_os` / `from_map` only — NOT on an explicit `insert`). Lookup is **ASCII-case-insensitive** via a secondary lowercase-keyed index; `$UserProfile` and `$USERPROFILE` resolve to the same value.

### Escape syntax

- POSIX form: a literal `$` is written as `$$`. `$${HOME}` expands to the literal string `${HOME}` (no expansion).
- Windows form: a literal `%` is written as `%%`. `%%USERNAME%%` expands to the literal string `%USERNAME%`.
- Backslash escapes (`\$`, `\%`) are **not** supported.

```yaml
- env:
    name: GREX_DOC_EXAMPLE
    value: "literal $${HOME} and %%USERNAME%%"   # → literal ${HOME} and %USERNAME%
```

## The 7 primitives

### 1. `symlink`

Create or update a symlink, with optional backup of any existing dst.

```yaml
- symlink:
    src: files/config.yaml     # relative to pack workdir
    dst: "$HOME/.warp/config.yaml"
    backup: true               # default false; renames existing dst to <dst>.grex-bak.<ts>
    normalize: true            # default true; absolute-normalizes both paths
    kind: auto                 # auto | file | directory; Windows needs explicit for dir symlinks
```

| Field | Type | Default | Notes |
|---|---|---|---|
| `src` | path | required | Resolved relative to pack workdir. |
| `dst` | path | required | May contain env vars. |
| `backup` | bool | false | Renames existing dst before creating symlink. |
| `normalize` | bool | true | Canonicalizes both sides. |
| `kind` | enum | auto | `auto` infers from src; `directory` forced on Windows for dir links. |

**Cross-platform**: uses `std::os::unix::fs::symlink` on POSIX, `std::os::windows::fs::{symlink_file, symlink_dir}` on Windows. Requires Developer Mode or SeCreateSymbolicLink privilege on Windows; `require` gate recommended.

**`kind: auto` with missing src**: if `kind` is `auto` and `src` does not exist at execute time, grex errors with `SymlinkAutoKindUnresolvable` rather than defaulting to `file`. A dangling file-symlink where a directory was required is worse than a loud failure.

**Idempotency**: if dst is already a symlink pointing at src, no-op (`changed: false`).

**Rollback**: removes the symlink; if a backup was made, restores it.

**Backup + create atomicity**: when `backup: true` is set and dst exists, grex renames `dst → <dst>.grex.bak` **then** creates the symlink. If the rename succeeds but the create fails, grex renames the backup back to `dst` (best-effort restore). If the restore rename also fails, grex surfaces `SymlinkCreateAfterBackupFailed` — the user is told exactly what is on disk (backup at `<dst>.grex.bak`, no symlink at `dst`) so manual recovery is unambiguous.

**Errors**: src missing, dst parent missing, privilege denied, `SymlinkAutoKindUnresolvable` (see above), `SymlinkCreateAfterBackupFailed` (see above).

**Duplicate `dst` within a pack**: two or more `symlink` actions in the same pack whose resolved `dst` paths are equal is a **plan-phase validation error** (`ActionArgsInvalid`), raised before any action executes. On case-insensitive filesystems (Windows, macOS default APFS) the comparison is ASCII-case-folded so `C:\Users\a\x` and `c:\users\a\X` are detected as duplicates. Cross-pack collisions on the same `dst` are handled separately by workspace-level conflict detection. See [pack-spec.md §Validation rules](./pack-spec.md#validation-rules).

### 2. `env`

Set an environment variable.

```yaml
- env:
    name: WARP_HOME
    value: "$HOME/.warp"
    scope: user                # user | machine | session
```

| Field | Type | Default | Notes |
|---|---|---|---|
| `name` | string | required | Variable name. |
| `value` | string | required | Expanded before setting. |
| `scope` | enum | `user` | `user` persists to shell rc / registry HKCU; `machine` → HKLM / `/etc/environment` (requires admin); `session` → current process only. |

**Platform**:
- Windows: `user` writes `HKCU\Environment` + broadcasts `WM_SETTINGCHANGE`. `machine` writes `HKLM\System\CurrentControlSet\Control\Session Manager\Environment`.
- POSIX: `user` appends managed-block to `~/.bashrc` / `~/.zshrc` / `~/.config/fish/config.fish`. `machine` writes `/etc/environment`.
- `session` uses `std::env::set_var` (doesn't persist).

**Idempotency**: re-read current value; no-op if already set.

**Rollback**: restores previous value if captured; else unsets.

### 3. `mkdir`

Create a directory, including parents.

```yaml
- mkdir: { path: "$HOME/.warp" }
```

| Field | Type | Default | Notes |
|---|---|---|---|
| `path` | path | required | Expanded. |
| `mode` | string | "755" (POSIX) | Ignored on Windows. |

**Idempotency**: no-op if already a directory.

**Errors**: path exists as non-directory.

**Rollback**: if grex created it, remove it (only if empty).

### 4. `rmdir`

Remove a directory, optionally with backup.

```yaml
- rmdir:
    path: "$HOME/.warp"
    backup: true               # default false; renames to <path>.grex-bak.<ts>
    force: false               # default false; if false, refuses non-empty unless backup
```

| Field | Type | Default | Notes |
|---|---|---|---|
| `path` | path | required | Expanded. |
| `backup` | bool | false | Renames rather than deleting. |
| `force` | bool | false | Allow recursive delete of non-empty. |

**Idempotency**: no-op if already absent.

**Rollback**: restores backup if one was made; else creates empty dir (best-effort).

### 5. `require`

Prerequisite / idempotency gate. Evaluates predicates; on failure, aborts or skips per `on_fail`.

```yaml
- require:
    all_of:                    # or any_of / none_of
      - cmd_available: git
      - os: windows
      - psversion: ">=5.1"
    on_fail: error             # error | skip | warn
```

Predicates:

| Predicate | Arg | Meaning |
|---|---|---|
| `path_exists` | path | Filesystem path present. |
| `cmd_available` | name | `name` in `PATH`. |
| `reg_key` | `hive\path!name` | Registry value present (Windows only; off-platform a leaf evaluation yields `PredicateNotSupported`). Forward-slash separators (`HKCU/Software/X`) are accepted and normalized to `\`. ACL-denied or transient registry I/O surfaces as `PredicateProbeFailed` rather than collapsing to `false`. |
| `os` | `windows`\|`linux`\|`macos` | Current OS matches. |
| `psversion` | version-spec | PowerShell version constraint (Windows only; off-platform a leaf evaluation yields `PredicateNotSupported`). Probe is bounded by a 5 s timeout, prefers the absolute `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe` path to resist PATH-hijack, compares the full `(major, minor)` tuple, and surfaces non-zero exit / timeout / unexpected I/O as `PredicateProbeFailed`. `powershell.exe` genuinely missing degrades to `false` (matches the `reg_key` NotFound shape). |
| `symlink_ok` | — | Privilege / dev-mode present to create symlinks. |

Combiners: `all_of` (AND), `any_of` (OR), `none_of` (NOT). Nest freely. Inside these combiners (and inside `when`'s `all_of` / `any_of` / `none_of` lists) a leg that yields `PredicateNotSupported` is treated as `false` so other legs still get a chance — this preserves the cross-platform rescue pattern `any_of: [{reg_key: ...}, {path_exists: /etc/foo}]`. The *top-level* `combiner` attached to a `require` stays strict: a single unsupported leaf under `require` still bubbles the typed error.

`on_fail`:
- `error` → abort pack install with non-zero exit.
- `skip` → remaining actions in this pack skipped, lifecycle reports "skipped".
- `warn` → log warning, continue.

**Observed frequency**: 9 uses in the scanned scripts. Highest-leverage primitive.

### 6. `when`

Platform / conditional gate wrapping nested actions. Sugar over `require` for common platform dispatch.

```yaml
- when:
    os: windows                # or: any_of / all_of / none_of
    actions:
      - mkdir: { path: "$HOME/.warp" }
      - symlink: { src: files/config.yaml, dst: "$HOME/.warp/config.yaml" }
```

| Field | Type | Default | Notes |
|---|---|---|---|
| `os` | string | — | Shorthand for `require { os: ... }`. |
| `all_of`/`any_of`/`none_of` | list | — | Full predicate combiner support. |
| `actions` | list | required | Nested actions; run only if condition holds. |

On condition false: all nested actions are skipped (not failures). No rollback needed — nothing ran.

**Combiner precedence**: when `os` and any of `all_of`/`any_of`/`none_of` appear together, they compose **conjunctively** (AND). `os:` is shorthand equivalent to an `os:` predicate inside an implicit `all_of`; the explicit combiners are appended to that same `all_of`. Mixed example:

```yaml
- when:
    os: windows
    all_of:
      - cmd_available: pwsh
      - psversion: ">=7.0"
    actions:
      - exec: { cmd: ["pwsh", "-NoProfile", "-File", "files/setup.ps1"] }
```

Both the `os: windows` shorthand and every predicate under `all_of` must hold for the nested actions to run.

### 7. `exec`

Shell escape. Runs a command. **Array form by default** (no shell interpretation). Opt into shell parsing explicitly.

```yaml
- exec:
    cmd: ["rclone", "copy", "gdrive:backup", "$HOME/backup"]
    cwd: "$HOME"               # default: pack workdir
    env:                       # extra env vars for this invocation
      RCLONE_CONFIG: "$HOME/.config/rclone/rclone.conf"
    shell: false               # default false; true = parse via sh -c / cmd /c
    on_fail: error             # error | warn | ignore
```

| Field | Type | Default | Notes |
|---|---|---|---|
| `cmd` | list[string] | required (when shell=false) | argv array. |
| `cmd_shell` | string | required (when shell=true) | Single string passed to shell. |
| `cwd` | path | pack workdir | Where to run. |
| `env` | map | {} | Extra env vars. |
| `shell` | bool | false | Enable shell interpretation. |
| `on_fail` | enum | `error` | Error propagation. |

**Rule**: `exec` is the last-resort primitive. If you find yourself writing a second `exec` in the same pack, consider promoting the logic to a purpose-built action (built-in or plugin).

**No idempotency guarantee**. grex does not know whether the command you ran is repeatable. Pair with `require` to gate it.

**Rollback**: none (grex cannot know how to undo arbitrary commands). Pack authors wanting true rollback must pair with a teardown action.

**stderr capture on failure**: when `exec` returns a non-zero status (and `on_fail: error`), grex records the failure as `ExecNonZero` and attaches a **truncated** copy of the command's stderr — capped at 2 KiB — to the manifest `action_halted` event. The cap bounds manifest event size to stay below the fd-lock append atomicity ceiling (see [manifest.md §Atomic append](./manifest.md#atomic-append)). Full stderr is printed to the terminal regardless; only the manifest copy is truncated.

## Observed-pattern → primitive mapping

From the E:\repos scan (3 PowerShell scripts, 945 LOC):

| Observed pattern | Count | v1 primitive | Notes |
|---|---|---|---|
| `New-Item -ItemType SymbolicLink` / `ln -s` | 8 | `symlink` | Direct mapping. |
| `if (Test-Path …) { … }` idempotency guards | 9 | `require` | `path_exists` or `cmd_available` predicate. |
| `[Environment]::SetEnvironmentVariable(…, 'User')` | 7 | `env` (scope: user) | Direct. |
| `& ./install.ps1` chain scripts | 5 | `exec` | Temporary; plugin should replace long-term. |
| `New-Item -ItemType Directory -Force` | 2 | `mkdir` | Direct. |
| `if ($IsWindows) { … }` platform gate | 2 | `when` | Direct. |
| `Rename-Item` backup then `Remove-Item -Recurse` | 1 | `rmdir` (backup: true) | Direct. |

No observed: package installs (`winget`, `choco`), JSON merges, archive extracts, template rendering. Those are real patterns but not in this sample. Deferred to v2 plugin contributions.

## Action plugin registration

Built-ins register via the canonical `register_builtins(&mut Registry)` free function called from `Registry::bootstrap()` (decision 2026-04-20). `inventory::submit!` auto-registration is feature-gated behind `plugin-inventory` (default off) and lands in Stage M4-E. User-facing YAML keys resolve through the registry name-to-plugin map.

Full trait definition, registration details, and v2 external-loading path: [plugin-api.md](./plugin-api.md).

## Error taxonomy

| Error | Cause | Recovery |
|---|---|---|
| `ActionArgsInvalid` | Malformed YAML for action. | Fix `pack.yaml`. |
| `ActionPreconditionFailed` | `require` predicate false with `on_fail: error`. | Fix environment or pack. |
| `ActionExecutionFailed` | Runtime error during action. | Pack-type rollback invoked. |
| `ActionUnknown` | Action key not registered. | Plugin missing. Exit 8. |
| `PredicateNotSupported` | Predicate (`reg_key` / `psversion`) is platform-specific and the current platform cannot answer it. Inside `all_of` / `any_of` / `none_of` combiners this is tolerated as `false`; at the top-level `require` it is fatal. | Wrap with `when: { os: windows }` or use `any_of` with a cross-platform fallback leg. |
| `PredicateProbeFailed` | The probe ran on the correct platform but itself broke — non-zero `powershell.exe` exit, 5 s timeout, ACL-denied registry read, or other OS I/O that is not a plain NOT_FOUND. Always fatal. | Investigate the probe error (AV hook, WinRM stall, ACL). Not rescued by combiner tolerance — a broken probe is not a rescue-eligible condition. |

All actions return `Result<ExecStep, ExecError>` to the pack-type driver (v1 shape, 2026-04-20; see [plugin-api.md](./plugin-api.md)); the driver aggregates failures and triggers rollback per pack-type policy.
