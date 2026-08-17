# Conformance

Hub contracts for ZeroStack and the three engines. This folder is not a test harness.
There is no in-repo conformance CLI (`CONTRACT.md` §8).

Gauntlet public-release scorecard (Phase 16, **not certified**):

- [`../docs/progress/FINAL_GAUNTLET_REPORT.md`](../docs/progress/FINAL_GAUNTLET_REPORT.md)
- [`../docs/progress/PARITY_RUNBOOK.md`](../docs/progress/PARITY_RUNBOOK.md)
- [`contracts/feature_coverage_dashboard.json`](contracts/feature_coverage_dashboard.json) -- 77 rows; present=67 partial=7 missing=0 excluded=3; effective `0.952282`; strict `0.940573`; **gate=red**

| | |
| --- | --- |
| [CONTRACT.md](CONTRACT.md) | Normative product contract |
| [engine-topology-v1.json](engine-topology-v1.json) | Repos, crates, dependency direction |
| [contracts/](contracts/) | Narrow crate contracts (FeatureUniverse, dashboard, spec hashes) |
| [schemas/](schemas/) | JSON Schema for wire types |
| [golden/](golden/) | Three-tier golden artifacts (Tier1Raw / Tier2Canonical / Tier3Logical) |
| [models/](models/) | Example instances |
| [authority/](authority/) | Claim ledger (unproven until measured) |

Build proof: `cargo build --workspace`.
Behavioral proof: a live `zsx exec` or `zsx mcp` session with receipts.
Measured token numbers: [`../benchmarks/`](../benchmarks/benchmarks.md).
Honesty gates: `python3 scripts/check_feature_universe_weights.py`, `check_feature_coverage_dashboard.py`, `check_golden_integrity.py`. Bless goldens only with `scripts/bless-golden.sh --i-am-the-operator`.
