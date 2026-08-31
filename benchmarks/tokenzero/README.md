# TokenZero benchmarks

This directory contains the keep-gate logic for typed TokenZero benchmark artifacts.
Benchmark execution is crate-owned. No alternate per-domain runner is supported.

The live benchmark target is:

- `cargo bench -p tokenzero-core --bench hotpaths --profile release-perf`

Run Cargo through RCH with one stable `CARGO_TARGET_DIR`. Published results must name
the exact command, source revision, profile, host class, corpus, tokenizer identity,
sample vectors, and every dropped sample.

`keep_gate.py` validates retained benchmark artifacts. `test_keep_gate.py` tests that
validator. Neither file proves the removed CLI surfaces.
