//! Registration bridge for the five transport-neutral zero-testkit promises.

pub const SHARED_PROMISES: [&str; 5] = [
    "packaging_lifecycle",
    "packaging_e2e",
    "racc_durability_matrix",
    "readme_claims",
    "readme_command_audit",
];

/// Return the canonical zero-testkit runner without copying its implementation.
pub fn run_all(harness: &mut dyn zero_testkit::EngineHarness) -> Vec<zero_testkit::SuiteReport> {
    zero_testkit::run_all(harness)
}
