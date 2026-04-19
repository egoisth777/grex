use crate::cli::args::{GlobalFlags, StatusArgs};
use anyhow::Result;

pub fn run(_args: StatusArgs, _global: &GlobalFlags) -> Result<()> {
    println!("grex status: unimplemented (M1 scaffold)");
    Ok(())
}
