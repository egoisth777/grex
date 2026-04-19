use crate::cli::args::{GlobalFlags, UpdateArgs};
use anyhow::Result;

pub fn run(_args: UpdateArgs, _global: &GlobalFlags) -> Result<()> {
    println!("grex update: unimplemented (M1 scaffold)");
    Ok(())
}
