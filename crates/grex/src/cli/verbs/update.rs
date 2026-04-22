use crate::cli::args::{GlobalFlags, UpdateArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: UpdateArgs, _global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    println!("grex update: unimplemented (M1 scaffold)");
    Ok(())
}
