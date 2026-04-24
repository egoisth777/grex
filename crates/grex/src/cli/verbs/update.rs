use crate::cli::args::{GlobalFlags, UpdateArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(_args: UpdateArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    if global.json {
        return super::emit_unimplemented_json("update");
    }
    println!("grex update: unimplemented (M1 scaffold)");
    Ok(())
}
