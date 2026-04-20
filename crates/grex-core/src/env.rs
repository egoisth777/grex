//! Pluggable environment-variable resolver (M4-B Stream S3).
//!
//! Defines the [`EnvResolver`] trait that abstracts variable lookup for
//! executors, plugins, and tests. Production code uses [`VarEnv`] via the
//! blanket impl below; tests can substitute a mock without constructing a
//! full environment snapshot.
//!
//! The trait returns an owned [`String`] rather than a borrowed slice so
//! implementations that synthesise values (e.g. from a remote secret store
//! in future milestones) don't need a backing allocation that outlives the
//! call.

use crate::vars::VarEnv;

/// Abstract environment-variable lookup.
///
/// Must be `Send + Sync` so executors can share a single resolver across
/// tasks (the scheduler runs actions concurrently in M5).
pub trait EnvResolver: Send + Sync {
    /// Look up `name`. Returns `None` when unset.
    fn resolve(&self, name: &str) -> Option<String>;
}

impl EnvResolver for VarEnv {
    fn resolve(&self, name: &str) -> Option<String> {
        // VarEnv::get returns Option<&str>; own it for the trait signature.
        self.get(name).map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn var_env_blanket_impl_resolves_present() {
        let mut e = VarEnv::new();
        e.insert("FOO", "bar");
        assert_eq!(EnvResolver::resolve(&e, "FOO"), Some("bar".to_owned()));
        assert_eq!(e.get("FOO"), Some("bar"));
    }

    #[test]
    fn var_env_blanket_impl_returns_none_for_absent() {
        let e = VarEnv::new();
        assert_eq!(EnvResolver::resolve(&e, "NOPE"), None);
    }

    #[test]
    fn trait_is_object_safe() {
        // Compile-time assertion: EnvResolver is dyn-compatible so
        // executors can hold `Arc<dyn EnvResolver>` in M5.
        let e = VarEnv::new();
        let _: &dyn EnvResolver = &e;
    }

    #[test]
    fn resolver_value_matches_var_env_get() {
        // Parity check: the trait never diverges from the underlying
        // VarEnv::get semantics (including Windows case folding).
        let mut e = VarEnv::new();
        e.insert("ALPHA", "one");
        e.insert("BETA", "two");
        for name in ["ALPHA", "BETA", "MISSING"] {
            assert_eq!(e.resolve(name), e.get(name).map(str::to_owned));
        }
    }
}
