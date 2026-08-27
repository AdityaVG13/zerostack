SHELL := /bin/bash

.PHONY: test readme-command-audit host-path-audit rust-test rust-verify rust-verify-report rust-release-build rust-codemode-build mcp-compat-build rust-proof package-check release-check irx9-gate cli-smoke doctor mcp-smoke mcp-soak shell-matrix install-smoke package-audit scripts-test perf-never-worse-gate linux-docker-verify linux-perf-budget

MCP_COMPAT_TARGET := $(if $(CARGO_TARGET_DIR),$(CARGO_TARGET_DIR)/mcp-compat,target/mcp-compat)
MCP_COMPAT_BIN := $(MCP_COMPAT_TARGET)/debug/tokenzero$(if $(filter Windows_NT,$(OS)),.exe,)

test: readme-command-audit host-path-audit scripts-test rust-test

# Unit tests for the scripts/ helpers themselves (stdlib unittest, no pytest).
scripts-test:
	@python3 -m unittest discover -s scripts -p 'test_*.py' -q

# Same-surface estimated-token gate. Uses an already-built canonical CLI.
perf-never-worse-gate:
	@test -x "$${TOKENZERO_BIN:-target/release/tokenzero}" || { echo "TOKENZERO_BIN must name an existing release binary" >&2; exit 2; }
	@TOKENZERO_BIN="$${TOKENZERO_BIN:-target/release/tokenzero}" benchmarks/competitor-bakeoff.sh
	@TOKENZERO_BIN="$${TOKENZERO_BIN:-target/release/tokenzero}" benchmarks/million-line-nav.sh

# Dockerized Linux verification for macOS/Windows contributors.
linux-docker-verify:
	@scripts/rust_linux_docker_verify.sh

# Linux perf budget check (runs inside Docker).
linux-perf-budget:
	@scripts/rust_linux_perf_budget.sh

host-path-audit:
	@python3 scripts/check_no_host_paths.py

readme-command-audit:
	@python3 scripts/readme_command_audit.py
	@python3 scripts/readme_command_audit.py --self-check

rust-test:
	@cargo test --workspace

rust-verify:
	@scripts/rust_verify.sh

rust-verify-report:
	@scripts/rust_verify.sh --robot --output-json results/current/rust_verify.json

# Canonical planner-free raw worker. Legacy CodeMode stays opt-in and is not
# part of the backend artifact.
rust-codemode-build:
	@cargo build --release -p tokenzero-worker --bin tokenzero-codemode --no-default-features
	@test -x target/release/tokenzero-codemode || { echo "irx9: missing target/release/tokenzero-codemode"; exit 1; }
	@if [ "$$(uname -s)" = "Darwin" ]; then 		file target/release/tokenzero-codemode | grep -q "$$(uname -m)" 		|| { echo "irx9: tokenzero-codemode is not native: $$(file target/release/tokenzero-codemode)"; exit 1; }; 	fi

rust-release-build:
	@cargo build --release -p tokenzero-cli --bin tokenzero --no-default-features
	@$(MAKE) --no-print-directory rust-codemode-build

mcp-compat-build:
	@cargo build -p tokenzero-cli --bin tokenzero --no-default-features --features surface-mcp --target-dir "$(MCP_COMPAT_TARGET)"

rust-proof: rust-verify rust-release-build mcp-smoke mcp-soak shell-matrix install-smoke package-audit

package-check: rust-release-build package-audit

# irx9-gate is mandatory: release-check cannot claim green without it.
# Note: rust-proof may run broad verify; irx9-gate is the named-package irx9 path.
release-check: irx9-gate rust-proof

# Focused irx9 parity/packaging/dispatcher/bench gates (no workspace-wide cargo).
irx9-gate:
	@scripts/irx9_release_gate.sh

cli-smoke:
	@target/debug/tokenzero read README.md --json >/dev/null
	@target/debug/tokenzero grep TokenZero README.md docs crates --json >/dev/null
	@target/debug/tokenzero glob 'crates/**/*.rs' . --json >/dev/null
	@target/debug/tokenzero run --json -- echo ok >/dev/null

doctor:
	@target/debug/tokenzero doctor --json

mcp-smoke: mcp-compat-build
	@"$(MCP_COMPAT_BIN)" mcp-smoke --output-md results/current/rust_mcp_smoke.md --output-json results/current/rust_mcp_smoke.json --json

mcp-soak: mcp-compat-build
	@"$(MCP_COMPAT_BIN)" mcp-soak --output-md results/current/rust_mcp_soak.md --output-json results/current/rust_mcp_soak.json --json

shell-matrix:
	@target/debug/tokenzero shell-matrix --output-md results/current/rust_shell_matrix_local.md --output-json results/current/rust_shell_matrix_local.json --json

install-smoke:
	@target/debug/tokenzero install-smoke --output-json results/current/rust_install_smoke.json --json

package-audit:
	@target/release/tokenzero package-audit --dist target/release --json
