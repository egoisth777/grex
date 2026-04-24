pub mod add;
pub mod doctor;
pub mod exec;
pub mod import;
pub mod init;
pub mod ls;
pub mod rm;
pub mod run;
pub mod serve;
pub mod status;
pub mod sync;
pub mod teardown;
pub mod update;

/// Shared JSON helper for M1-scaffold stubs.
///
/// Emits `{"status": "unimplemented", "verb": "<name>"}` so that
/// `--json` callers still receive a parseable document when the verb
/// has not yet been implemented (issue #35 / M8-6). Human-output
/// behaviour is preserved on the non-JSON path.
pub(crate) fn emit_unimplemented_json(verb: &str) -> anyhow::Result<()> {
    let doc = serde_json::json!({
        "status": "unimplemented",
        "verb": verb,
    });
    println!("{}", serde_json::to_string(&doc)?);
    Ok(())
}
