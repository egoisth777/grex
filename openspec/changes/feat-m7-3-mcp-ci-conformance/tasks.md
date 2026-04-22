# feat-m7-3 — tasks (CI is the oracle)

**Convention**: bring the validator up locally first; deliberate-regression smoke proves the gate actually gates.

## Stage 1 — Validator pin

- [ ] Confirm correct package name on PyPI: `mcp-protocol-validator` (NOT `mcp-validator`).
- [ ] Find latest stable release on PyPI + matching commit SHA on `github.com/Janix-AI/mcp-protocol-validator`.
- [ ] Document the pin (version + SHA) either as a comment in `ci.yml` or in `docs/ci/mcp-conformance.md`.
- [ ] Local bring-up: `pip install 'mcp-protocol-validator==X.Y.Z'` in a venv; run against `target/release/grex serve --protocol-version 2025-06-18`; assert exit 0.

## Stage 2 — Release build + cache

- [ ] Confirm current `.github/workflows/ci.yml` `build` matrix runs debug only (no release artefact).
- [ ] Add a self-contained `cargo build --release -p grex` step inside the new `mcp-conformance` job (do NOT modify the existing `build` job).
- [ ] Configure `Swatinem/rust-cache@v2` with a distinct `key: release` so the release target doesn't thrash the debug cache.

## Stage 3 — Wire validator invocation

- [ ] Append `mcp-conformance` job to `.github/workflows/ci.yml` per spec §Design.
- [ ] Server command: `"$GITHUB_WORKSPACE/target/release/grex serve"`.
- [ ] Protocol version: `2025-06-18`.
- [ ] `needs: [build]` — verify this resolves against the existing matrix job name.
- [ ] Upload validator log as artefact on failure (always).
- [ ] Push to a throwaway branch; verify job runs, is green on baseline, and fails loudly on a deliberate handshake regression → revert.

## Stage 4 — Branch protection + bypass doc

- [ ] Maintainer: mark `mcp-conformance` as a required status check on `main` via GitHub branch-protection settings.
- [ ] Document the bypass procedure in `docs/ci/mcp-conformance.md` §Bypass (how to temporarily remove the required check if the validator itself regresses).
- [ ] Update `progress.md` with commit SHA + CI green confirmation.
