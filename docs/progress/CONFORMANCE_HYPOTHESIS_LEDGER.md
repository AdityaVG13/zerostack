# Conformance hypothesis ledger

Index into [GAUNTLET_EXPERIMENT_DESIGNS.md](GAUNTLET_EXPERIMENT_DESIGNS.md). Full template fields live there. Grep the negative ledger first: [conformance-negative-results.md](conformance-negative-results.md).

**Open-hypothesis count (this pillar):** 13 (`CONF-0001..0005`, `IDEA-0002/0010/0012/0013/0015/0024`, `ADV-0002`, `ADV-0003`).
**Confirmed gaps owed to pass 11:** 3 (`CONF-0006` hash pin; `SURF-0010` cancel test is surface-indexed but conformance-shaped; plus idea children). SPEC tags stay `OPEN` until a live verifier runs.

| ID | Status | One-line hypothesis | Invocation |
|---|---|---|---|
| CLOSED-0001 | CLOSED | EngineIdentity Subject≠Oracle (`7992967`). | `cargo test -p zerostack-harness --lib engine_identity` |
| CLOSED-0002 | CLOSED | Both-error is agreement. | `cargo test -p zerostack-harness --lib oracle` |
| CLOSED-0003 | CLOSED | FailureBundle writes `/failure/first_divergence` (`9081c2c`). | `cargo test -p zerostack-harness --lib failure_bundle` |
| CLOSED-0004 | CLOSED | Crash oracle 5/5 green (`9081c2c`). | `cargo test -p zerostack-harness --test crash_oracle` |
| CLOSED-0005 | CLOSED | `zero-ref` parse/Display proptest present. | `cargo test -p zero-ref --test zeroref_proptest` |
| CLOSED-0008 | CLOSED | Three-tier goldens + integrity (`738a803`). | `python3 scripts/check_golden_integrity.py` |
| CONF-0001 | OPEN | `[SPEC-COMP-002]` live multi-mutation rollback. | `rg SPEC-COMP-002 docs/spec/SPEC-TAGS.md` |
| CONF-0002 | OPEN | `[SPEC-HON-002]` production spill receipt. | `rg result_finalization_receipt crates` |
| CONF-0003 | OPEN | `[SPEC-HON-006]` hub accepts TokenZero Exact receipt. | `rg SPEC-HON-006 docs/spec/SPEC-TAGS.md` |
| CONF-0004 | OPEN | `[SPEC-HUB-002]` fail-loud suite on live receipts. | `rg SPEC-HUB-002 docs/spec/SPEC-TAGS.md` |
| CONF-0005 | OPEN | `[SPEC-HUB-005]` digest bump mutation test. | `rg SPEC-HUB-005 docs/spec/SPEC-TAGS.md` |
| CONF-0006 | CONFIRMED_GAP | AGENTS.md sha256 drifted; preflight red. | `cargo run -p zerostack-harness --bin oracle-preflight-doctor -- --json` |
| IDEA-0002 | OPEN | Pin tracked `AGENTS-MANDATE.md` instead of gitignored law. | `shasum -a 256 docs/progress/AGENTS-MANDATE.md AGENTS.md` |
| IDEA-0010 | OPEN | Smoke test allows certifying=false. | `rg fn preflight crates/zerostack-harness/tests/oracle_smoke.rs` |
| IDEA-0012 | OPEN | Empty Q99 window is `unavailable`, never a number. | `rg unavailable crates/zsx-core/src/residency.rs` |
| IDEA-0013 | OPEN | `ZeroRefErrorClass::ALL` reachability table. | `rg ZeroRefErrorClass::ALL crates/zero-ref` |
| IDEA-0015 | OPEN | MCP late-Ok `commit_race` test without `fszero.rs`. | `rg commit_race crates/zero-mcp/src/mcp_transport.rs` |
| IDEA-0024 | OPEN | Oversize `zsx exec` nulls `visibleTokenCount`. | `rg result_finalization_receipt crates` |
| ADV-0002 | OPEN | e-process on spec-hash is redundant with preflight. | `cargo test -p zerostack-harness --lib eprocess` |
| ADV-0003 | OPEN | Inflight count returns to 0 after cancel. | `rg inflight crates/zero-mcp/src/mcp_transport.rs` |

## Five UNVERIFIED SPEC tags (do not mark CONFIRMED_GAP)

| Tag | Why unverified | Card |
|---|---|---|
| COMP-002 | journal+undo types; no live later-failed-step execute | CONF-0001 |
| HON-002 | hub crates do not emit `result_finalization_receipt.v1` (fixtures only) | CONF-0002 |
| HON-006 | permission, TokenZero-owned Exact | CONF-0003 |
| HUB-002 | global fail-loud; no single static surface | CONF-0004 |
| HUB-005 | digest bump needs a mutation test | CONF-0005 |

## Already correct (do not re-open)

- 48/53 verifiable tags wired (`7992967`)
- `invent-second-conformance-catalog` REJECTED (CONTRACT §8)
- `commit-race-mislabel` REJECTED
- `engine-import-cycle` REJECTED
- `host-path-leak` REJECTED
- `both-error-as-failure` REJECTED

## Pass-11 note

Smallest conformance close: CONF-0006 (re-pin agents-law hash + bless golden). Do not invent a conformance CLI. Do not emit Exact from the hub. Do not edit rival-dirty `fszero.rs`.
