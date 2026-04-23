# feat-m7-3 — tasks (CI is the oracle)

**Convention**: bring the validator up locally first; deliberate-regression smoke proves the gate actually gates.

## Stage 1 — Validator pin (resolved at draft time)

Pin locked to `mcp-validator==0.3.1` / SHA `d766d3ee94076b13d0b73253e5221bbc76b9edb2` (tag `v0.3.1`, published 2025-07-08 per `gh api repos/Janix-ai/mcp-validator/releases/latest` lookup on 2026-04-21). Canonical PyPI name is `mcp-validator`; repo slug is `Janix-ai/mcp-validator` (lowercase `ai`).

- [ ] Verify pin is reachable: `pip install 'mcp-validator==0.3.1'` in a throwaway venv succeeds and `mcp-validator --help` prints.
- [ ] Fallback if PyPI distribution disappears: `pip install git+https://github.com/Janix-ai/mcp-validator@d766d3ee94076b13d0b73253e5221bbc76b9edb2` — document in the `ci.yml` pin comment.
- [ ] Local bring-up: `pip install 'mcp-validator==0.3.1'` in a venv; run the validator (via `mcp-validator` invocation) against `target/release/grex serve`; pass `--protocol-version 2025-06-18` to `mcp-validator`, NOT to `grex serve` (which does not accept that flag); assert exit 0.
- [ ] Document the pin (version + SHA + lookup date + source URL) either as a comment block at the top of the `mcp-conformance` job in `ci.yml` or in `docs/ci/mcp-conformance.md`.
- [ ] Bump policy: any bump re-runs `gh api repos/Janix-ai/mcp-validator/releases/latest` + `gh api repos/Janix-ai/mcp-validator/git/refs/tags/v<NEW_VERSION>` (substituting the new version number; current pinned example is `v0.3.1`). Update PyPI pin + SHA together in the same PR.

## Stage 2 — Release build + cache

- [ ] Confirm current `.github/workflows/ci.yml` `build` matrix runs debug only (no release artefact).
- [ ] Add a self-contained `cargo build --release -p grex` step inside the new `mcp-conformance` job (do NOT modify the existing `build` job).
- [ ] Configure `Swatinem/rust-cache@v2` with a distinct `key: release` so the release target doesn't thrash the debug cache.

## Stage 3 — Wire validator invocation

- [ ] Append `mcp-conformance` job to `.github/workflows/ci.yml` per spec §Design.
- [ ] Server command: `"$GITHUB_WORKSPACE/target/release/grex serve"`.
- [ ] Protocol version: `2025-06-18`.
- [ ] Do NOT add `needs: [build]` — the validator job is self-contained (owns its own release build) and runs in parallel with the `build` matrix per spec §Design + §Acceptance budget line.
- [ ] Upload validator log as artefact on failure (always).
- [ ] Push to a throwaway branch; verify job runs, is green on baseline, and fails loudly on a deliberate handshake regression → revert.

## Stage 4 — Branch protection + bypass doc

- [ ] Maintainer: mark `mcp-conformance` as a required status check on `main` via GitHub branch-protection settings.
- [ ] Document the bypass procedure in `docs/ci/mcp-conformance.md` §Bypass (how to temporarily remove the required check if the validator itself regresses).
- [ ] Update `progress.md` with commit SHA + CI green confirmation.
