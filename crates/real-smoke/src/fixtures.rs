//! Hardcoded GitHub fixture URL registry.
//!
//! These six repositories are **pre-provisioned** by the provisioning script
//! (`scripts/provision-fixtures.ps1`/`.sh`, owned by a sibling worker) and live
//! under the `egoisth777` GitHub account. Each fixture exercises a specific
//! topology relevant to grex's walker / lockfile invariants:
//!
//! | Constant            | Topology                                   |
//! |---------------------|--------------------------------------------|
//! | [`FIXTURE_LEAF`]    | Single leaf pack, no children.             |
//! | [`FIXTURE_META_FLAT`] | Meta-pack with N flat sub-packs.         |
//! | [`FIXTURE_META_NESTED`] | Meta-pack with nested sub-meta-packs.  |
//! | [`FIXTURE_CYCLE_A`] | Cycle-detection fixture — A imports B.     |
//! | [`FIXTURE_CYCLE_B`] | Cycle-detection fixture — B imports A.     |
//! | [`FIXTURE_BROKEN`]  | Manifest with malformed YAML / schema.     |
//!
//! HTTPS URLs are used because the fixtures are PUBLIC repositories — they
//! clone anonymously without auth, which keeps CI portable (no SSH key secret
//! required). The provisioning script keeps SSH for the operator's local push
//! path; the harness runtime + CI only ever clone (read), via HTTPS.

/// Single leaf pack, no children. Smoke-tests basic clone + checkout.
pub const FIXTURE_LEAF: &str = "https://github.com/egoisth777/grex-test-leaf.git";

/// Meta-pack containing N flat sub-packs (no further nesting).
pub const FIXTURE_META_FLAT: &str = "https://github.com/egoisth777/grex-test-meta-flat.git";

/// Meta-pack with sub-meta-packs nested at least one level deep.
pub const FIXTURE_META_NESTED: &str = "https://github.com/egoisth777/grex-test-meta-nested.git";

/// Cycle-detection fixture, side A. Imports `FIXTURE_CYCLE_B`.
pub const FIXTURE_CYCLE_A: &str = "https://github.com/egoisth777/grex-test-cycle-a.git";

/// Cycle-detection fixture, side B. Imports `FIXTURE_CYCLE_A`.
pub const FIXTURE_CYCLE_B: &str = "https://github.com/egoisth777/grex-test-cycle-b.git";

/// Manifest with intentionally malformed YAML / invalid schema.
pub const FIXTURE_BROKEN: &str = "https://github.com/egoisth777/grex-test-broken-manifest.git";

/// All six fixture URLs, in declaration order. Convenience for batch
/// provisioning / cache warm-up scripts.
pub const ALL: &[&str] = &[
    FIXTURE_LEAF,
    FIXTURE_META_FLAT,
    FIXTURE_META_NESTED,
    FIXTURE_CYCLE_A,
    FIXTURE_CYCLE_B,
    FIXTURE_BROKEN,
];
