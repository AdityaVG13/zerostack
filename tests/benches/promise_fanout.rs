//! Cross-engine `Promise.all` fan-out vs sequential awaits — benchmark definition.
//!
//! Bead: `zerostack-promise-fanout-bench-057b`
//! Background: moved from GraphZero `graphzero-c5yy`. `Promise.all` fan-out across
//! `zero.fs` / `zero.graph` / `zero.token` in one aggregate plan is hub composition.
//! GraphZero-only JSON-DAG parallel groups are not cross-engine proof.
//!
//! Technical approach (from bead):
//! - Benchmark + doc pattern: parallel `orient + snap + search` (or equivalent)
//!   vs sequential `await`s in **one** hub plan.
//! - Publish wall-clock and token deltas. Depends on FSZero / TokenZero connectors
//!   and bounded microtask contracts.
//! - Do not treat GraphZero-only parallel groups as this bead.
//!
//! Success criteria (from bead):
//! - Parallel vs sequential numbers published with hardware / commit / binary SHA.
//! - Token deltas labeled (not unlabeled %).
//! - GraphZero `c5yy` stays a pointer.
//!
//! Evidence status — honest blocker (no invented numbers):
//! - This file is the **definition artifact**. Numbers require an external RCH run
//!   with live pinned engines (`FSZero`, `GraphZero`, `TokenZero` at `origin/main`),
//!   bounded microtask contracts, and hardware/commit/binary SHA capture.
//! - Operator forbade wasteful benches this session (bead notes). The scheduled
//!   measurement run is deferred. This commit publishes the definition without
//!   inventing p50/p95 or token counts.
//! - `cargo test` here validates the definition shape; it does **not** run the
//!   wall-clock campaign. Do not ratchet keep-window / p50 from this file.
//!
//! How to run when unblocked (do not run on this Mac without RCH):
//! ```sh
//! rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack \
//!   cargo test --test promise_fanout -- --test-threads=1 --nocapture
//! # or the bead's original lib probe (pre-benches layout):
//! # rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack \
//! #   cargo test -p zero-codemode --lib promise_fanout -- --test-threads=1
//! ```
//! Never full-workspace `cargo test`. Always `--test-threads=1` and
//! `CARGO_TARGET_DIR=/tmp/rch_target_zerostack` exactly.
//!
//! Anchors:
//! - `benchmarks/savings-bench.json` — existing Exact/envelope/call-fusion bench (no fan-out).
//! - `tests/zerostack_harness_store_cas_microbench.rs` — bench-shaped harness pattern reused.
//! - `crates/zero-codemode/src/interpreter.rs` — `Promise.all` host contract (no numbers here).
//! - `.bench-history/savings-bench.latest.json` — ratchet seed (cv_pct null, not fan-out).

use serde_json::{Value, json};

const SCHEMA: &str = "zerostack.promise-fanout-bench.v1";
const BEAD: &str = "zerostack-promise-fanout-bench-057b";
const POINTER_BEAD: &str = "graphzero-c5yy";

/// Definition report — no invented measurements. Wall-clock / token fields are
/// `null` until an external RCH run with pinned engines fills them.
/// This file is a definition artifact, not an executable benchmark. Actual
/// fanout measurement is an external RCH campaign (see header comment).
#[allow(dead_code)]
fn definition_report() -> Value {
    json!({
        "schema": SCHEMA,
        "bead": BEAD,
        "pointer_bead": POINTER_BEAD,
        "status": "definition_only",
        "blocker": {
            "reason": "external_rch_run_required",
            "detail": "Operator forbade wasteful benches this session; requires RCH run with live FSZero/GraphZero/TokenZero at pinned origin/main and bounded microtask contracts.",
            "required_evidence": [
                "hardware (os/arch/cpu_model/kernel)",
                "commit SHA",
                "binary SHA (aggregate_host + engine binaries)",
                "cargo_profile (release-perf with RUSTFLAGS=\"-C force-frame-pointers=yes\")",
                "wall-clock p50/p95 sequential vs fan-out (n, iterations, cv_pct)",
                "token deltas labeled (billed_tokens/raw_tokens/visible_tokens, not unlabeled %)",
                "plan path (parallel Promise.all vs sequential awaits, one hub plan)"
            ]
        },
        "benchmark": {
            "name": "cross-engine Promise.all fan-out vs sequential awaits",
            "composition": "one hub aggregate plan across zero.fs / zero.graph / zero.token",
            "parallel_plan": "await Promise.all([zero.fs.search(...), zero.graph.orient(...), zero.token.read(...)]) // or orient+snap+search equivalent",
            "sequential_plan": "await zero.fs.search(...); await zero.graph.orient(...); await zero.token.read(...);",
            "do_not": [
                "Treat GraphZero-only JSON-DAG parallel groups as cross-engine proof",
                "Invent p50/p95 or token numbers without hardware/commit/binary SHA",
                "Ratchet keep-window from this definition",
                "Sum savingsBytes into Exact tokens"
            ]
        },
        "metrics": {
            "wall_clock_sequential": { "p50_ms": null, "p95_ms": null, "n": null, "cv_pct": null, "iterations": null },
            "wall_clock_fanout": { "p50_ms": null, "p95_ms": null, "n": null, "cv_pct": null, "iterations": null },
            "token_deltas": { "billed_tokens": null, "raw_tokens": null, "visible_tokens": null, "recovery_tokens": null, "exact_ref_tokens": null, "note": "TokenZero WorkerTokenAccounting only; FS adapters are uncertified (input_token_cost:0)" },
            "environment": { "os": null, "arch": null, "cpu_model": null, "kernel": null, "rustc_version": null, "cargo_version": null, "git_sha": null, "cargo_profile": null, "binary_sha": null }
        },
        "existing_suite_reuse": {
            "benchmarks_md": "benchmarks/benchmarks.md",
            "savings_bench": "benchmarks/savings-bench.json",
            "harness_pattern": "tests/zerostack_harness_store_cas_microbench.rs",
            "harness_helpers": "crates/zerostack-harness/src/measure.rs (measure_with_teardown, WARMUP/MIN/MAX_ITERS, TARGET_DURATION) and hot_path_profile_snapshot",
            "profile": "profile.release-perf (opt-level=3, codegen-units=1, lto=fat, debug=line-tables-only, strip=false, panic=abort) with RUSTFLAGS=\"-C force-frame-pointers=yes\""
        },
        "command_when_unblocked": "rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test --test promise_fanout -- --test-threads=1 --nocapture",
        "graphzero_c5yy": "pointer only; not closed by this hub bead"
    })
}
