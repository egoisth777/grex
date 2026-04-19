use clap::Parser;

mod cli;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("grex=info")),
        )
        .init();

    let args = cli::args::Cli::parse();
    cli::run(args)
}
