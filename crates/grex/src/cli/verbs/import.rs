use crate::cli::args::{GlobalFlags, ImportArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: ImportArgs, _global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    println!("grex import: unimplemented (M1 scaffold)");
    Ok(())
}
