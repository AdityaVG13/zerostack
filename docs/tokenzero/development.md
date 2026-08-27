# Development

Build, test, and verify the TokenZero Rust Core from source. Most users should
prefer a prebuilt binary from the [latest Release](https://github.com/AdityaVG13/tokenzero/releases);
this page is for contributors and from-source builds.

TokenZero requires Rust 1.98 nightly or newer. `rust-toolchain.toml` pins
`nightly-2026-05-31` so local and CI builds use the verified minimum toolchain.

## Build

```bash
cargo build --release -p tokenzero

target/release/tokenzero doctor --json
target/release/tokenzero read README.md --json
target/release/tokenzero find "TokenZero" docs --json
target/release/tokenzero tree . --depth 2 --json
target/release/tokenzero run -- cargo test --workspace
target/release/tokenzero expand tz://blob/<id> --selector raw
```

## Verify

The debug binary is fine for the development loop:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
target/debug/tokenzero mcp-smoke --json
```

### Targeted formatting (dirty worktrees)

Do NOT use `cargo fmt -- path/to/file.rs` as a file allowlist. `cargo fmt`
ignores trailing paths as a scope filter and may format the entire workspace
(~60 files). For single-file formatting use the repo-supported helper:

```bash
scripts/rustfmt_targeted.sh crates/tokenzero-core/src/lib.rs
scripts/rustfmt_targeted.sh --check crates/tokenzero-core/src/lib.rs
```

The helper validates explicit `.rs` files inside the repo, rejects zero
args/directories/non-`.rs`/outside-repo paths, preserves spaces, and runs
`rustfmt --edition 2024 -- <file>` once per file without invoking cargo-fmt.

## Workspace

Eight Rust crates:

| Crate | Responsibility |
| --- | --- |
| `tokenzero-core` | Compression model and content-addressed exact-recovery refs |
| `tokenzero-recovery` | Bounded recovery cache with exact byte-recovery for refs |
| `tokenzero-runtime` | Runtime and session orchestration for the context layer |
| `tokenzero-filters` | Content filters and selectors for compression |
| `tokenzero-cli` | Standalone CLI and classic MCP compatibility entrypoint |
| `tokenzero` | The `tokenzero` binary |
| `tokenzero-install` | Installer and agent-wiring (Claude/Codex/Grok/etc.) |
| `tokenzero-pulse` | Pulse telemetry and forecasting |

## Verification artifacts

Proof artifacts are written under `results/current/` when a local or CI
verification run emits them:

| Artifact | Proves |
| --- | --- |
| `rust_mcp_smoke.json` | MCP tool and alias smoke |
| `tokenzero_exact_recovery_audit.json` | CLI/MCP exact recovery checks, including degraded-cache rows |
| `tokenzero_protected_anchor_audit.json` | Protected failure/warning anchor recall |
| `tokenzero_shell_matrix.json` | Local shell/runtime matrix for this host |
| `tokenzero_os_reach_audit.json` | OS-scoped evidence and install-smoke status |
| `tokenzero_source_currency.json` | Public-claim source freshness gate |
| `tokenzero_claim_audit.json` | Release/public-claim gate summary |

Some release jobs additionally upload platform-specific artifacts outside the
repo checkout; do not infer those from a local `results/current/` listing.

## Release boundaries

Pre-launch: do not upload packages, mutate global config, publish remotes,
rewrite history, or perform a public release without explicit approval. See
[`../SECURITY.md`](../SECURITY.md) and [`../CONTRIBUTING.md`](../CONTRIBUTING.md).
