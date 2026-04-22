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
| Upstream | [`github.com/Janix-ai/mcp-validator`](https://github.com/Janix-ai/mcp-validator) |
| CI source | [`github.com/egoisth777/mcp-validator`](https://github.com/egoisth777/mcp-validator) (org-controlled mirror, same SHA) |
| Tag | `v0.3.1` |
| Commit SHA | `d766d3ee94076b13d0b73253e5221bbc76b9edb2` |
| Released | 2025-07-08T13:55:45Z |
| Install path | `actions/checkout` the pinned SHA from the **mirror** into `.mcp-validator/`, then `pip install -r .mcp-validator/requirements.txt`, then run `python -m mcp_testing.stdio.cli` with `PYTHONPATH=.mcp-validator`. |
| PyPI status | `mcp-validator==0.3.1` is **NOT** published on PyPI (only `0.1.1` is). |
| `pip install git+URL` | **NOT** supported at this SHA. The upstream repo at tag `v0.3.1` ships neither `setup.py` nor `pyproject.toml`, so pip refuses with `does not appear to be a Python project`. Clone-and-run is the only supported path until upstream adds a packaging file. |
| Protocol | `2025-06-18` (matches `.omne/cfg/mcp.md` SSOT) |
| Pin verified | 2026-04-22 via `gh api repos/Janix-ai/mcp-validator/git/refs/tags/v0.3.1` |

### Bump policy

Any bump MUST update tag AND SHA together. Re-run:

```bash
gh api repos/Janix-ai/mcp-validator/releases/latest --jq '.tag_name,.published_at'
gh api repos/Janix-ai/mcp-validator/git/refs/tags/v<NEW> --jq '.object.sha'
```

Drift between the two is a merge blocker.

### Supply-chain hardening

The CI job checks out from `egoisth777/mcp-validator` (an **org-controlled
mirror / fork** of `Janix-ai/mcp-validator`) rather than upstream directly.
Rationale: `actions/checkout` of an external repo that we then `pip install`
hands that external maintainer arbitrary code execution under the CI token
if upstream is compromised, rewritten, or replaced. Mirroring the pin into
a repo we control closes that window — the SHA is byte-identical to
upstream, but the host cannot be tampered with by third parties.

**Mirror refresh procedure** (run once per validator bump):

```bash
# 1. Confirm new upstream tag + SHA.
gh api repos/Janix-ai/mcp-validator/releases/latest --jq '.tag_name,.published_at'
NEW_SHA=$(gh api repos/Janix-ai/mcp-validator/git/refs/tags/v<NEW> --jq '.object.sha')

# 2. Sync mirror's default branch with upstream (one-time, if not already
#    tracking). The fork created via `gh repo fork Janix-ai/mcp-validator`
#    already has all history; subsequent refreshes via:
gh api -X POST repos/egoisth777/mcp-validator/merge-upstream \
  -f branch=main

# 3. Verify the new SHA is reachable from the mirror.
gh api repos/egoisth777/mcp-validator/commits/$NEW_SHA --jq '.sha'

# 4. Update `ref:` in `.github/workflows/ci.yml` mcp-conformance job.
```

## CLI invocation

There is no `mcp-validator` console entry point at this SHA. The canonical
invocation (matches upstream `ref_gh_actions/stdio-validation.yml` at tag
`v0.3.1`) is a `python -m` call against the checked-out module:

```bash
PYTHONPATH=/abs/path/to/mcp-validator-checkout \
python -m mcp_testing.stdio.cli \
  "$GITHUB_WORKSPACE/target/release/grex serve" \
  --protocol-version 2025-06-18 \
  --output-dir reports \
  --report-format json
```

Verified `--help` output at tag `v0.3.1`:

```
usage: cli.py [-h] [--args ARGS [ARGS ...]] [--debug]
              [--protocol-version {2024-11-05,2025-03-26,2025-06-18}]
              [--output-dir OUTPUT_DIR] [--report-format {text,json,html}]
              server_command
```

Notes:

- Server command is **positional**, NOT `--server-command` (the earlier
  spec draft had this wrong; corrected here and in `ci.yml`).
- **No `--timeout` flag exists at this SHA.** Upstream's own
  `ref_gh_actions/stdio-validation.yml` template lists `--timeout 30` but
  the template drifted from the code; the CLI rejects it with
  `unrecognized arguments: --timeout 30`. Omitted.
- `--protocol-version` is passed to the validator, NOT to `grex serve`
  (which does not accept that flag).
- `PYTHONPATH` is required because the upstream repo at this SHA is not
  pip-installable (no `setup.py` / `pyproject.toml`).
- The job uploads `reports/` as a workflow artefact (`mcp-conformance-reports`,
  14-day retention) regardless of pass/fail so failed runs are debuggable.

## Local repro

From repo root on any supported OS (Python 3.12):

```bash
cargo build --release -p grex
# Use the org mirror (same SHA) so local repro matches CI's supply chain.
git clone https://github.com/egoisth777/mcp-validator .mcp-validator
git -C .mcp-validator checkout d766d3ee94076b13d0b73253e5221bbc76b9edb2
python -m pip install --upgrade pip
pip install -r .mcp-validator/requirements.txt
mkdir -p reports
PYTHONPATH="$(pwd)/.mcp-validator" \
python -m mcp_testing.stdio.cli \
  "$(pwd)/target/release/grex serve" \
  --protocol-version 2025-06-18 \
  --output-dir reports \
  --report-format json
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

### Upstream disappearance

If `Janix-ai/mcp-validator` is deleted, renamed, or has its history
rewritten, CI continues to work unchanged because the job reads from the
**org mirror** `egoisth777/mcp-validator`, which retains the pinned SHA
independently. Remediation in that scenario:

1. Confirm the mirror still holds the pinned SHA:
   ```bash
   gh api repos/egoisth777/mcp-validator/commits/d766d3ee94076b13d0b73253e5221bbc76b9edb2 --jq '.sha'
   ```
2. File a tracking issue noting upstream loss so future bumps either (a)
   find a replacement validator or (b) vendor the validator source under
   `.mcp-validator-vendored/` in-repo and drop the external checkout step.
3. No CI changes required in the meantime — the mirror IS the durable
   source of truth for the pinned build.

Pointing `ref:` back at upstream is only appropriate if upstream is
restored AND has been re-audited.

## CI job layout

See `.github/workflows/ci.yml` job `mcp-conformance`. Notes:

- **No `needs:` dependency.** The job owns its own `cargo build --release -p grex`
  step and runs in parallel with the debug `build` matrix. Adding
  `needs: [build]` would stall on the 3-OS matrix (~5 min p95) with no
  artefact payoff. Budget: ~3.5 min cold / ~1.5 min warm.
- **Release cache key is distinct** (`key: release`) so the release target
  profile does not thrash the debug cache used by `build`.
- **Python 3.12** matches upstream's template.
