use crate::cli::args::{DoctorArgs, GlobalFlags};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(
    _args: DoctorArgs,
    _global: &GlobalFlags,
    _cancel: &CancellationToken,
) -> Result<()> {
    println!("grex doctor: unimplemented (M1 scaffold)");
    Ok(())
}
