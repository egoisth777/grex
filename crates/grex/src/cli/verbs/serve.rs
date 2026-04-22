//! `grex serve` — launch the MCP stdio server (feat-m7-1 stage 8).
//!
//! Wires the rmcp framework (built in `grex-mcp`) to the actual stdio
//! transport on top of a Tokio runtime. The runtime is constructed
//! per-invocation: `cli::run` is sync, but rmcp's stdio loop is async,
//! so we bridge with a current-thread runtime started on demand.
//!
//! ## Tracing discipline
//!
//! `grex-mcp::GrexMcpServer::run` reinstalls a stderr-pinned subscriber
//! using `EnvFilter::try_from_default_env()` with `info` fallback. To
//! silence rmcp's "Service initialized as server" INFO line in
//! production, `main.rs` defaults `RUST_LOG` to `grex=info,rmcp=warn`
//! when no env var is present (Stage 5 wiring note #7). Tests can
//! override with `RUST_LOG=…`.

use crate::cli::args::{GlobalFlags, ServeArgs};
use anyhow::{Context, Result};
use grex_core::{Registry, Scheduler};
use grex_mcp::{GrexMcpServer, ServerState};
use tokio_util::sync::CancellationToken;

// `_cancel` is intentionally unused here. The verb-level CancellationToken from
// `cli::run` is plumbed for future global-shutdown wiring (Stage 8+). Per-request
// cancellation is handled by rmcp's internal local_ct_pool (see grex-mcp Stage 7
// commit and lib.rs comment block above serve()).
pub fn run(args: ServeArgs, _global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    let workspace = match args.workspace {
        Some(p) => p,
        None => std::env::current_dir().context("resolve cwd for --workspace default")?,
    };
    let manifest_path = match args.manifest {
        Some(p) => p,
        None => workspace.join("grex.jsonl"),
    };
    let parallel = resolve_parallel(args.parallel);

    let scheduler = Scheduler::new(parallel);
    let registry = Registry::default();
    let state = ServerState::new(scheduler, registry, manifest_path, workspace);
    let server = GrexMcpServer::new(state);

    // Bridge sync `cli::run` → async rmcp loop. A fresh single-thread
    // runtime is sufficient: the server has no other in-process work,
    // and rmcp drives its own request fan-out via tokio::spawn.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime for grex serve")?;

    rt.block_on(async move {
        let transport = rmcp::transport::stdio();
        // Pre-`run` info line so stderr has at least one tracing event
        // even with the default `grex=info,rmcp=warn` filter that hides
        // rmcp's "Service initialized as server" log. Useful for ops
        // visibility (PID, parallel cap) and asserted by 8.T3.
        tracing::info!(
            target: "grex",
            parallel,
            "grex serve: MCP stdio transport ready",
        );
        server.run(transport).await.context("grex-mcp server exited with error")
    })
}

/// Resolve the `--parallel` flag to a concrete worker count, falling
/// back to `std::thread::available_parallelism()` when unset and to `1`
/// as the conservative floor when even that fails (uncommon — reserved
/// VMs / sandboxed CI). Matches the harness contract in
/// `.omne/cfg/concurrency.md`.
fn resolve_parallel(opt: Option<u32>) -> usize {
    match opt {
        Some(n) => n as usize,
        None => std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
    }
}
