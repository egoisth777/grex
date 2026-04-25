# Release process

How to cut a `grex` release. Covers the GitHub Release (binaries via
cargo-dist) and the crates.io publish steps. Rollback procedure at the end.

Audience: maintainers. Users should install per [README.md §Install](https://github.com/egoisth777/grex/blob/main/README.md#install).

## Prerequisites

- Push access to `main` and tag-push rights on `egoisth777/grex`.
- A crates.io API token with publish rights on `grex-cli`, `grex-core`,
  `grex-mcp`, `grex-plugins-builtin` (`cargo login` on your workstation).
- `cargo-dist` installed locally at the pinned version matching
  `[workspace.metadata.dist].cargo-dist-version` (currently `0.31.0`) —
  only required if you want to re-run `dist plan` before tagging.
- Clean `git status`; working tree must match the exact commit you are
  releasing. No un-committed changes.

## 1. Prepare the CHANGELOG

In `CHANGELOG.md`:

1. Rename the `[Unreleased - 1.0.0]` heading to `[1.0.0] - YYYY-MM-DD`
   using today's UTC date.
2. Open a new empty `[Unreleased]` section above it with empty
   `Added` / `Changed` / `Fixed` / `Removed` subsections.
3. Ensure every `Added` bullet references the PR that introduced it.
4. Commit: `git commit -am "chore(release): prepare v1.0.0"`.
5. Push to `main` via the normal PR flow. Do NOT tag yet.

## 2. Tag and push

Once the `chore(release): prepare v1.0.0` commit is on `main`:

```sh
git switch main
git pull --ff-only
git tag -a v1.0.0 -m "grex v1.0.0"
git push origin v1.0.0
```

The tag push triggers `.github/workflows/release.yml`:

- `plan` — validates `dist-manifest.json` against the 5 targets.
- `build-local-artifacts` × 5 — builds `grex` for each target, signs
  artefacts via GitHub's native attestations (`actions/attest-build-provenance`).
- `build-global-artifacts` — produces the `installer.sh` + `installer.ps1`
  scripts and SHA-256 sums.
- `host` + `announce` — creates the GitHub Release and uploads all
  artefacts (`.tar.xz` / `.zip` / `*.sha256` / installers / `source.tar.gz`).

The GitHub Release body is auto-extracted from the `[1.0.0]` section of
`CHANGELOG.md` (cargo-dist convention).

## 3. Publish to crates.io (manual)

cargo-dist does NOT publish to crates.io. Do this **manually** from a
checkout of the tagged commit, in strict topological order. Prefer
`--wait-for-publish` (cargo 1.66+) over a hand-timed `sleep` — it polls
the index and only exits once the crate is actually resolvable:

```sh
git switch --detach v1.0.0

cargo publish --wait-for-publish --timeout 300 -p grex-core
cargo publish --wait-for-publish --timeout 300 -p grex-plugins-builtin
cargo publish --wait-for-publish --timeout 300 -p grex-mcp
cargo publish --wait-for-publish --timeout 300 -p grex-cli
```

Order rationale: `grex-plugins-builtin` and `grex-mcp` both depend on
`grex-core`; `grex-cli` depends on all three. See
[`openspec/changes/feat-m8-release/crates-io-names.md`](../openspec/changes/feat-m8-release/crates-io-names.md)
§2 for the dep graph.

**Smoke test post-publish:**

```sh
cargo install grex-cli --locked
grex --version   # must print 1.0.0
```

## 4. Installer smoke tests

From a fresh shell session:

```sh
# Linux / macOS
curl -LsSf https://github.com/egoisth777/grex/releases/latest/download/grex-cli-installer.sh | sh
grex --version

# Windows
powershell -c "irm https://github.com/egoisth777/grex/releases/latest/download/grex-cli-installer.ps1 | iex"
grex --version
```

### Verified install (recommended for security-sensitive environments)

Every artefact is signed via GitHub's native build provenance
(`actions/attest-build-provenance`). Users can verify the binary matches
the commit + workflow that produced it before trusting it:

```sh
# Download + verify attestation (requires gh CLI >= 2.49)
gh release download v1.0.0 --repo egoisth777/grex --pattern '*.tar.xz'
gh attestation verify grex-cli-x86_64-unknown-linux-gnu.tar.xz --repo egoisth777/grex
tar xf grex-cli-x86_64-unknown-linux-gnu.tar.xz
sudo mv grex-cli*/grex /usr/local/bin/
grex --version
```

The `curl | sh` / `irm | iex` one-liners above are a convenience path
and do NOT verify attestations.

### Supported platforms

Pre-built binaries ship for these five triples (see
`[workspace.metadata.dist].targets` in root `Cargo.toml`):

| Triple                         | Runner            |
|--------------------------------|-------------------|
| `x86_64-unknown-linux-gnu`     | `ubuntu-22.04`    |
| `aarch64-unknown-linux-gnu`    | `ubuntu-22.04-arm`|
| `x86_64-apple-darwin`          | `macos-13`        |
| `aarch64-apple-darwin`         | `macos-14`        |
| `x86_64-pc-windows-msvc`       | `windows-2022`    |

Everything else (32-bit, musl, FreeBSD, aarch64-windows, etc.) falls
back to building from source:

```sh
cargo install grex-cli --locked
```

## Rollback

### Yank a bad crates.io release

`cargo yank` hides the version from the resolver without deleting it.
Yank in reverse-dependency order (bin first, so dependents cannot keep
pulling it in):

```sh
cargo yank --version 1.0.0 grex-cli
cargo yank --version 1.0.0 grex-mcp
cargo yank --version 1.0.0 grex-plugins-builtin
cargo yank --version 1.0.0 grex-core
```

Yanking is reversible: `cargo yank --version 1.0.0 --undo <crate>` if
you decide to keep the release after all.

### Mark the GitHub Release as pre-release

```sh
gh release edit v1.0.0 --prerelease
```

This hides it from the "latest" installer URL without deleting the
artefacts. Users on the installer one-liner will stop picking up the bad
release automatically.

### Ship a fix

Cut a fresh patch release (`v1.0.1`) with the fix — do not re-tag
`v1.0.0`. Re-tagging breaks provenance and every cached copy of the
installer script.

### On the limits of rollback

- **`cargo yank` is not `cargo delete`.** The crate file stays on
  crates.io forever; yanking only excludes it from new resolves. Code
  that pinned `= 1.0.0` in a lockfile keeps compiling. There is no
  delete API.
- **Sigstore attestations are immutable.** A released artefact whose
  build provenance is on the Sigstore transparency log cannot be
  revoked — `gh attestation verify` will keep returning `OK` even after
  you mark the release pre-release.
- **Compromised-binary rollback MUST use a patch bump** (`v1.0.1`) that
  supersedes the bad version. Yank `v1.0.0`, mark its GitHub Release
  pre-release, and push `v1.0.1` through the same release pipeline. Do
  not attempt to re-tag or delete artefacts.

## Pinning updates

To update the pinned `cargo-dist` version:

1. Bump `[workspace.metadata.dist].cargo-dist-version` in root `Cargo.toml`.
2. Run `cargo install cargo-dist --locked --version <new>` locally.
3. Run `dist generate` to regenerate `.github/workflows/release.yml`.
4. Commit both files together. CI's `release-plan` job verifies the
   manifest still parses.
