use crate::cli::args::{GlobalFlags, LsArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: LsArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    if global.json {
        return super::emit_unimplemented_json("ls");
    }
    println!("grex ls: unimplemented (M1 scaffold)");
    Ok(())
}
