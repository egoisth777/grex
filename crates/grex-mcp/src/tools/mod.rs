//! Tool registry surface for the grex MCP server.
//!
//! Stage 6 lights up the full registry. Each verb lives in its own
//! module under this directory, exporting a `Params` type and a free
//! `handle(&ServerState, Parameters<P>) -> Result<CallToolResult, _>`
//! function with the actual logic. The 11 [`#[rmcp::tool]`](rmcp::tool)
//! handler methods on [`crate::GrexMcpServer`] (in [`handlers`]) are
//! paper-thin shims that forward into those free functions.
//!
//! Per `openspec/changes/feat-m7-1-mcp-server/spec.md` §"Tool surface":
//! the 11 verbs below are exposed; `serve` (the transport itself) and
//! `teardown` (a plugin lifecycle hook invoked from `rm`) are NOT.
//!
//! # Annotation matrix (frozen by `.omne/cfg/mcp.md`)
//!
//! | tool   | read_only_hint | destructive_hint |
//! |--------|----------------|------------------|
//! | init   | false          | false            |
//! | add    | false          | false            |
//! | rm     | false          | **true**         |
//! | ls     | true           | false            |
//! | status | true           | false            |
//! | sync   | false          | false            |
//! | update | false          | false            |
//! | doctor | true           | false            |
//! | import | false          | false            |
//! | run    | false          | **true**         |
//! | exec   | false          | **true**         |

pub mod add;
pub mod doctor;
pub mod exec;
pub mod handlers;
pub mod import;
pub mod init;
pub mod ls;
pub mod rm;
pub mod run;
pub mod status;
pub mod sync;
pub mod update;

/// The CLI verbs surfaced as MCP tools. Order is the spec's
/// documentation order — keep stable so doc-string snapshots remain
/// meaningful.
///
/// Renamed from `VERBS_EXPOSED` in feat-m7-2 Stage 5 (Flag 3
/// from m7-2 discovery). The arity (11) is enforced at compile-time
/// below; the const name no longer carries the count so future MCP-only
/// tools (e.g. `workspace/subscribe`) can join without a churny rename.
pub const VERBS_EXPOSED: &[&str] =
    &["init", "add", "rm", "ls", "status", "sync", "update", "doctor", "import", "run", "exec"];

// Compile-time invariant: spec mandates exactly 11 tools.
const _: () = assert!(VERBS_EXPOSED.len() == 11);

/// Per-verb annotation matrix. Source of truth for tests 6.T2 / 6.T3.
/// Kept as a `const &[…]` (not a `HashMap`) so the table is greppable
/// and reorderings show up in code-review diffs verbatim.
pub const ANNOTATIONS: &[(&str, bool, bool)] = &[
    // (verb,    read_only_hint, destructive_hint)
    ("init", false, false),
    ("add", false, false),
    ("rm", false, true),
    ("ls", true, false),
    ("status", true, false),
    ("sync", false, false),
    ("update", false, false),
    ("doctor", true, false),
    ("import", false, false),
    ("run", false, true),
    ("exec", false, true),
];

const _: () = assert!(ANNOTATIONS.len() == 11);

/// List all advertised tools by reaching into the `#[tool_router]`-
/// generated `tool_router()` on [`crate::GrexMcpServer`]. Test-only
/// helper; runtime callers should reach for the router directly via
/// `GrexMcpServer::tool_router()` (see `lib.rs::list_tools`).
#[cfg(test)]
pub(crate) fn list_all() -> Vec<rmcp::model::Tool> {
    crate::GrexMcpServer::tool_router().list_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 6.T1 — `tools/list` advertises exactly 11 tools, matching
    /// `VERBS_EXPOSED`.
    #[test]
    fn tools_list_advertises_exactly_11() {
        let tools = list_all();
        assert_eq!(
            tools.len(),
            11,
            "expected 11 tools, got {}: {:?}",
            tools.len(),
            tools.iter().map(|t| t.name.as_ref()).collect::<Vec<_>>()
        );
        let names: std::collections::BTreeSet<&str> =
            tools.iter().map(|t| t.name.as_ref()).collect();
        let expected: std::collections::BTreeSet<&str> = VERBS_EXPOSED.iter().copied().collect();
        assert_eq!(names, expected);
    }

    /// 6.T2 — every advertised tool sets BOTH `read_only_hint` and
    /// `destructive_hint`. Catches a future contributor who forgets one.
    #[test]
    fn every_tool_has_both_annotation_hints() {
        for t in list_all() {
            let a = t
                .annotations
                .as_ref()
                .unwrap_or_else(|| panic!("tool `{}` missing annotations entirely", t.name));
            assert!(a.read_only_hint.is_some(), "tool `{}` missing read_only_hint", t.name);
            assert!(a.destructive_hint.is_some(), "tool `{}` missing destructive_hint", t.name);
        }
    }

    /// 6.T3 — the destructive set is exactly `{rm, run, exec}`.
    #[test]
    fn destructive_tools_are_rm_run_exec_only() {
        let mut destructive: Vec<String> = list_all()
            .iter()
            .filter(|t| t.annotations.as_ref().and_then(|a| a.destructive_hint).unwrap_or(false))
            .map(|t| t.name.to_string())
            .collect();
        destructive.sort();
        assert_eq!(destructive, vec!["exec", "rm", "run"]);
    }

    /// Annotation matrix table consistency: every (verb, ro, de) row in
    /// `ANNOTATIONS` matches what the live router actually advertises.
    #[test]
    fn annotation_matrix_matches_live_router() {
        use std::collections::HashMap;
        let live: HashMap<String, (bool, bool)> = list_all()
            .into_iter()
            .map(|t| {
                let a = t.annotations.expect("annotations present");
                (t.name.to_string(), (a.read_only_hint.unwrap(), a.destructive_hint.unwrap()))
            })
            .collect();
        for (verb, ro, de) in ANNOTATIONS {
            let got = live
                .get(*verb)
                .copied()
                .unwrap_or_else(|| panic!("verb `{verb}` missing from live router"));
            assert_eq!(got, (*ro, *de), "annotation mismatch for `{verb}`");
        }
    }

    #[test]
    fn lists_exactly_eleven_verbs() {
        assert_eq!(VERBS_EXPOSED.len(), 11);
    }

    #[test]
    fn excludes_serve_and_teardown() {
        assert!(!VERBS_EXPOSED.contains(&"serve"));
        assert!(!VERBS_EXPOSED.contains(&"teardown"));
    }

    #[test]
    fn names_are_unique() {
        let mut sorted: Vec<&str> = VERBS_EXPOSED.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), VERBS_EXPOSED.len());
    }
}
