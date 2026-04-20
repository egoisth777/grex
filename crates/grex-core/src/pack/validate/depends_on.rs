//! `depends_on` resolver over an assembled [`PackGraph`].
//!
//! The walker records `depends_on` as edges only when the target already
//! exists in the graph; unresolved entries are surfaced here as
//! [`PackValidationError::DependsOnUnsatisfied`].
//!
//! Resolution rule (mirrors the walker's `looks_like_url`):
//!
//! * URL-shaped entry (scheme prefix or `.git` suffix) → match against
//!   each node's `source_url`.
//! * Otherwise → match against each node's `name`.

use super::{GraphValidator, PackValidationError};
use crate::tree::PackGraph;

/// Verify every `depends_on` entry resolves to some node in the graph.
pub struct DependsOnValidator;

impl GraphValidator for DependsOnValidator {
    fn name(&self) -> &'static str {
        "depends_on_unsatisfied"
    }

    fn check(&self, graph: &PackGraph) -> Vec<PackValidationError> {
        let mut errs = Vec::new();
        for node in graph.nodes() {
            for dep in &node.manifest.depends_on {
                if !is_resolvable(graph, dep) {
                    errs.push(PackValidationError::DependsOnUnsatisfied {
                        pack: node.name.clone(),
                        required: dep.clone(),
                    });
                }
            }
        }
        errs
    }
}

fn is_resolvable(graph: &PackGraph, dep: &str) -> bool {
    if looks_like_url(dep) {
        graph.nodes().iter().any(|n| n.source_url.as_deref() == Some(dep))
    } else {
        graph.find_by_name(dep).is_some()
    }
}

/// Literal URL/path heuristic. Duplicates the walker's rule to keep both
/// modules decoupled — neither imports from the other's internals.
fn looks_like_url(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("ssh://")
        || s.starts_with("git@")
        || s.ends_with(".git")
}
