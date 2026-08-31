# Development

Build, test, and verify the TokenZero Rust core from source. Source development happens in this monorepo.

TokenZero requires Rust 1.98 nightly or newer. `rust-toolchain.toml` pins
`nightly-2026-05-31` so local and CI builds use the verified minimum toolchain.

## Build

```bash
git clone https://github.com/AdityaVG13/zerostack
cd zerostack
cargo build --release -p zero-kernel

./target/release/zero-kernel doctor
./target/release/zero-kernel exec
```

TokenZero is domain logic plus the ZeroKernel adapter. There is no `tokenzero` product CLI.

## Verify

Use a named package and exact `--lib`, `--bin`, or `--test` target. Do not treat `cargo test --workspace` as the project gate.

```bash
cargo test -p zero-token --lib
cargo fmt --all -- --check
```

Format changed Rust files with `rustfmt --edition 2024 -- <file.rs>`.

## Workspace

The internal domain library lives under `crates/tokenzero/`; its ZeroKernel adapter lives under `crates/zerostack/`.

| Crate | Responsibility |
| --- | --- |
| `tokenzero-core` | Tokenizer identity, compression, model artifacts, and exact-recovery decisions |
| `zero-token` | Typed `TokenEngine` implementation consumed by ZeroKernel |

Hub Pulse telemetry lives in `crates/zerostack/zero-pulse`. There is no `tokenzero pulse` command.

## Verification artifacts

Local TokenZero proof artifacts are not ZeroStack release channels.

## Release boundaries

Pre-launch: do not upload packages, mutate global config, publish remotes,
rewrite history, or perform a public release without explicit approval. See
[`SECURITY.md`](../../SECURITY.md) and [`CONTRIBUTING.md`](../../CONTRIBUTING.md).
