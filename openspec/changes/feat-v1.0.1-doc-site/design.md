# feat-v1.0.1-doc-site — design

**Status**: draft
**Spec**: [`proposal.md`](./proposal.md) · [`tasks.md`](./tasks.md)

## Decision: where should authored human docs live?

Two competing reservations:

- `docs/` — by org convention reserved for *agent-readable* project descriptions (the spec-style `.omne/cfg/`-adjacent material).
- `man/` — already the home of generated `*.1` man pages (one per CLI verb).

Both are "documentation"; only the audience differs. The chosen split:

| Directory | Audience | Content |
|---|---|---|
| `.omne/cfg/` | LLM agents | Source-of-truth design docs (manifest, MCP, CLI, etc.) |
| `man/` | Humans (CLI users + reading the doc site) | Auto-generated `*.1` + authored markdown reference |
| `grex-doc/` | Humans (web readers) | Built artefact: mdBook site sourced from `man/` |
| `docs/` | — (deleted) | n/a |

`docs/` overloaded both roles before this PR. Deleting it removes the conflation; `.omne/cfg/` remains the agent-readable source of truth, `man/` becomes the single human-readable home, and `grex-doc/` is purely a build artefact directory pointing at `man/`.

## Decision: man/ subfolder layout

The 17 authored files in `docs/src-authored/` plus `release.md`, `semver.md`, and `ci/mcp-conformance.md` need a logical bucketing in `man/`. Choice: 5 subdirs by audience-intent.

```
man/
├── *.1                            (existing, untouched)
├── concepts/                      "what is grex / how does it work"
│   ├── pack-spec.md
│   ├── manifest.md
│   ├── architecture.md
│   ├── goals.md
│   └── concurrency.md
├── reference/                     "exact contracts / schemas"
│   ├── cli.md
│   ├── cli-json.md
│   ├── actions.md
│   ├── plugin-api.md
│   └── mcp.md
├── guides/                        "task-oriented walkthroughs"
│   ├── migration.md
│   ├── engineering.md
│   └── test-plan.md
├── internals/                     "for grex contributors / advanced users"
│   ├── linter.md
│   ├── m3-review-findings.md
│   ├── roadmap.md
│   └── man-pages.md
├── ci/
│   └── mcp-conformance.md         (kept — referenced from ci.yml)
├── release.md                     (top-level — release procedure)
└── semver.md                      (top-level — versioning policy)
```

Rationale:
- **concepts** vs **reference** mirrors the standard tech-doc dichotomy (Diátaxis "explanation" vs "reference").
- **guides** is "how-to" material that's neither pure concept nor pure spec.
- **internals** isolates the noise (`m3-review-findings.md`, etc.) from things a v1 user would skim.
- `release.md` and `semver.md` stay at `man/` root because they're the most user-facing of the lot — they need to be findable from the README without spelunking subdirs.
- `ci/` keeps its own bucket because `mcp-conformance.md` is referenced by `ci.yml` comments verbatim and moving it under another bucket would require more workflow edits.

## Decision: SUMMARY.md generation

mdBook requires `src/SUMMARY.md` to exist before `mdbook build` will run. Two choices for how to keep it in sync with `man/`:

1. **Auto-generate via xtask**: `doc-site-prep` walks `man/**/*.md` and emits SUMMARY.md derived from filesystem layout.
2. **Hand-author once**: SUMMARY.md ships in `grex-doc/src/SUMMARY.md` and is updated manually when new chapters are added.

Choice: **(2) hand-author for v1.** Reasons:
- Auto-gen requires deciding chapter title (vs filename), nesting depth, and ordering — none of which is encoded in the filesystem. Either we add front-matter to every chapter or we accept lossy output.
- Adding new chapters is rare (this PR moves 17 files; adding one more is a 1-line SUMMARY edit).
- Linkcheck wants stable anchors; auto-regenerated SUMMARY would churn anchor IDs every time chapter ordering changed.
- Hand-author avoids hiding mdBook structure inside Rust code that contributors don't read.

`doc-site-prep` therefore handles only the *content* copy from `man/` → `grex-doc/src/`; SUMMARY.md is version-controlled and edited by hand.

## Decision: copy vs symlink

mdBook can read `src/` from any path, so we could in theory point it at `../man/` directly. We don't, because:

- mdBook's `src` config is relative and resolution behaviour around `..` is implementation-defined across versions.
- Symlinks under git on Windows are a permission minefield (developer mode, admin rights, Git LFS interactions).
- Linkcheck operates on the resolved tree; an `src/` that mixes copies and symlinks confuses path-relative checks.

`doc-site-prep` does a flat copy. Cost: one extra second on doc builds. Benefit: deterministic, Windows-friendly, no symlink permissions surprises.

## Decision: linkcheck preprocessor scope

`mdbook-linkcheck` validates **internal** Markdown links by default. We enable it; we do NOT enable external HTTP checking (`follow-web-links = false`):

- External-URL liveness checking would make CI flaky on transient network issues (every PR would gamble against GitHub Pages, crates.io, etc.).
- External-link rot is better caught by a periodic out-of-band audit, not the per-PR doc gate.

Internal-only linkcheck still catches the most common doc-rot bug: a renamed chapter breaking inbound links from sibling chapters.

## Decision: workflow trigger surface

`.github/workflows/doc-site.yml` triggers on:

- `pull_request` paths-filter on `man/**`, `grex-doc/**`, `crates/xtask/**` — smoke build only, no deploy.
- `push: tags: ['v*.*.*']` — full build + deploy to GitHub Pages.

We deliberately do NOT deploy on `push: branches: [main]`. Reason: the doc site is *versioned* — it should reflect the latest *released* version, not whatever's on `main` mid-release-prep. Deploying on tag push pins the published site to actual SemVer cuts.

Trade-off: the live site lags `main` until the next tag. Acceptable for v1; if doc cadence outpaces release cadence later, we add a separate `docs-preview` deploy on `main` push to a different gh-pages subdir.

## Decision: scope of `doc-site-prep` xtask

Minimum viable: walk `man/**/*.md`, mirror to `grex-doc/src/`, skip `*.1` and `README.md`. No SUMMARY generation, no front-matter rewriting, no link rewriting.

Why keep it minimal:
- Every additional transformation is a new bug surface and an inconsistency between "how the file reads in `man/`" vs "how it reads in the rendered site".
- Linkcheck catches relative-link breakages; we don't need link rewriting.
- Front-matter is mdBook-specific noise that would pollute the human-readable `man/` source.

## Out-of-scope for this PR

- **Multi-version doc selector** (e.g. v1.0.1 / v1.1.0 dropdown). Single-version site is fine for v1; revisit when we have a second minor.
- **Custom domain** (`docs.grex.dev`). Default `*.github.io` is acceptable per M8 spec.
- **Search index optimisation**. mdBook's default search is fine for ~30 chapters.
- **Auto-generation of CLI reference from clap**. The existing `cargo xtask gen-man` produces `*.1`; rendering those into the mdBook site as HTML is a v1.1 concern.
- **Adoption of mdbook-admonish / mdbook-katex / other preprocessors**. Linkcheck only for v1.0.1; richer rendering deferred.

## Risks

1. **mdBook + linkcheck not installable in CI** — `peaceiris/actions-mdbook@v2` handles mdBook but does NOT install preprocessors. Workaround: explicit `cargo install mdbook-linkcheck` step in the workflow.
2. **Path-rewrites missed** — a stray `docs/...` reference in a code comment slips past CI because nothing fails on a stale doc link in a Rust file. Mitigation: a deliberate `grep -rn 'docs/' --exclude-dir=target --exclude-dir=.git` audit step in the PR description.
3. **PR-A merge conflict on `man/README.md`** — PR-A creates `man/README.md`; if we land first, PR-A rebases. If they land first, we ignore (don't touch their file).
4. **Workspace version bump cascade** — easy to forget one of `[workspace.package]`, `[workspace.dependencies]`, `crates/xtask/Cargo.toml`. The new `version_test.rs` regression test catches the xtask piece; `cargo metadata` output verification covers the rest.
