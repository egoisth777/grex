# feat-m6-1-parallel-scheduler

**Status**: draft

Bounded parallel `grex sync` via `tokio::sync::Semaphore`, gated by a new `--parallel N` flag. Introduces `crates/grex-core/src/scheduler.rs` and threads `Arc<Semaphore>` through `ExecCtx` into plugin dispatch. Serial semantics preserved with `--parallel 1`; `0` means unbounded; default `num_cpus::get()`.
