use crate::cli::args::{ExecArgs, GlobalFlags};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: ExecArgs, _global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    println!("grex exec: unimplemented (M1 scaffold)");
    Ok(())
}
