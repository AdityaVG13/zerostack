# Install and build GraphZero

GraphZero currently builds from source. The repository pins the expected toolchain.

## Build the standalone CLI

```bash
git clone https://github.com/AdityaVG13/GraphZero.git
cd GraphZero
cargo build --release -p graphzero-cli --bin graphzero
./target/release/graphzero --help
```

The CLI supports graph diagnostics, indexing, orientation, blast-radius inspection, claims, and store maintenance. It is not a second model-facing planner.

## Use GraphZero through ZeroKernel

```bash
mkdir zerostack-workspace
cd zerostack-workspace
git clone https://github.com/AdityaVG13/ZeroStack.git
git clone https://github.com/AdityaVG13/FSZero.git
git clone https://github.com/AdityaVG13/GraphZero.git
git clone https://github.com/AdityaVG13/TokenZero.git
cd ZeroStack
cargo build -p zero-kernel
```

ZeroKernel loads `graphzero-kernel` as the structure authority behind `z.find`. Do not register removed GraphZero MCP or engine-local CodeMode catalogs beside it.

## Store isolation

GraphZero defaults to a repository-local graph store. Shared family storage is explicit and project-scoped. A ref proves identity only when the process can reach and digest-verify the stored object.

## Focused verification

```bash
cargo metadata --no-deps --format-version 1
cargo test -p graphzero-cli --test cli_claim_verify_cli
python3 scripts/readme_command_audit.py
```

Repository-level quality and release gates run through DSR and RCH. GitHub workflows are retained only as manual cross-platform specifications.
