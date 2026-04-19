use crate::cli::args::{ExecArgs, GlobalFlags};
use anyhow::Result;

pub fn run(_args: ExecArgs, _global: &GlobalFlags) -> Result<()> {
    println!("grex exec: unimplemented (M1 scaffold)");
    Ok(())
}
