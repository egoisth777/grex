use crate::cli::args::{GlobalFlags, RunArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: RunArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    if global.json {
        return super::emit_unimplemented_json("run");
    }
    println!("grex run: unimplemented (M1 scaffold)");
    Ok(())
}
