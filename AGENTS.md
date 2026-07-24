# ZeroStack documentation hub

This repository is the canonical public aggregation point for ZeroStack. Keep engine implementation code in the TokenZero, FSZero, and GraphZero repositories.

## Scope

- Public documentation lives in `README.md` and `docs/`.
- Reproducible measurements live in `benchmarks/`.
- Shared contracts and checks live in `conformance/`.
- Shared foundation crates live in `crates/`.
- Do not publish private engine source or private package source here.

## Documentation rules

- Describe both supported deployment modes: standard MCP adapter or CodeMode.
- State that deployments choose exactly one mode and never run both simultaneously.
- Mark TokenZero public; mark FSZero and GraphZero private and in development until their status changes.
- Use repository-relative examples. Do not add machine-specific absolute paths, credentials, personal data, or private URLs.
- Treat benchmark claims as evidence-backed only when their artifacts are committed.

## Validation

Run documentation privacy checks, then the conformance suite when its Rust toolchain is available:

~~~sh
rg -n '/Users/|/home/|BEGIN .*PRIVATE KEY|api[_-]?key|password' README.md docs AGENTS.md
cargo test --manifest-path conformance/Cargo.toml
~~~
