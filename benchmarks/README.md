# ZeroStack benchmark protocol

Benchmark history records live in `.bench-history/` and are reviewed artifacts. Do not ignore that directory or publish a performance claim without a measured baseline.

## zero-ref parse pass-over-pass procedure

Run only the targeted benchmark, never a workspace benchmark:

~~~sh
cargo bench -p zero-ref --bench parse --profile release-perf
~~~

1. Use one otherwise-idle host for both passes. Record its stable fingerprint, operating system, and architecture. Reject results collected on a different host.
2. Pin the same Cargo and Rust toolchain for baseline and candidate. Record their full versions and reject cross-toolchain comparisons.
3. Use adjacent baseline and candidate revisions in one revision window. The window may contain only the intended benchmarked change, with no dependency, configuration, or unrelated source drift. Record both revisions.
4. Run the command independently at least five times on the baseline revision and at least five times on the candidate revision. Do not use `--workspace` or add other packages or benches.
5. Immediately after every invocation, record each benchmark's middle value from the CLI `time: [lower point upper]` output before starting the next invocation. This is Criterion's selected typical point estimate, in nanoseconds per iteration; it is not necessarily `median.point_estimate` from `estimates.json`.

For each benchmark ID and pass, store the per-run typical point estimates in `independent_run_point_estimates_ns`, set `run_count`, then calculate:

- `median_ns`: median of the independent per-run point estimates.
- `cv_pct`: 100 times the sample standard deviation divided by their mean.
- `regression_pct`: 100 times `(candidate_median_ns - baseline_median_ns) / baseline_median_ns`.

Update `.bench-history/zero-ref-parse.latest.json` provenance and measurement fields, set each comparison status to `pass` only when both passes have `cv_pct <= 5` and `regression_pct <= 5`, then set the top-level status. Otherwise mark it `fail` or leave it `pending` and rerun the complete pass pair. Never substitute Criterion's samples within one invocation for independent runs.

The stable benchmark IDs are:

- `zero_ref_v1_parse_whole_fz`
- `zero_ref_v1_parse_byte_span_gz`
- `zero_ref_v1_parse_line_span_tz`
