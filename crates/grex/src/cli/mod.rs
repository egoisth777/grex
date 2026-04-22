pub mod args;
pub mod verbs;

use anyhow::Result;
use args::{Cli, Verb};
use tokio_util::sync::CancellationToken;

pub fn run(cli: Cli) -> Result<()> {
    // feat-m7-1 stage 2: every verb accepts `&CancellationToken` as its
    // final argument. The CLI is a single-shot, foreground process that
    // has no out-of-band cancel channel, so it always passes a sentinel
    // — a freshly-constructed token that nobody ever flips. The MCP
    // server (stage 5+) constructs a real token tied to the request
    // lifetime and threads it through the same signature.
    let cancel = CancellationToken::new();
    match cli.verb {
        Verb::Init(a) => verbs::init::run(a, &cli.global, &cancel),
        Verb::Add(a) => verbs::add::run(a, &cli.global, &cancel),
        Verb::Rm(a) => verbs::rm::run(a, &cli.global, &cancel),
        Verb::Ls(a) => verbs::ls::run(a, &cli.global, &cancel),
        Verb::Status(a) => verbs::status::run(a, &cli.global, &cancel),
        Verb::Sync(a) => verbs::sync::run(a, &cli.global, &cancel),
        Verb::Update(a) => verbs::update::run(a, &cli.global, &cancel),
        Verb::Doctor(a) => verbs::doctor::run(a, &cli.global, &cancel),
        Verb::Serve(a) => verbs::serve::run(a, &cli.global, &cancel),
        Verb::Import(a) => verbs::import::run(a, &cli.global, &cancel),
        Verb::Run(a) => verbs::run::run(a, &cli.global, &cancel),
        Verb::Exec(a) => verbs::exec::run(a, &cli.global, &cancel),
        Verb::Teardown(a) => verbs::teardown::run(a, &cli.global, &cancel),
    }
}
