use crate::cli::args::{GlobalFlags, InitArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: InitArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    if global.json {
        return super::emit_unimplemented_json("init");
    }
    println!("grex init: unimplemented (M1 scaffold)");
    Ok(())
}
