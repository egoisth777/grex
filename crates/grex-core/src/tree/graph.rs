//! Read-only pack graph produced by the [`crate::tree::Walker`].
//!
//! The graph is a value type: once the walker returns a [`PackGraph`] the
//! structure is immutable. Callers (validators, schedulers, renderers) only
//! ever see the already-assembled graph — they never participate in its
//! construction. This decoupling lets us swap the walker for, say, an IPC
//! driver or a snapshot-replay harness without touching any downstream
//! consumer.
//!
//! # Ownership model
//!
//! * Nodes live in a `Vec`; node id == vector index.
//! * Edges are a flat vector for cheap iteration; the few lookups we perform
//!   on a walked tree do not justify an adjacency-map yet.
//! * The root is always at index `0` by construction.
//!
//! # Non-goals
//!
//! * No mutation API. The graph cannot grow or shrink after walker exit.
//! * No topological sort here — that belongs to the scheduler slice.
//! * No serialisation — persistence is a later slice.

use std::path::PathBuf;

use crate::pack::PackManifest;

/// Node-edge relationship kind.
///
/// `Child` edges are *owned*: the walker cloned the target repo and recursed
/// into it. `DependsOn` edges are *referential*: the walker recorded that the
/// parent named this dep but did not hydrate it — resolution happens at
/// validate time via `DependsOnValidator`.
///
/// Marked `#[non_exhaustive]` so new edge relations (e.g. `Provides`,
/// `Conflicts`) can land without breaking external match sites.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// Parent owns/walks this child (cloned + recursed).
    Child,
    /// Parent merely references this pack by name or url.
    DependsOn,
}

/// A pack in the walked graph.
///
/// Every field is `pub` by design: the graph is a read-only value type, and
/// exposing the full record here is simpler than hand-curating accessors for
/// each field.
///
/// Marked `#[non_exhaustive]` so audit fields (resolved ref SHA, hydration
/// timestamps) can land without breaking library consumers who destructure
/// the struct.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct PackNode {
    /// Stable index inside the graph (equal to the `Vec` position).
    pub id: usize,
    /// `name` copied from the pack manifest for O(1) lookup.
    pub name: String,
    /// On-disk location of the pack's working tree.
    pub path: PathBuf,
    /// Source URL the walker used to hydrate this node, or `None` for the
    /// root / nodes that were loaded directly from an on-disk path.
    pub source_url: Option<String>,
    /// Full parsed manifest.
    pub manifest: PackManifest,
    /// Parent id; `None` for the root.
    pub parent: Option<usize>,
}

/// An edge in the walked graph.
///
/// Marked `#[non_exhaustive]` so future edge-level metadata (priority,
/// guard expression) is non-breaking for library consumers.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct PackEdge {
    /// Origin node id.
    pub from: usize,
    /// Target node id.
    pub to: usize,
    /// Relationship kind.
    pub kind: EdgeKind,
}

/// Fully-walked pack graph. Immutable post-construction.
///
/// Nodes are owned; callers borrow via [`PackGraph::nodes`] or the dedicated
/// lookup helpers.
#[derive(Debug)]
pub struct PackGraph {
    nodes: Vec<PackNode>,
    edges: Vec<PackEdge>,
}

impl PackGraph {
    /// Construct a graph from raw node and edge vectors.
    ///
    /// Pub-crate visibility keeps mutation ownership with the walker while
    /// allowing an empty graph or a manually-assembled fixture inside tests
    /// (via `pub(crate)` helpers invoked from the integration test harness).
    ///
    /// # Panics
    ///
    /// Panics if `nodes` is empty — a walk always produces at least the
    /// root. This is a programming-error guard rather than a user-facing
    /// failure mode.
    #[must_use]
    pub(crate) fn new(nodes: Vec<PackNode>, edges: Vec<PackEdge>) -> Self {
        assert!(!nodes.is_empty(), "PackGraph must contain at least the root node");
        Self { nodes, edges }
    }

    /// The root node (id == 0).
    #[must_use]
    pub fn root(&self) -> &PackNode {
        &self.nodes[0]
    }

    /// All nodes in insertion order.
    #[must_use]
    pub fn nodes(&self) -> &[PackNode] {
        &self.nodes
    }

    /// All edges in insertion order.
    #[must_use]
    pub fn edges(&self) -> &[PackEdge] {
        &self.edges
    }

    /// Iterate the `Child`-kind neighbours of `id` (in insertion order).
    pub fn children_of(&self, id: usize) -> impl Iterator<Item = &PackNode> {
        self.neighbours(id, EdgeKind::Child)
    }

    /// Iterate the `DependsOn`-kind neighbours of `id`.
    pub fn depends_on_of(&self, id: usize) -> impl Iterator<Item = &PackNode> {
        self.neighbours(id, EdgeKind::DependsOn)
    }

    /// Find a node by its manifest name. Returns the first match in
    /// insertion order; names are not guaranteed unique across a graph,
    /// though per-pack validators may reject duplicates in future slices.
    #[must_use]
    pub fn find_by_name(&self, name: &str) -> Option<&PackNode> {
        self.nodes.iter().find(|n| n.name == name)
    }

    /// Borrow a node by id.
    #[must_use]
    pub fn node(&self, id: usize) -> Option<&PackNode> {
        self.nodes.get(id)
    }

    fn neighbours(&self, id: usize, kind: EdgeKind) -> impl Iterator<Item = &PackNode> {
        self.edges
            .iter()
            .filter(move |e| e.from == id && e.kind == kind)
            .filter_map(|e| self.nodes.get(e.to))
    }
}
