//! ADVISORY pointer only -- not a wall-budget gate.
//!
//! This file previously timed `Instant::now()` with no work and asserted
//! `elapsed < 50`, which always passed. That placeholder was removed.
//!
//! Real snap-export latency/size gates live in:
//!   cargo test -p graphzero-test-support --test snap_export_perf_gate -- --nocapture
//!   crates/graphzero-test-support/src/gates/snap_export_perf_gate.rs
//!
//! Full snap+export+blast+handoff as one CI wall gate is not wired here.
//! Hyperfine / harness notes: scripts/perf/hyperfine_snap_export.sh,
//! scripts/perf/cargo_bench_harness.sh.
//!
//! This path is intentionally not a crate test target (scripts/perf is not a
//! package member). Do not assert process wall budgets against empty work.

#[cfg(test)]
mod snap_export_full_loop {
    /// Documents that the real gate is elsewhere; never asserts a wall budget.
    #[test]
    fn full_loop_gate_is_delegated_to_snap_export_perf_gate() {
        // Advisory: keep this test free of Instant/empty-work budget asserts.
        // Run the measured gate:
        //   cargo test -p graphzero-test-support --test snap_export_perf_gate -- --nocapture
        const REAL_GATE: &str = "graphzero-test-support::snap_export_perf_gate";
        assert!(
            !REAL_GATE.is_empty(),
            "pointer to measured snap_export_perf_gate must remain non-empty"
        );
        eprintln!(
            "advisory: full snap+export+blast+handoff wall gate is not implemented here; \
             use {REAL_GATE} for measured snap-export latency/size"
        );
    }
}
