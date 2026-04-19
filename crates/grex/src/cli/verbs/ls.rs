use crate::cli::args::{GlobalFlags, LsArgs};
use anyhow::Result;

pub fn run(_args: LsArgs, _global: &GlobalFlags) -> Result<()> {
    println!("grex ls: unimplemented (M1 scaffold)");
    Ok(())
}
