## feat-m7-3-mcp-ci-conformance

**Status**: draft

External MCP protocol conformance gate in CI. Adds a single `mcp-conformance` job that runs `mcp-validator` (Janix-ai, pinned) against `grex serve` on `ubuntu-latest`, asserting protocol `2025-06-18`. PR-blocking via branch protection. Nothing else — Inspector smoke dropped (scope creep), fuzz deferred to M8.
