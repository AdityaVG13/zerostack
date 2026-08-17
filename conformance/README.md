# Conformance

Hub contracts for ZeroStack and the three engines. This folder is not a test harness.

| | |
| --- | --- |
| [CONTRACT.md](CONTRACT.md) | Normative product contract |
| [engine-topology.json](engine-topology.json) | Repos, crates, dependency direction |
| [contracts/](contracts/) | Live crate contracts |
| [schemas/](schemas/) | Live wire schemas |

Build proof: `cargo build --workspace`.
Behavioral proof: a live `zsx exec` or `zsx mcp` session.
Honesty gate: `python3 scripts/check_feature_universe_weights.py`.
