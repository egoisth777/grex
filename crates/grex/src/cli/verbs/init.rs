use crate::cli::args::{GlobalFlags, InitArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: InitArgs, _global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    println!("grex init: unimplemented (M1 scaffold)");
    Ok(())
}
