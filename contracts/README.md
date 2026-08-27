# Contracts

Machine-readable contracts that define the stable surface between the hub, engines, and conformance.

All files in this directory are normative inputs. Checked-in fixtures and digests are required for deterministic tests and release gates.

## Files

| File | Purpose |
| --- | --- |
| `filesystem-v1.json` | Canonical FSZero filesystem contract version 1. Typed operations, error classes, golden vectors, and aliases |
| `filesystem-v1.coverage.json` | Coverage map for the filesystem contract |
| `ncib-conformance-corpus-v1.json` | FSZero NCIB conformance corpus |
| `operation-abi-schemas-v1.json` | Canonical input and output JSON Schemas for FSZero domain operations, MCP tools, CodeMode tools, and CodeMode methods |
| `zeroref-v1-fixtures.json` | ZeroRef v1 golden vectors for blob and fragment parsing and expansion |
| `zeroref-capability-fixtures.json` | ZeroRef capability peer descriptors for negotiation tests |
| `approved_operation_abi_digest.txt` | Pinned approved operation ABI digest for GraphZero release gates |
| `SurfaceMatrix.toml` | Declared GraphZero operation surface matrix and documentation anchors |
| `digest_break_approval.json` | Explicit break classification when the approved digest intentionally changes |

## Usage

FSZero loads `filesystem-v1.json` and `operation-abi-schemas-v1.json` at compile time via `include_str!` and exposes the parsed value to all surfaces. GraphZero loads `approved_operation_abi_digest.txt` and `SurfaceMatrix.toml` for parity and release gates. ZeroRef fixtures are shared across FSZero, GraphZero, and TokenZero.

Do not edit these files without updating the related Rust constants, tests, and the `crates/zerostack/zerostack-conformance` suite. Contract changes require a digest update when the operation ABI changes.

## Change process

See `CONTRIBUTING.md` and `crates/zerostack/zerostack-conformance/CONTRACT.md` for the change process and invariants.
