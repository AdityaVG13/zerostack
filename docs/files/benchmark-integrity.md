# Benchmark integrity policy

This policy applies to every FSZero performance or quality number published from this
repository. Numeric budgets, when they exist, live in [BUDGETS.md](BUDGETS.md).

## Required evidence

Every published result must record:

1. the exact source revision and whether the tree was dirty;
2. the exact command, package, target, features, profile, and Rust toolchain;
3. the standalone `zero-kernel` CLI or typed FSZero API exercised;
4. the corpus identity, generation procedure, size, and digest;
5. CPU, core count, memory, operating system, architecture, and host class;
6. every raw sample, warmup, failure, retry, and dropped-sample reason;
7. the statistic and estimator used for each summary; and
8. the matching baseline configuration and input digest.

Percentile claims require at least 20 successful measured samples. Warm and cold results
must be labeled separately. Setup, cache priming, and teardown boundaries must be stated.
Never replace an error payload with a timing sample. Never discard a losing case.

## Execution

Use `release-perf` for latency-sensitive Rust targets and run Cargo through RCH with one
stable `CARGO_TARGET_DIR`. A harness adapter cannot prove standalone ZeroStack behavior.
The committed runner must fail when the candidate, baseline, corpus, or receipt is absent
or malformed.

## Publication

Do not publish an absolute threshold or comparison until a live committed runner produces
a complete artifact under this policy. Method changes start a new baseline. Historical CLI,
MCP, and engine-local CodeMode artifacts are not comparable to direct ZeroKernel results.
