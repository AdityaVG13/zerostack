# Development

Build, test, and verify the TokenZero Rust core from source. Source development happens in this monorepo.

TokenZero requires Rust 1.98 nightly or newer. `rust-toolchain.toml` pins
`nightly-2026-05-31` so local and CI builds use the verified minimum toolchain.

## Build

```bash
git clone https://github.com/AdityaVG13/zerostack
cd zerostack
cargo build --release -p tokenzero-cli --bin tokenzero

./target/release/tokenzero doctor --json
./target/release/tokenzero read README.md --json
./target/release/tokenzero find "TokenZero" docs --json
./target/release/tokenzero tree . --depth 2 --json
./target/release/tokenzero run -- cargo test -p tokenzero-kernel --lib
./target/release/tokenzero expand tz://blob/<id> --selector raw
```

## Verify

Use a named package and exact `--lib`, `--bin`, or `--test` target. Do not treat `cargo test --workspace` as the project gate.

```bash
cargo test -p tokenzero-kernel --lib
cargo fmt --all -- --check
```

Format changed Rust files with `rustfmt --edition 2024 -- <file.rs>`. There is no `scripts/rustfmt_targeted.sh` helper in this repository.

## Workspace

Ten Cargo packages under `crates/tokenzero/`:

| Crate | Responsibility |
| --- | --- |
| `tokenzero-core` | Compression model and content-addressed exact-recovery refs |
| `tokenzero-recovery` | Bounded recovery cache with exact byte-recovery for refs |
| `tokenzero-runtime` | Runtime and session orchestration for the context layer |
| `tokenzero-filters` | Content filters and selectors for compression |
| `tokenzero-cli` | Standalone CLI and classic MCP compatibility entrypoint |
| `tokenzero-engine` | Typed TokenEngine adapter consumed by ZeroKernel |
| `tokenzero-kernel` | Kernel-facing measurement and projection |
| `tokenzero-install` | Installer and agent-wiring |
| `tokenzero-pulse` | Pulse telemetry |
| `tokenzero-test-support` | Shared test helpers |

The `tokenzero` CLI binary is produced by `tokenzero-cli`.

## Verification artifacts

Local TokenZero proof artifacts are not a ZeroStack release channel. Do not treat `results/current/` listings, MCP smoke JSON, or platform uploads as a tagged release, Homebrew bottle, or npm publication.

## Release boundaries

Pre-launch: do not upload packages, mutate global config, publish remotes,
rewrite history, or perform a public release without explicit approval. See
[`SECURITY.md`](../../SECURITY.md) and [`CONTRIBUTING.md`](../../CONTRIBUTING.md).
