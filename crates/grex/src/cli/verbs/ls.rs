//! `grex ls` — read-only tree listing of the pack graph.
//!
//! Walks the workspace from a root `pack.yaml` without cloning, fetching,
//! or executing anything. The actual walk is delegated to the shared
//! [`grex_core::build_ls_tree`] backend so the CLI and the MCP `ls` tool
//! stay byte-aligned (FIX-2 deduplication of the v1.1.1 reviewer
//! findings). This file owns only argument plumbing, error-envelope
//! shaping, and the human-mode renderer.
//!
//! Output:
//! * Default — box-drawing tree, one node per line, `(<type>)` suffix
//!   per node, `~ ` prefix on synthetic packs and `(scripted, synthetic)`
//!   in their suffix. Unsynced declared children render as
//!   `<name> (declared, unsynced)`. Children whose on-disk
//!   `.grex/pack.yaml` failed to parse/read render with an
//!   `[error: parse]` (or `read` / `other`) suffix and the underlying
//!   detail is echoed to stderr by the shared backend.
//! * `--json` — `{"workspace": "<abs>", "tree": [<node>, ...]}` envelope
//!   pretty-printed via `serde_json`. Each node carries
//!   `{id, name, path, type, synthetic, children}` plus, when relevant,
//!   `unsynced: true` or `error: {kind, message}`.
//!
//! Errors loading the **root** manifest surface as a structured envelope
//! in JSON mode and as `stderr + exit 2` in human mode, matching the
//! `sync` verb's usage convention. The envelope's `kind` is `"tree"`
//! (matching the documented per-verb taxonomy in
//! `man/reference/cli-json.md` — FIX-5).

use crate::cli::args::{GlobalFlags, LsArgs};
use anyhow::Result;
use grex_core::{build_ls_tree, LsNode, LsTree};
use tokio_util::sync::CancellationToken;

pub fn run(args: LsArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    let pack_root = match args.pack_root.clone() {
        Some(p) => p,
        None => std::env::current_dir()?,
    };

    match build_ls_tree(&pack_root) {
        Ok(tree) => {
            if global.json {
                emit_json(&tree);
            } else {
                render_tree(&tree);
            }
            Ok(())
        }
        Err(detail) => {
            if global.json {
                emit_json_error("tree", &detail);
            } else {
                eprintln!("grex ls: {detail}");
            }
            std::process::exit(2);
        }
    }
}

// --- Rendering -------------------------------------------------------------

fn render_tree(tree: &LsTree) {
    for root in &tree.tree {
        println!("{}", node_label(root));
        let count = root.children.len();
        for (idx, child) in root.children.iter().enumerate() {
            render_subtree(child, "", idx + 1 == count);
        }
    }
}

fn render_subtree(node: &LsNode, prefix: &str, is_last: bool) {
    let connector = if is_last { "└── " } else { "├── " };
    println!("{prefix}{connector}{}", node_label(node));
    let next_prefix = format!("{prefix}{}", if is_last { "    " } else { "│   " });
    let count = node.children.len();
    for (idx, child) in node.children.iter().enumerate() {
        render_subtree(child, &next_prefix, idx + 1 == count);
    }
}

/// Build the trailing label for a single node. The four exclusive
/// states (loaded / synthetic / unsynced / errored) each get a
/// distinct visible suffix so an operator scanning `grex ls` output
/// can triage at a glance.
fn node_label(node: &LsNode) -> String {
    if let Some(err) = &node.error {
        return format!("{} ({}) [error: {}]", node.name, node.pack_type, err.kind);
    }
    if node.unsynced {
        return format!("{} (declared, unsynced)", node.name);
    }
    let marker = if node.synthetic { "~ " } else { "" };
    let suffix = if node.synthetic {
        format!("({}, synthetic)", node.pack_type)
    } else {
        format!("({})", node.pack_type)
    };
    format!("{marker}{} {suffix}", node.name)
}

// --- JSON ------------------------------------------------------------------

fn emit_json(tree: &LsTree) {
    // v1.3.0: dual-emit "workspace" + "pack". v1.4.0 drops "workspace".
    // `LsTree` lives in `grex-core` and keeps its `workspace` field; the
    // CLI envelope wraps it here so the additive `pack` key lands without
    // perturbing the shared backend struct. Order is `workspace` first,
    // `pack` second (byte-stable for diff-friendly consumers).
    let doc = serde_json::json!({
        "workspace": tree.workspace,
        "pack": tree.workspace,
        "tree": tree.tree,
    });
    if let Ok(s) = serde_json::to_string_pretty(&doc) {
        println!("{s}");
    }
}

fn emit_json_error(kind: &str, message: &str) {
    let doc = serde_json::json!({
        "verb": "ls",
        "error": {
            "kind": kind,
            "message": message,
        },
    });
    if let Ok(s) = serde_json::to_string(&doc) {
        println!("{s}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use grex_core::LsNodeError;

    fn leaf_loaded(name: &str, pack_type: &str) -> LsNode {
        LsNode {
            id: 0,
            name: name.to_string(),
            path: name.to_string(),
            pack_type: pack_type.to_string(),
            synthetic: false,
            unsynced: false,
            error: None,
            children: Vec::new(),
        }
    }

    fn leaf_synthetic(name: &str) -> LsNode {
        LsNode {
            id: 0,
            name: name.to_string(),
            path: name.to_string(),
            pack_type: "scripted".to_string(),
            synthetic: true,
            unsynced: false,
            error: None,
            children: Vec::new(),
        }
    }

    fn leaf_unsynced(name: &str) -> LsNode {
        LsNode {
            id: 0,
            name: name.to_string(),
            path: name.to_string(),
            pack_type: "scripted".to_string(),
            synthetic: false,
            unsynced: true,
            error: None,
            children: Vec::new(),
        }
    }

    fn leaf_errored(name: &str, kind: &str) -> LsNode {
        LsNode {
            id: 0,
            name: name.to_string(),
            path: name.to_string(),
            pack_type: "scripted".to_string(),
            synthetic: false,
            unsynced: false,
            error: Some(LsNodeError { kind: kind.to_string(), message: "boom".to_string() }),
            children: Vec::new(),
        }
    }

    #[test]
    fn label_for_declarative_has_plain_suffix() {
        assert_eq!(node_label(&leaf_loaded("warp-cfg", "declarative")), "warp-cfg (declarative)");
    }

    #[test]
    fn label_for_synthetic_has_tilde_prefix_and_synthetic_suffix() {
        assert_eq!(node_label(&leaf_synthetic("algo-leet")), "~ algo-leet (scripted, synthetic)");
    }

    #[test]
    fn label_for_meta_has_meta_suffix() {
        assert_eq!(node_label(&leaf_loaded("dev-env", "meta")), "dev-env (meta)");
    }

    #[test]
    fn label_for_unsynced_marks_declared_unsynced() {
        assert_eq!(node_label(&leaf_unsynced("alpha")), "alpha (declared, unsynced)");
    }

    #[test]
    fn label_for_errored_carries_error_kind() {
        assert_eq!(
            node_label(&leaf_errored("corrupt", "parse")),
            "corrupt (scripted) [error: parse]"
        );
    }
}
