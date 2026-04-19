use crate::cli::args::{GlobalFlags, RmArgs};
use anyhow::Result;

pub fn run(_args: RmArgs, _global: &GlobalFlags) -> Result<()> {
    println!("grex rm: unimplemented (M1 scaffold)");
    Ok(())
}
