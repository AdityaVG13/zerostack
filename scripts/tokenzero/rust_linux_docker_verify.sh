#!/usr/bin/env bash
set -euo pipefail

export PATH="/usr/local/cargo/bin:${PATH}"; export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/linux-docker}"

cargo test --workspace
cargo build -p tokenzero-cli --bin tokenzero --no-default-features
cargo build -p tokenzero-worker --bin tokenzero-codemode --no-default-features
cargo run -p tokenzero-cli --bin tokenzero --no-default-features -- shell-matrix \
  --output-json results/current/rust_shell_matrix_linux_docker.json \
  --output-md results/current/rust_shell_matrix_linux_docker.md \
  --json
cargo run -p tokenzero-cli --bin tokenzero --no-default-features -- package-audit \
  --dist "${CARGO_TARGET_DIR}/debug" \
  --json > results/current/rust_package_audit_linux_docker.json
