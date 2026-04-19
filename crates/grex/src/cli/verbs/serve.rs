use crate::cli::args::{GlobalFlags, ServeArgs};
use anyhow::Result;

pub fn run(_args: ServeArgs, _global: &GlobalFlags) -> Result<()> {
    println!("grex serve: unimplemented (M1 scaffold)");
    Ok(())
}
