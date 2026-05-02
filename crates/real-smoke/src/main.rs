//! `real-smoke` runner binary.
//!
//! Real test logic lives in `crates/real-smoke/tests/` (owned by W2c). This
//! binary is intentionally thin — it prints usage and exits 0 so the crate
//! compiles as a normal `[[bin]]` target while orchestration happens through
//! `cargo test -p real-smoke --tests`.
//!
//! Run the regression suite via:
//!     cargo test -p real-smoke --tests -- --nocapture

fn main() {
    println!("real-smoke: black-box harness for the grex CLI.");
    println!();
    println!("This binary is a placeholder. Run the regression suite with:");
    println!("    cargo test -p real-smoke --tests -- --nocapture");
    println!();
    println!("Library modules (consumed by the regression tests):");
    println!("    real_smoke::worktree    -- RAII git-worktree manager");
    println!("    real_smoke::grex_cli    -- subprocess driver");
    println!("    real_smoke::assertions  -- fs + string-shape assertions");
    println!("    real_smoke::fixtures    -- pre-provisioned GitHub fixture URLs");
}
