use clap::Parser;
use grex_cli::cli;

fn main() -> anyhow::Result<()> {
    let args = cli::args::Cli::parse();

    // feat-m7-1 stage 8: when running `grex serve`, defer ALL tracing
    // setup to `grex_mcp::GrexMcpServer::run`, which pins the writer to
    // stderr to keep stdout reserved for JSON-RPC frames. Calling
    // `tracing_subscriber::fmt().init()` here would install a global
    // stdout writer that the server's later `set_global_default` cannot
    // displace, breaking the stdio discipline contract.
    //
    // Stage 5 wiring note #7: rmcp emits `tracing::info!` from
    // `serve_inner` (loud at INFO). Default the filter to
    // `grex=info,rmcp=warn` so a vanilla `grex serve` is quiet on
    // stderr while still surfacing grex-side diagnostics.
    if matches!(args.verb, cli::args::Verb::Serve(_)) {
        // feat-m7-1 stage 8 + Stage 5 wiring note #7:
        // `grex serve` uses stdio for JSON-RPC, so the tracing writer
        // MUST be pinned to stderr. Default the filter to
        // `grex=info,rmcp=warn` to silence rmcp's loud
        // "Service initialized as server" INFO at startup while still
        // surfacing grex-side diagnostics. Honour any user RUST_LOG.
        //
        // Done in main (not in `verbs/serve.rs`) so the subscriber is
        // live before any `tracing::*!` macro fires. `grex_mcp::run`
        // also calls `init_stderr_tracing` defensively, but the second
        // `set_global_default` is a no-op once we've set one here.
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("grex=info,rmcp=warn"));
        tracing_subscriber::fmt().with_writer(std::io::stderr).with_env_filter(filter).init();
    } else {
        // Non-serve verbs may emit `--json` envelopes on stdout (e.g.
        // `grex sync --json`, `grex doctor --json`). The default
        // `tracing_subscriber::fmt()` writer is `io::stdout`, which would
        // interleave any `tracing::warn!` / `info!` line with the JSON
        // payload and break consumers that parse stdout. Pin the writer
        // to stderr — the same discipline the serve branch uses for
        // JSON-RPC framing — so structured stdout stays JSON-only.
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("grex=info")),
            )
            .with_writer(std::io::stderr)
            .init();
    }

    cli::run(args)
}
