use crate::cli::args::{GlobalFlags, StatusArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: StatusArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    if global.json {
        return super::emit_unimplemented_json("status");
    }
    println!("grex status: unimplemented (M1 scaffold)");
    Ok(())
}
