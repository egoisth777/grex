use crate::cli::args::{GlobalFlags, StatusArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: StatusArgs, _global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    println!("grex status: unimplemented (M1 scaffold)");
    Ok(())
}
