use crate::cli::args::{GlobalFlags, LsArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(
    _args: LsArgs,
    _global: &GlobalFlags,
    _cancel: &CancellationToken,
) -> Result<()> {
    println!("grex ls: unimplemented (M1 scaffold)");
    Ok(())
}
