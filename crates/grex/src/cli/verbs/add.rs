use crate::cli::args::{AddArgs, GlobalFlags};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: AddArgs, _global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    println!("grex add: unimplemented (M1 scaffold)");
    Ok(())
}
