use crate::cli::args::{GlobalFlags, RmArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(
    _args: RmArgs,
    _global: &GlobalFlags,
    _cancel: &CancellationToken,
) -> Result<()> {
    println!("grex rm: unimplemented (M1 scaffold)");
    Ok(())
}
