use crate::cli::args::{GlobalFlags, ImportArgs};
use anyhow::Result;

pub fn run(_args: ImportArgs, _global: &GlobalFlags) -> Result<()> {
    println!("grex import: unimplemented (M1 scaffold)");
    Ok(())
}
