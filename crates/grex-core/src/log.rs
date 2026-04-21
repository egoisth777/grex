//! Pluggable action logger (M4-B Stream S3).
//!
//! Defines the [`ActionLogger`] trait that executors and plugins use to
//! report step results and ad-hoc messages. Production code uses
//! [`TracingLogger`], which delegates to the `tracing` crate; tests may
//! supply a mock.
//!
//! The trait is intentionally tiny: no dynamic allocation on hot paths
//! beyond what `tracing` already does, no per-plugin knobs. M5 may grow
//! structured fields once the plugin surface stabilises.

use crate::execute::ExecStep;

/// Severity level for [`ActionLogger::log_message`].
///
/// Matches the standard `tracing` levels so [`TracingLogger`] can
/// forward without translation.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Sink for executor / plugin observability events.
///
/// Implementations must be `Send + Sync` so they can be shared across
/// task boundaries (the scheduler runs actions concurrently in M5).
#[doc(hidden)]
pub trait ActionLogger: Send + Sync {
    /// Record a completed [`ExecStep`]. Called once per action.
    fn log_step(&self, step: &ExecStep);

    /// Emit a free-form diagnostic message at the given level.
    fn log_message(&self, level: LogLevel, msg: &str);
}

/// Default [`ActionLogger`] backed by the `tracing` crate.
///
/// Step records land at `info` level on the `grex::exec` target; messages
/// honour their [`LogLevel`]. Consumers wire up `tracing-subscriber` (or
/// any alternative) in the host binary.
#[doc(hidden)]
#[derive(Debug, Default, Clone, Copy)]
pub struct TracingLogger;

impl ActionLogger for TracingLogger {
    fn log_step(&self, step: &ExecStep) {
        tracing::info!(target: "grex::exec", ?step, "action step");
    }

    fn log_message(&self, level: LogLevel, msg: &str) {
        match level {
            LogLevel::Trace => tracing::trace!(target: "grex::exec", "{}", msg),
            LogLevel::Debug => tracing::debug!(target: "grex::exec", "{}", msg),
            LogLevel::Info => tracing::info!(target: "grex::exec", "{}", msg),
            LogLevel::Warn => tracing::warn!(target: "grex::exec", "{}", msg),
            LogLevel::Error => tracing::error!(target: "grex::exec", "{}", msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execute::{ExecResult, ExecStep, PredicateOutcome, StepKind};
    use crate::pack::RequireOnFail;
    use std::borrow::Cow;
    use std::sync::Mutex;

    /// Minimal mock that counts dispatches per method.
    #[derive(Default)]
    struct MockLogger {
        steps: Mutex<usize>,
        messages: Mutex<Vec<(LogLevel, String)>>,
    }

    impl ActionLogger for MockLogger {
        fn log_step(&self, _step: &ExecStep) {
            *self.steps.lock().unwrap() += 1;
        }

        fn log_message(&self, level: LogLevel, msg: &str) {
            self.messages.lock().unwrap().push((level, msg.to_owned()));
        }
    }

    fn sample_step() -> ExecStep {
        ExecStep {
            action_name: Cow::Borrowed("require"),
            result: ExecResult::NoOp,
            details: StepKind::Require {
                outcome: PredicateOutcome::Unsatisfied,
                on_fail: RequireOnFail::Skip,
            },
        }
    }

    #[test]
    fn mock_logger_counts_step_calls() {
        let m = MockLogger::default();
        let s = sample_step();
        m.log_step(&s);
        m.log_step(&s);
        assert_eq!(*m.steps.lock().unwrap(), 2);
        assert!(m.messages.lock().unwrap().is_empty());
    }

    #[test]
    fn mock_logger_records_messages() {
        let m = MockLogger::default();
        m.log_message(LogLevel::Info, "hello");
        m.log_message(LogLevel::Error, "boom");
        let msgs = m.messages.lock().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0], (LogLevel::Info, "hello".to_owned()));
        assert_eq!(msgs[1], (LogLevel::Error, "boom".to_owned()));
    }

    #[test]
    fn tracing_logger_does_not_panic() {
        // Smoke test: without a global subscriber installed, tracing is a
        // no-op; we just confirm dispatch compiles and runs cleanly.
        let t = TracingLogger;
        t.log_step(&sample_step());
        t.log_message(LogLevel::Trace, "t");
        t.log_message(LogLevel::Debug, "d");
        t.log_message(LogLevel::Info, "i");
        t.log_message(LogLevel::Warn, "w");
        t.log_message(LogLevel::Error, "e");
    }

    #[test]
    fn trait_is_object_safe() {
        // Compile-time assertion: ActionLogger is dyn-compatible so
        // executors can hold `Arc<dyn ActionLogger>` in M5.
        let _: &dyn ActionLogger = &TracingLogger;
    }
}
