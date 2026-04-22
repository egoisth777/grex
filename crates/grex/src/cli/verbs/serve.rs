use crate::cli::args::{GlobalFlags, ServeArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(
    _args: ServeArgs,
    _global: &GlobalFlags,
    _cancel: &CancellationToken,
) -> Result<()> {
    println!("grex serve: unimplemented (M1 scaffold)");
    Ok(())
}
