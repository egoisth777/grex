//! Tool registry surface for the grex MCP server.
//!
//! Stage 5 ships the **name table** only — the per-tool handler bodies and
//! their `#[tool]` annotations land in Stage 6. This file is the single
//! source of truth for which CLI verbs are reachable as MCP tools.
//!
//! Per `openspec/changes/feat-m7-1-mcp-server/spec.md` §"Tool surface":
//! the 11 verbs below are exposed; `serve` (the transport itself) and
//! `teardown` (a plugin lifecycle hook invoked from `rm`) are NOT.

/// The 11 CLI verbs surfaced as MCP tools. Order is the spec's documentation
/// order — keep stable so doc-string snapshots remain meaningful.
pub const VERBS_11_EXPOSED_AS_TOOLS: &[&str] = &[
    "init", "add", "rm", "ls", "status", "sync", "update", "doctor", "import", "run", "exec",
];

// Compile-time invariant: the spec mandates exactly 11 tools. Catch any
// drift the moment someone edits the array.
const _: () = assert!(VERBS_11_EXPOSED_AS_TOOLS.len() == 11);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_exactly_eleven_verbs() {
        assert_eq!(VERBS_11_EXPOSED_AS_TOOLS.len(), 11);
    }

    #[test]
    fn excludes_serve_and_teardown() {
        assert!(!VERBS_11_EXPOSED_AS_TOOLS.contains(&"serve"));
        assert!(!VERBS_11_EXPOSED_AS_TOOLS.contains(&"teardown"));
    }

    #[test]
    fn names_are_unique() {
        let mut sorted: Vec<&str> = VERBS_11_EXPOSED_AS_TOOLS.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), VERBS_11_EXPOSED_AS_TOOLS.len());
    }
}
