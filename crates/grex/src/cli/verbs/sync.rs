use crate::cli::args::{GlobalFlags, SyncArgs};
use anyhow::Result;

pub fn run(_args: SyncArgs, _global: &GlobalFlags) -> Result<()> {
    println!("grex sync: unimplemented (M1 scaffold)");
    Ok(())
}
