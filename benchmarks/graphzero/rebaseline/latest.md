# GraphZero Northstar re-baseline

- Date: 2026-08-07T02:50:55.736413+00:00
- Corpus: graphzero (365 Rust files)
- Build profile: release-perf
- Binary: ${CARGO_TARGET_DIR}/release-perf/graphzero (35836aff9c7cad6588f1338cff001e5811fee06e7b744e528bc516274a11631f)
- Host class: macos-rch-test-runner
- Isolation: dedicated targeted RCH test process; no concurrent GraphZero benchmark launched by this session
- Warm reindex p50/p95: 57.959 ms / 63.16 ms (20 runs)
- Orient symbol p50/p95: 24.172 ms / 24.532 ms (20 runs, symbol run_index)
- Blast p50/p95: 19.587 ms / 24.918 ms (20 runs)
- Per-save incremental update: not_available (per-save incremental update API has not landed in GraphZero yet)
- Report class: live measurement; freshness binds run.py, stats.py, methodology, and the Rust corpus digest.
