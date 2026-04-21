pub mod args;
pub mod verbs;

use anyhow::Result;
use args::{Cli, Verb};

pub fn run(cli: Cli) -> Result<()> {
    match cli.verb {
        Verb::Init(a) => verbs::init::run(a, &cli.global),
        Verb::Add(a) => verbs::add::run(a, &cli.global),
        Verb::Rm(a) => verbs::rm::run(a, &cli.global),
        Verb::Ls(a) => verbs::ls::run(a, &cli.global),
        Verb::Status(a) => verbs::status::run(a, &cli.global),
        Verb::Sync(a) => verbs::sync::run(a, &cli.global),
        Verb::Update(a) => verbs::update::run(a, &cli.global),
        Verb::Doctor(a) => verbs::doctor::run(a, &cli.global),
        Verb::Serve(a) => verbs::serve::run(a, &cli.global),
        Verb::Import(a) => verbs::import::run(a, &cli.global),
        Verb::Run(a) => verbs::run::run(a, &cli.global),
        Verb::Exec(a) => verbs::exec::run(a, &cli.global),
        Verb::Teardown(a) => verbs::teardown::run(a, &cli.global),
    }
}
