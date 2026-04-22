//! L3 — CLI ↔ MCP per-verb parity (Stage 5 of feat-m7-2).
//!
//! For each verb in [`grex_mcp::VERBS_EXPOSED`] we drive **both** surfaces
//! against the same per-test fixture and assert their outcomes agree on
//! the only contract both surfaces can satisfy today: structural shape
//! parity expressed as a [`common::ParitySignal`].
//!
//! ## ParitySignal contract (m7-2-bounded)
//!
//! Spec line 98 calls for `assert_eq!(normalize(cli_json), normalize(mcp_json))`
//! — strict byte-equal of the two normalised JSON payloads. That is the
//! intended **destination** contract; it cannot be reached today because
//! none of the 11 CLI verbs wire their output through the global
//! `--json` flag (it is parsed in `crates/grex/src/cli/args.rs:16` but
//! never consumed by any verb's `run()` — `crates/grex/src/cli/verbs/
//! sync.rs:30` ignores `global.json`; the 9 stub verbs print the literal
//! string `"grex <verb>: unimplemented (M1 scaffold)"`).
//!
//! What both surfaces DO carry today is a structured "this verb is
//! M7-1-stub-only" signal classified by [`common::ParitySignal`]:
//!
//! | surface | shape                                                            |
//! |---------|------------------------------------------------------------------|
//! | CLI     | stdout text contains `"unimplemented"` → `Unimplemented`         |
//! |         | OR non-zero exit with no marker → `PackOpError`                  |
//! | MCP     | `CallToolResult { isError: Some(true) }` whose `content[0].text` |
//! |         | parses as a JSON envelope with `data.kind = "not_implemented"`   |
//! |         | → `Unimplemented`, OR `data.kind = "pack_op"` → `PackOpError`    |
//!
//! [`common::assert_parity`] asserts both signals match per verb. When
//! CLI `--json` wiring lands (m7-4 scope alongside real verb impls),
//! this helper flips to the spec-shaped strict byte-equal — call sites
//! below stay unchanged.
//!
//! See `openspec/changes/feat-m7-2-mcp-test-harness/spec.md` §"Known
//! limitations" entry 5 for the spec-side documentation of this gap.
//!
//! ## tools/list `>=` preflight
//!
//! Per spec §"Tool enumeration" the parity preflight uses `>=` (not
//! `==`) so future MCP-only tools never retrip the `tools/list.len()`
//! check. The preflight is folded into `drive_mcp` (per tasks 5.4) so a
//! registry shrink fails the per-verb test with a clear diagnostic
//! before the parity assertion even runs.

use grex_mcp::VERBS_EXPOSED;

#[path = "common/mod.rs"]
mod common;

use common::assert_parity;

/// Compile-time pin: 11 parametric tests below = `VERBS_EXPOSED.len()`.
/// The const-assert keeps a future contributor who adds an MCP-only tool
/// from silently shrinking the parity matrix.
const _: () = assert!(VERBS_EXPOSED.len() == 11);

#[tokio::test]
async fn parity_init() {
    assert_parity("init").await;
}

#[tokio::test]
async fn parity_add() {
    assert_parity("add").await;
}

#[tokio::test]
async fn parity_rm() {
    assert_parity("rm").await;
}

#[tokio::test]
async fn parity_ls() {
    assert_parity("ls").await;
}

#[tokio::test]
async fn parity_status() {
    assert_parity("status").await;
}

#[tokio::test]
async fn parity_sync() {
    // Substantive case: `sync` is the only verb with a real core impl.
    // Both surfaces hit `grex_core::sync::run` against an absolute
    // tempdir-rooted path that does not exist as `.grex/pack.yaml` and
    // produce a structural error. CLI prints to stderr and exits
    // non-zero (→ `PackOpError`); MCP returns `packop_error(...)` (→
    // `PackOpError`). Both signal the same outcome class — the spec's
    // strict byte-equal awaits CLI `--json` wiring.
    assert_parity("sync").await;
}

#[tokio::test]
async fn parity_update() {
    assert_parity("update").await;
}

#[tokio::test]
async fn parity_doctor() {
    assert_parity("doctor").await;
}

#[tokio::test]
async fn parity_import() {
    assert_parity("import").await;
}

#[tokio::test]
async fn parity_run() {
    assert_parity("run").await;
}

#[tokio::test]
async fn parity_exec() {
    assert_parity("exec").await;
}
