# feat-m7-3 — MCP CI conformance (L6 only)

**Status**: draft
**Milestone**: M7 (see [`../../../milestone.md`](../../../milestone.md) §M7)
**Depends on**: feat-m7-1 (server), feat-m7-2 (tests). The job is self-contained (own `cargo build --release -p grex` step) and does NOT depend on the existing `build` matrix job.

## Motivation

feat-m7-2 covers internal correctness against our own rmcp-typed client. That leaves a gap: "we think we follow the spec" has never been checked by an **independent implementation**. An external protocol-conformance gate closes that gap and is a required check for merge.

Scope is deliberately narrow. Earlier drafts bundled L6 conformance + L7 Inspector smoke + L8 fuzz into one change; the trimmed scope below keeps only the conformance gate.

Dropped from prior draft (explicitly out of scope):
- ~~L7 Inspector CLI smoke~~ — scope creep; `mcp-validator` covers conformance already.
- ~~L8 `cargo-fuzz`~~ — deferred to M8 (milestone doesn't require fuzz).
- ~~Auto-issue filing on fuzz crash~~ — dropped with L8.
- ~~Node CI matrix addition~~ — dropped with L7 (no Inspector = no Node).

## Goal

One new CI job — `mcp-conformance` — on `ubuntu-latest`, running `mcp-validator` against a release build of `grex serve` at protocol `2025-06-18`. PR-blocking via branch-protection.

## Design

### Tool

**`mcp-validator`** (Janix-ai, repo `github.com/Janix-ai/mcp-validator`). Canonical PyPI name and GitHub repo slug both verified — use the actual published names everywhere; earlier drafts used a longer, unpublished name which has been removed.

Pin: PyPI version `0.3.1` (published 2025-07-08, supports protocol `2025-06-18`) verified against git commit SHA `d766d3ee94076b13d0b73253e5221bbc76b9edb2` (tag `v0.3.1`). See §Validator pin below.

### CI job

Append to `.github/workflows/ci.yml`:

```yaml
mcp-conformance:
  name: MCP protocol conformance (2025-06-18)
  runs-on: ubuntu-latest
  # needs: [build] would force a wait on the full 3-OS × 1-toolchain matrix
  # (ubuntu + macos + windows) ~= 5 min p95. The validator job is
  # self-contained — it does its own `cargo build --release -p grex` step
  # below and does not consume any artefact from the `build` matrix — so
  # we OMIT `needs:` and let it run in parallel. Rationale documented in
  # §Acceptance budget line.
  steps:
    - uses: actions/checkout@v6
    - uses: dtolnay/rust-toolchain@stable
    - uses: Swatinem/rust-cache@v2
      with:
        key: release                        # separate cache key from the debug `build` job
    - uses: actions/setup-python@v5
      with: { python-version: "3.12" }
    - name: Build release grex (self-contained; ~3 min cold, < 1 min warm)
      run: cargo build --release -p grex
    - name: Install validator (pinned)
      # Pin: `mcp-validator==0.3.1` from PyPI, verified against
      # `github.com/Janix-ai/mcp-validator@d766d3ee94076b13d0b73253e5221bbc76b9edb2`
      # (tag v0.3.1, published 2025-07-08). Bump both values together.
      run: pip install 'mcp-validator==0.3.1'
    - name: Run conformance suite
      run: |
        mcp-validator \
          --server-command "$GITHUB_WORKSPACE/target/release/grex serve" \
          --protocol-version 2025-06-18
    - if: always()
      uses: actions/upload-artifact@v7
      with:
        name: mcp-conformance-log
        path: mcp-validator.log
```

Notes:
- **`needs:` is deliberately OMITTED.** The existing `build` matrix is a fan-out (`ubuntu-latest` + `macos-latest` + `windows-latest` × `stable`) that produces only debug artefacts; a `needs: [build]` dependency would stall this job until the slowest matrix leg completes (~5 min p95) with no artefact payoff. The validator job owns its own release build step (see `Build release grex`), so it is genuinely independent and can fan out in parallel with the matrix. If a future revision extracts release-build caching out of this job, revisit.
- Validator runs against the **compiled binary**, not `cargo run` — release build is mandatory so test duration reflects shipped artefact.
- Release target cached separately from the debug `build` matrix via distinct `key:`; avoids thrashing the existing cache.
- Job fails on non-zero validator exit; the job failure **blocks PR merge** via branch protection (configured out-of-band by a maintainer — see Acceptance #3).

### Validator pin

Both forms of pin recorded, in `docs/ci/mcp-conformance.md` (optional file — may be inlined as a comment in `ci.yml` instead):

- **PyPI**: `mcp-validator==0.3.1` — current stable at draft time, resolved 2026-04-21 via `gh api repos/Janix-ai/mcp-validator/releases/latest` (tag `v0.3.1`, published 2025-07-08). Explicitly supports protocol `2025-06-18`.
- **Upstream**: `github.com/Janix-ai/mcp-validator@d766d3ee94076b13d0b73253e5221bbc76b9edb2` — commit SHA corresponding to tag `v0.3.1`, resolved via `gh api repos/Janix-ai/mcp-validator/git/refs/tags/v0.3.1`. Fallback install path: `pip install git+https://github.com/Janix-ai/mcp-validator@d766d3ee94076b13d0b73253e5221bbc76b9edb2`.

**Any bump MUST update both PyPI version and SHA together.** A version drift between the two is a merge blocker — Stage 1 of `tasks.md` re-runs the lookup when bumping.

### Bypass path

Adversarial review flagged the case "validator itself breaks; all PRs blocked". Mitigations:

1. Branch protection is configurable by maintainers — the required-check can be removed temporarily.
2. Job runs with `continue-on-error: false` but the pin is explicit, so a validator regression is reproducible locally (`pip install mcp-validator==0.3.1`) and a dated fix is a one-line PR.
3. Document the bypass procedure in `docs/ci/mcp-conformance.md` §Bypass.

## File / module targets

| Concrete path | Change |
|---|---|
| `.github/workflows/ci.yml` | Append `mcp-conformance` job. |
| `docs/ci/mcp-conformance.md` | New (optional) — pin rationale + bypass procedure. |
| `branch-protection` (GitHub settings, doc'd not in-repo) | Add `mcp-conformance` as required check on `main`. |

## Test plan

The CI job **is** the test. Meta-validation:

- Local dry-run on a dev machine: `pip install mcp-validator==0.3.1 && mcp-validator --server-command "$(pwd)/target/release/grex serve" --protocol-version 2025-06-18` → exit 0.
- Deliberate regression: comment out the `initialized` notification handler on a throwaway branch → assert validator exit ≠ 0 → revert.
- Verify the job shows up as a required check in GitHub's branch-protection UI after maintainer configures it.

## Non-goals

- **No Inspector CLI smoke.** Dropped from this change.
- **No fuzz.** Deferred to M8.
- **No Node in CI matrix.** Dropped with Inspector.
- **No auto-issue filing.** Dropped with fuzz.
- **No HTTP/SSE transport conformance.** stdio only per M7.
- **No OAuth / auth conformance.** No auth in v1.

## Dependencies

- **Prior**: feat-m7-1 (server binary exists; `grex serve` launches it) + feat-m7-2 (in-process harness already exercises the server, giving confidence the validator will have a working peer).
- **External runtimes in CI**: Python 3.12.
- **No new Rust deps.**
- **CI**: existing `build` matrix job remains authoritative for debug builds; this change only adds a release build inside its own job.

## Acceptance

1. `mcp-conformance` job runs on every PR and is green on the baseline.
2. Validator version and commit SHA pinned and documented (either in `ci.yml` comment or `docs/ci/mcp-conformance.md`).
3. Branch-protection rule on `main` lists `mcp-conformance` as a required status check (maintainer action; documented).
4. Deliberate-regression smoke: a known-bad handshake on a throwaway branch makes the job fail ≠ 0.
5. No regression on existing CI wall-clock — new job runs in parallel with `build`; added total wall-clock ≤ release build time (~3 min cold, < 1 min warm cache).
   **Budget line**: with `needs:` omitted, the validator job runs in parallel and costs only its own steps: `cargo build --release -p grex` (~3 min cold / < 1 min warm) + validator install (~15 s) + validator run (~30 s) ≈ **3.5 min cold / 1.5 min warm p95**. If a maintainer re-adds `needs: [build]` for artefact reuse in a later revision, the budget rises to `build matrix (~5 min) + release build (~3 min) + validator (~30 s) ≈ 9 min p95` — document + accept before merging such a revision.
6. Local repro of any CI failure works via the documented pin.

## Source-of-truth links

- [`.omne/cfg/mcp.md`](../../../.omne/cfg/mcp.md) — wire spec under test (Path B rewrite).
- [`.github/workflows/ci.yml`](../../../.github/workflows/ci.yml) — target file.
- [`milestone.md`](../../../milestone.md) §M7.
- [`openspec/changes/feat-m7-1-mcp-server/spec.md`](../feat-m7-1-mcp-server/spec.md) — server under test.
- [`openspec/changes/feat-m7-2-mcp-test-harness/spec.md`](../feat-m7-2-mcp-test-harness/spec.md) — internal harness this layer complements.
- [`openspec/changes/feat-m7-mcp-research/`](../feat-m7-mcp-research/) — validator stack decision record.
