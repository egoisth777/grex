use crate::cli::args::{DoctorArgs, GlobalFlags};
use anyhow::Result;

pub fn run(_args: DoctorArgs, _global: &GlobalFlags) -> Result<()> {
    println!("grex doctor: unimplemented (M1 scaffold)");
    Ok(())
}
