//! Post-hoc cycle detector over an assembled [`PackGraph`].
//!
//! The walker detects cycles during recursion, so in normal use this
//! validator is redundant. It exists as a **defensive** check so:
//!
//! * Manually-assembled test fixtures (or future graph mutation APIs)
//!   cannot smuggle a cycle past validation.
//! * A future IPC-backed walker that fails to enforce cycle detection
//!   cannot silently produce a bad graph.
//!
//! Implementation: depth-first traversal of `Child` edges (cycles in the
//! owned-subtree space are what the walker-specified invariant forbids).
//! `DependsOn` edges are non-owning references and are excluded from the
//! check.

use std::collections::HashSet;

use super::{GraphValidator, PackValidationError};
use crate::tree::{EdgeKind, PackGraph};

/// Detect cycles among `Child` edges of a pack graph.
pub struct CycleValidator;

impl GraphValidator for CycleValidator {
    fn name(&self) -> &'static str {
        "graph_cycle"
    }

    fn check(&self, graph: &PackGraph) -> Vec<PackValidationError> {
        let mut errs = Vec::new();
        let mut visited: HashSet<usize> = HashSet::new();
        for start in 0..graph.nodes().len() {
            if visited.contains(&start) {
                continue;
            }
            let mut on_stack = Vec::new();
            detect_from(graph, start, &mut visited, &mut on_stack, &mut errs);
        }
        errs
    }
}

/// DFS helper. Pushes node id onto `on_stack` on entry, pops on exit;
/// a back-edge into `on_stack` is a cycle.
fn detect_from(
    graph: &PackGraph,
    id: usize,
    visited: &mut HashSet<usize>,
    on_stack: &mut Vec<usize>,
    errs: &mut Vec<PackValidationError>,
) {
    if on_stack.contains(&id) {
        errs.push(build_cycle_error(graph, on_stack, id));
        return;
    }
    if !visited.insert(id) {
        return;
    }
    on_stack.push(id);
    for edge in graph.edges() {
        if edge.from == id && edge.kind == EdgeKind::Child {
            detect_from(graph, edge.to, visited, on_stack, errs);
        }
    }
    on_stack.pop();
}

/// Build a [`PackValidationError::GraphCycle`] whose chain lists the
/// back-edge path.
fn build_cycle_error(
    graph: &PackGraph,
    on_stack: &[usize],
    back_edge_target: usize,
) -> PackValidationError {
    let mut chain: Vec<String> =
        on_stack.iter().filter_map(|id| graph.node(*id).map(|n| n.name.clone())).collect();
    if let Some(n) = graph.node(back_edge_target) {
        chain.push(n.name.clone());
    }
    PackValidationError::GraphCycle { chain }
}
