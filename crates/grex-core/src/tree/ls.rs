//! Read-only `ls` tree builder.
//!
//! Shared structured-tree backend for the `grex ls` CLI surface and the
//! MCP `ls` tool. Both surfaces walk the workspace identically — the
//! only divergence is the rendering layer: CLI prints box-drawing
//! characters in human mode; MCP returns the tree as JSON inside a
//! `CallToolResult` envelope. Keeping the walk + JSON shape in one place
//! prevents drift between the two surfaces (the v1.1.1 parity blocker
//! tracked in `tests/parity.rs::parity_ls`).
//!
//! The walk is strictly read-only: no clone, no fetch, no exec. Each
//! on-disk child resolves into one of four states:
//!
//! 1. *Loaded* — `.grex/pack.yaml` parsed cleanly. Real manifest values
//!    flow into the [`LsNode`].
//! 2. *Synthetic* — no `.grex/pack.yaml` but `.git/` is present (and the
//!    destination itself is not a symlink, per the FIX-1 hardening in
//!    [`super::walker::dest_has_git_repo`]). The walker synthesises a
//!    leaf scripted manifest via [`super::synthesize_plain_git_manifest`]
//!    so future shape changes flow here automatically.
//! 3. *Unsynced* — declared in the parent manifest but the destination
//!    directory is absent on disk. Surfaced as a placeholder node so a
//!    fresh meta-pack checkout (pre-`grex sync`) does not look like an
//!    empty tree.
//! 4. *Errored* — `.grex/pack.yaml` exists but failed to read or parse.
//!    The node carries an `error: {kind, message}` envelope so JSON
//!    consumers see the failure without the verb aborting.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::pack::{ChildRef, PackManifest, PackType};

use super::error::TreeError;
use super::loader::{FsPackLoader, PackLoader};
use super::walker::{dest_has_git_repo, synthesize_plain_git_manifest};

/// Top-level envelope shared by CLI `--json` and MCP `ls`.
///
/// Field shape pinned by `man/reference/cli-json.md` §"ls": `{workspace,
/// tree[]}`. The `tree` array always has length 1 for a successful
/// build (the root pack); kept as an array so future surfaces that walk
/// from a workspace dir with multiple sibling packs can extend without
/// a schema break.
#[derive(Debug, Clone, Serialize)]
pub struct LsTree {
    /// Absolute path to the resolved workspace (the dir containing the
    /// root pack's `.grex/`, or the pack root itself for the
    /// flat-sibling layout).
    pub workspace: String,
    /// Root nodes. Currently always a single entry — the root pack.
    pub tree: Vec<LsNode>,
}

/// One node in the structured `ls` tree.
///
/// `id` is a stable in-walk counter so JSON consumers can address a
/// specific node without path-string parsing. `synthetic = true` marks
/// plain-git children whose pack manifest was synthesised in-memory by
/// the walker (no `.grex/pack.yaml` on disk); `type` is always
/// `"scripted"` for synthetic nodes per the v1.1.1 design.
///
/// The flags `synthetic`, `unsynced`, and `error` are mutually
/// exclusive — at most one is set per node. A successfully loaded real
/// manifest leaves all three at their default (`false`/`None`).
#[derive(Debug, Clone, Serialize)]
pub struct LsNode {
    /// Stable in-walk identifier (depth-first, root = 0).
    pub id: usize,
    /// Pack name (manifest `name:` for real packs, child path for
    /// synthetic / unsynced / errored ones).
    pub name: String,
    /// Absolute on-disk path of the pack root.
    pub path: String,
    /// Pack flavour discriminator. Snake-case label from
    /// [`PackType::as_str`] — `"meta"`, `"declarative"`, or
    /// `"scripted"`.
    #[serde(rename = "type")]
    pub pack_type: String,
    /// True iff the manifest was synthesised in-memory (plain-git
    /// child). See `man/reference/pack-spec.md` §"Plain-git children".
    pub synthetic: bool,
    /// True iff the child is declared in its parent's manifest but its
    /// destination directory is absent on disk. Distinguishes a
    /// fresh-checkout state (which pre-FIX-4 was rendered as an empty
    /// tree) from a fully-synced workspace. Skipped from JSON output
    /// when `false` so the v1.1.1 baseline JSON shape is unchanged for
    /// the common case.
    #[serde(default, skip_serializing_if = "is_false")]
    pub unsynced: bool,
    /// `Some` when the child's manifest exists but failed to read or
    /// parse. Surfaces partial-corruption to JSON consumers without
    /// aborting the entire walk. Skipped from JSON output when `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<LsNodeError>,
    /// Recursively walked children. Empty for leaves (declarative and
    /// scripted packs and synthetic plain-git children).
    pub children: Vec<LsNode>,
}

/// Per-node error envelope. `kind` is one of `"parse"`, `"read"`, or
/// `"other"` — matching the JSON taxonomy documented in
/// `man/reference/cli-json.md` §"ls".
#[derive(Debug, Clone, Serialize)]
pub struct LsNodeError {
    /// Short discriminator: `"parse"` (YAML didn't deserialise),
    /// `"read"` (IO error reading the file), `"other"` (any other
    /// `TreeError` variant — e.g. a `PackNameMismatch` surfacing
    /// without the walker's git stages running).
    pub kind: String,
    /// Human-readable underlying error message (`Display` of the
    /// originating `TreeError`).
    pub message: String,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// Build the structured tree rooted at `pack_root`.
///
/// `pack_root` is either a directory holding `.grex/pack.yaml` or the
/// YAML file itself. Returns the same in-memory shape both the CLI
/// `--json` mode and MCP `ls` tool emit; the human-mode CLI renderer
/// reads off the same struct so both surfaces stay byte-aligned.
///
/// # Errors
///
/// Returns a human-readable error string when the **root** manifest
/// cannot be loaded (missing, unreadable, or invalid YAML). Per-child
/// failures are surfaced inside the tree itself rather than aborting:
///
/// * Children declared but absent on disk render as nodes with
///   `unsynced = true`.
/// * Children whose `.grex/pack.yaml` failed to read or parse render
///   with `error = Some(...)` plus a stderr line carrying the same
///   detail.
pub fn build_ls_tree(pack_root: &Path) -> Result<LsTree, String> {
    let loader = FsPackLoader::new();
    let root_manifest = loader.load(pack_root).map_err(|e| format!("{e}"))?;
    let workspace = workspace_dir_for(pack_root);
    let mut counter: usize = 0;
    let id = next_id(&mut counter);
    let children = walk_children(&loader, &workspace, &root_manifest, &mut counter);
    Ok(LsTree {
        workspace: workspace.display().to_string(),
        tree: vec![LsNode {
            id,
            name: root_manifest.name.clone(),
            path: pack_root.display().to_string(),
            pack_type: root_manifest.r#type.as_str().to_string(),
            synthetic: false,
            unsynced: false,
            error: None,
            children,
        }],
    })
}

/// Resolve where children live on disk. When `pack_root` is a YAML
/// file, children sit beside the directory holding `.grex/`. Otherwise
/// the pack root IS the workspace root (post-v1.1.0 flat-sibling
/// layout).
fn workspace_dir_for(pack_root: &Path) -> PathBuf {
    if has_yaml_extension(pack_root) {
        // <ws>/.grex/pack.yaml → <ws>
        pack_root
            .parent()
            .and_then(Path::parent)
            .map_or_else(|| pack_root.to_path_buf(), Path::to_path_buf)
    } else {
        pack_root.to_path_buf()
    }
}

fn has_yaml_extension(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("yaml" | "yml"))
}

fn walk_children(
    loader: &FsPackLoader,
    workspace: &Path,
    parent: &PackManifest,
    counter: &mut usize,
) -> Vec<LsNode> {
    let mut out = Vec::with_capacity(parent.children.len());
    for child in &parent.children {
        let dest = workspace.join(child.effective_path());
        out.push(load_child_node(loader, workspace, child, &dest, counter));
    }
    out
}

/// Resolve a single declared child into an [`LsNode`]. Centralises the
/// four-way branch over loader outcomes so the recursion in
/// `walk_children` stays simple. The branching mirrors the per-fix
/// taxonomy: parsed-ok, synthetic plain-git, unsynced placeholder,
/// errored (parse / read / other).
fn load_child_node(
    loader: &FsPackLoader,
    workspace: &Path,
    child: &ChildRef,
    dest: &Path,
    counter: &mut usize,
) -> LsNode {
    match loader.load(dest) {
        Ok(manifest) => loaded_node(loader, workspace, &manifest, dest, counter, false),
        Err(TreeError::ManifestNotFound(_)) if dest_has_git_repo(dest) => {
            let manifest = synthesize_plain_git_manifest(child);
            loaded_node(loader, workspace, &manifest, dest, counter, true)
        }
        Err(TreeError::ManifestNotFound(_)) => unsynced_node(child, dest, counter),
        Err(e @ TreeError::ManifestParse { .. }) => errored_node(child, dest, counter, "parse", &e),
        Err(e @ TreeError::ManifestRead(_)) => errored_node(child, dest, counter, "read", &e),
        Err(e) => errored_node(child, dest, counter, "other", &e),
    }
}

/// Build an `LsNode` for a successfully loaded (real) or
/// canonical-synthesised manifest. Recurses through `walk_children` so
/// any future synthesised manifest carrying nested children would walk
/// transparently.
fn loaded_node(
    loader: &FsPackLoader,
    workspace: &Path,
    manifest: &PackManifest,
    dest: &Path,
    counter: &mut usize,
    synthetic: bool,
) -> LsNode {
    let id = next_id(counter);
    let children = walk_children(loader, workspace, manifest, counter);
    LsNode {
        id,
        name: manifest.name.clone(),
        path: dest.display().to_string(),
        pack_type: manifest.r#type.as_str().to_string(),
        synthetic,
        unsynced: false,
        error: None,
        children,
    }
}

/// Build the FIX-4 placeholder for a declared-but-absent child.
fn unsynced_node(child: &ChildRef, dest: &Path, counter: &mut usize) -> LsNode {
    let id = next_id(counter);
    LsNode {
        id,
        name: child.effective_path(),
        path: dest.display().to_string(),
        pack_type: PackType::Scripted.as_str().to_string(),
        synthetic: false,
        unsynced: true,
        error: None,
        children: Vec::new(),
    }
}

/// Build the FIX-3 placeholder for a child whose manifest failed to
/// load with a recoverable error (parse / read / other). Echoes the
/// underlying detail to stderr so operators see the same signal in
/// human and JSON modes.
fn errored_node(
    child: &ChildRef,
    dest: &Path,
    counter: &mut usize,
    kind: &str,
    err: &TreeError,
) -> LsNode {
    let message = format!("{err}");
    eprintln!("grex ls: {}: {message}", child.effective_path());
    let id = next_id(counter);
    LsNode {
        id,
        name: child.effective_path(),
        path: dest.display().to_string(),
        pack_type: PackType::Scripted.as_str().to_string(),
        synthetic: false,
        unsynced: false,
        error: Some(LsNodeError { kind: kind.to_string(), message }),
        children: Vec::new(),
    }
}

fn next_id(counter: &mut usize) -> usize {
    let id = *counter;
    *counter += 1;
    id
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn build_ls_tree_emits_root_for_meta_pack() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".grex")).unwrap();
        fs::write(root.join(".grex/pack.yaml"), "schema_version: \"1\"\nname: rootp\ntype: meta\n")
            .unwrap();

        let tree = build_ls_tree(root).expect("root manifest loads");
        assert_eq!(tree.tree.len(), 1);
        let node = &tree.tree[0];
        assert_eq!(node.name, "rootp");
        assert_eq!(node.pack_type, "meta");
        assert!(!node.synthetic);
        assert!(!node.unsynced);
        assert!(node.error.is_none());
        assert!(node.children.is_empty());
    }

    #[test]
    fn build_ls_tree_surfaces_synthetic_plain_git_child() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".grex")).unwrap();
        fs::write(
            root.join(".grex/pack.yaml"),
            "schema_version: \"1\"\nname: rootp\ntype: meta\nchildren:\n  - url: file:///dev/null\n    path: alpha\n",
        )
        .unwrap();
        // Plain-git child slot: `.git/` only, no `.grex/`.
        fs::create_dir_all(root.join("alpha/.git")).unwrap();

        let tree = build_ls_tree(root).expect("root manifest loads");
        let root_node = &tree.tree[0];
        assert_eq!(root_node.children.len(), 1);
        let child = &root_node.children[0];
        assert!(child.synthetic, "plain-git child must be flagged synthetic");
        assert_eq!(child.pack_type, "scripted");
        assert_eq!(child.name, "alpha");
        assert!(!child.unsynced);
        assert!(child.error.is_none());
        assert!(child.children.is_empty());
    }

    #[test]
    fn build_ls_tree_returns_error_for_missing_manifest() {
        let dir = tempdir().unwrap();
        let err = build_ls_tree(dir.path()).expect_err("missing manifest is fatal");
        assert!(!err.is_empty(), "error string must be human-readable");
    }

    /// FIX-4: a child declared in the parent manifest whose destination
    /// directory does not exist on disk MUST surface as an unsynced
    /// placeholder. Pre-FIX-4 the walk silently elided these, making a
    /// fresh meta-pack checkout look like an empty tree.
    #[test]
    fn build_ls_tree_surfaces_unsynced_child_placeholder() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".grex")).unwrap();
        fs::write(
            root.join(".grex/pack.yaml"),
            "schema_version: \"1\"\nname: rootp\ntype: meta\nchildren:\n  - url: file:///dev/null\n    path: alpha\n  - url: file:///dev/null\n    path: beta\n",
        )
        .unwrap();
        // No `alpha/` or `beta/` on disk.

        let tree = build_ls_tree(root).expect("root manifest loads");
        let root_node = &tree.tree[0];
        assert_eq!(root_node.children.len(), 2, "both declared children must appear");
        for (idx, expected) in ["alpha", "beta"].iter().enumerate() {
            let child = &root_node.children[idx];
            assert_eq!(child.name, *expected);
            assert!(!child.synthetic);
            assert!(child.unsynced, "unsynced placeholder expected for `{expected}`");
            assert!(child.error.is_none());
        }
    }

    /// FIX-3: a child whose on-disk `.grex/pack.yaml` is corrupt YAML
    /// MUST surface with `error.kind == "parse"` rather than being
    /// silently elided. This is the read-only diagnostic surface — the
    /// verb keeps walking the rest of the tree.
    #[test]
    fn build_ls_tree_surfaces_parse_error_on_corrupt_child_yaml() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".grex")).unwrap();
        fs::write(
            root.join(".grex/pack.yaml"),
            "schema_version: \"1\"\nname: rootp\ntype: meta\nchildren:\n  - url: file:///dev/null\n    path: corrupt\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("corrupt/.grex")).unwrap();
        // Garbage YAML — not a mapping at all.
        fs::write(root.join("corrupt/.grex/pack.yaml"), "::: not yaml ::: : :\n").unwrap();

        let tree = build_ls_tree(root).expect("root manifest loads");
        let root_node = &tree.tree[0];
        assert_eq!(root_node.children.len(), 1);
        let child = &root_node.children[0];
        let err = child.error.as_ref().expect("parse-error child must carry error envelope");
        assert_eq!(err.kind, "parse");
        assert!(!err.message.is_empty());
        assert!(!child.synthetic);
        assert!(!child.unsynced);
    }
}
