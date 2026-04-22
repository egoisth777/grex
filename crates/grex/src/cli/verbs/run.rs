use crate::cli::args::{GlobalFlags, RunArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: RunArgs, _global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    println!("grex run: unimplemented (M1 scaffold)");
    Ok(())
}
