# Benchmarks

Catalog and measured numbers: [benchmarks.md](benchmarks.md).

The v1 savings run (Exact tokens, envelope bytes, call fusion) is [savings-bench-v1.md](savings-bench-v1.md).

The pass-over-pass ratchet seeds from that v1 file into [`.bench-history/savings-bench.latest.json`](../.bench-history/savings-bench.latest.json). Check a new run with `python3 scripts/check_bench_history.py --current <report.json>`. Do not replace the v1 citation set; extend it.
