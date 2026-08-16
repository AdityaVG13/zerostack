# Conformance

Hub contracts for ZeroStack and the three engines. This folder is not a test harness.

| | |
| --- | --- |
| [CONTRACT.md](CONTRACT.md) | Normative product contract |
| [engine-topology-v1.json](engine-topology-v1.json) | Repos, crates, dependency direction |
| [contracts/](contracts/) | Narrow crate contracts |
| [schemas/](schemas/) | JSON Schema for wire types |
| [models/](models/) | Example instances |
| [authority/](authority/) | Claim ledger (unproven until measured) |

Build proof: `cargo build --workspace`.
Behavioral proof: a live `zsx exec` or `zsx mcp` session with receipts.
Measured token numbers: [`../benchmarks/`](../benchmarks/benchmarks.md).
