use crate::cli::args::{AddArgs, GlobalFlags};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: AddArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    if global.json {
        return super::emit_unimplemented_json("add");
    }
    println!("grex add: unimplemented (M1 scaffold)");
    Ok(())
}
