# MCP protocol conformance (CI gate)

> Shipped in feat-m7-3. See `openspec/changes/feat-m7-3-mcp-ci-conformance/proposal.md`.

## Purpose

`mcp-conformance` is the **L6 external-oracle gate**: an independent
implementation (`mcp-validator`, Janix-ai) drives `grex serve` and asserts
wire-protocol conformance at MCP protocol version `2025-06-18`. Pairs with
the in-process L2-L5 harness from feat-m7-2 — the harness checks our own
rmcp-typed client, this job checks a third-party one.

## Pin

| Field | Value |
|---|---|
| Repository | [`github.com/Janix-ai/mcp-validator`](https://github.com/Janix-ai/mcp-validator) |
| Tag | `v0.3.1` |
| Commit SHA | `d766d3ee94076b13d0b73253e5221bbc76b9edb2` |
| Released | 2025-07-08T13:55:45Z |
| Install path | `pip install 'git+https://github.com/Janix-ai/mcp-validator@d766d3ee94076b13d0b73253e5221bbc76b9edb2'` |
| PyPI status | `mcp-validator==0.3.1` is **NOT** published on PyPI (only `0.1.1` is). The git+SHA install is the canonical and only reliable pin. |
| Protocol | `2025-06-18` (matches `.omne/cfg/mcp.md` SSOT) |
| Pin verified | 2026-04-22 via `gh api repos/Janix-ai/mcp-validator/git/refs/tags/v0.3.1` |

### Bump policy

Any bump MUST update tag AND SHA together. Re-run:

```bash
gh api repos/Janix-ai/mcp-validator/releases/latest --jq '.tag_name,.published_at'
gh api repos/Janix-ai/mcp-validator/git/refs/tags/v<NEW> --jq '.object.sha'
```

Drift between the two is a merge blocker.

## CLI invocation

The PyPI name `mcp-validator` does not ship a `mcp-validator` console entry
point at this version. The canonical invocation (matches upstream
`ref_gh_actions/stdio-validation.yml` at tag `v0.3.1`) is:

```bash
python -m mcp_testing.stdio.cli \
  "$GITHUB_WORKSPACE/target/release/grex serve" \
  --protocol-version 2025-06-18 \
  --output-dir reports \
  --timeout 30
```

Notes:

- Server command is **positional**, NOT `--server-command` (the earlier
  spec draft had this wrong; corrected here and in `ci.yml`).
- `--protocol-version` is passed to the validator, NOT to `grex serve`
  (which does not accept that flag).
- The job uploads `reports/` as a workflow artefact (`mcp-conformance-reports`,
  14-day retention) regardless of pass/fail so failed runs are debuggable.

## Local repro

From repo root on any supported OS (validator install requires Python 3.12):

```bash
cargo build --release -p grex
python -m pip install --upgrade pip
pip install 'git+https://github.com/Janix-ai/mcp-validator@d766d3ee94076b13d0b73253e5221bbc76b9edb2'
mkdir -p reports
python -m mcp_testing.stdio.cli \
  "$(pwd)/target/release/grex serve" \
  --protocol-version 2025-06-18 \
  --output-dir reports \
  --timeout 30
```

Exit code `0` = conformant. Non-zero = protocol drift (inspect
`reports/*.json` for the failing test cases).

### Deliberate-regression smoke

To confirm the gate actually gates (not just green-by-accident):

1. On a throwaway branch, break one MCP tool schema (e.g. rename a tool in
   `crates/grex-mcp/src/tools/` or drop a required field from a response).
2. Rerun the commands above — validator MUST exit non-zero.
3. Revert the change and confirm exit 0 again.

## Bypass procedure

Adversarial case: the validator itself regresses and blocks all PRs. To
remove the gate temporarily:

1. **Maintainer**: GitHub → Settings → Branches → `main` branch protection
   → "Required status checks" → untick `MCP protocol conformance (2025-06-18)`.
2. Save. The gate is now advisory.
3. File a tracking issue linking the validator upstream regression.
4. Once the upstream pin is fixed (or reverted), re-tick the required check
   and close the issue.

The pin is explicit, so a validator regression is always reproducible
locally via the install command above — fixes are single-line PRs that bump
the tag + SHA together.

## CI job layout

See `.github/workflows/ci.yml` job `mcp-conformance`. Notes:

- **No `needs:` dependency.** The job owns its own `cargo build --release -p grex`
  step and runs in parallel with the debug `build` matrix. Adding
  `needs: [build]` would stall on the 3-OS matrix (~5 min p95) with no
  artefact payoff. Budget: ~3.5 min cold / ~1.5 min warm.
- **Release cache key is distinct** (`key: release`) so the release target
  profile does not thrash the debug cache used by `build`.
- **Python 3.12** matches upstream's template.
