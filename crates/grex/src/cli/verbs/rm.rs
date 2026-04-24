use crate::cli::args::{GlobalFlags, RmArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: RmArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    if global.json {
        return super::emit_unimplemented_json("rm");
    }
    println!("grex rm: unimplemented (M1 scaffold)");
    Ok(())
}
