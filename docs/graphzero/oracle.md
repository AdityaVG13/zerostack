# Correctness oracle — as-built design (EPIC graphzero-consolidated-oracle-gold-coverage-b19t)

One coherent measurement system answers "how accurate is the graph": a
versioned gold corpus, one scorer, explicit gate policy, and reproducible
published artifacts. This annex names the pieces so accuracy claims trace to
executable evidence instead of scattered per-file assertions.

## Components (affected crates/files)

| Piece | Location |
|---|---|
| Gold corpus | `benchmarks/gold/edges.jsonl` (13 rows: 9 true edges, 4 confirmed non-edges; calls/imports/implements; rust+typescript; 4 fixtures across 3 corpora) |
| Corpus fixtures | `benchmarks/gold/corpora/{graphzero,foreign-rust-thiserror,foreign-ts-zod}/` |
| Row schema | `benchmarks/gold/schema.json` |
| Methodology + integrity rules | `benchmarks/gold/METHODOLOGY.md` |
| Row/schema gate | `tests/cli/gold_edge_validation.rs` (`cargo test -p graphzero-cli --test cli_gold_edge_validation`) — schema + METHODOLOGY invariants on `edges.jsonl`; not a live FP/FN scorer |
| Published report | `benchmarks/gold/edge_accuracy_report.json` — committed historical measurement (`generated_by` still names the retired `gold_edge_accuracy_metrics` scorer). Do not hand-edit rates; a replacement scorer is a new bead, not this file |
| Freshness binding | report embeds SHA-256 of methodology, rows, schema, and the scorer that produced it; `gold_edge_validation` does not re-score FP/FN |
| Metamorphic invariants | `crates/graphzero-store/tests/metamorphic_contract.rs` |
| Head-to-head impact bake-off | `benchmarks/impact_bakeoff/` (pins gold digests) |
| Prevented-read bake-off | `benchmarks/prevented_read_bakeoff/` (three arms, published losses) |

## Gate policy

- False positives: `fp_rate_bps <= 100` (1%) on both arms — ratcheted from
  the original 5%; current measurement is 0 FP on both.
- False negatives: fused arm must recover every committed true edge
  (`false_negatives == 0`) and never regress against the structural baseline.
- Corpus diversity floor (ratchet-only): >=13 rows, >=3 corpora,
  >=2 languages, >=4 fixtures, >=2 relations, >=3 edge kinds, >=9 true
  edges, >=4 confirmed non-edges.
- Claim scope stays labeled `contract-only: static fixture adapter-contract
  gold set; not a production precision claim` until a broader corpus lands.

## Non-goals

- Production precision claims from this static-fixture set (scope label above).
- LSP/tsserver runtime integration in the scorer: the fused arm consumes
  committed resolution spans, by design, so the measurement is deterministic.
- Benchmark-only special cases in product code (METHODOLOGY integrity rules).

## Known gaps (tracked as narrow beads)

- Implements edges are attributed to the file node, not the implementing
  type (`record_trait_impl_edges` in `crates/graphzero-extract/src/engine.rs`);
  the gold row `gz-rust-direvidence-implements-evidencestore` records this
  contract truthfully.
- `SurfaceMatrix.toml` (FeatureUniverse -> tests/docs/oracle evidence
  traceability) does not exist yet.

## Five-mode failure evidence

GraphZero's oracle adapter has five modes: `gold`, `differential`, `metamorphic`,
`property`, and `mutation`. The mode is a stable serialized label. The adapter
emits failure evidence only; a passing release-gate report cannot be represented
as a success receipt.

`FailureBundle` is the GraphZero-owned failure evidence shape. It embeds the
hub `zero_abi::EngineIdentity` and requires `graphzero`, schema version `1`, the
lowercase 64-hex operation contract digest, semantic contract version, a
nonempty corpus identity, a full lowercase 40-hex source revision, at least one
lossless `GateFailure`, and sorted unique evidence references. Unknown fields,
malformed identity fields, digest mismatches, and empty failure/evidence sets
fail closed.

Bundles are constructed from an existing `ReleaseGateReport`, preserving every
`GateFailure` field. Failure ordering is deterministic and duplicate failures
are retained. Canonical bytes use GraphZero's existing deterministic-facts
JSON canonicalizer. This contract records oracle evidence mechanics only; it
does not expand or certify any gold-corpus claim.

The six executable verifiers for this contract are tracked in the `docs/spec-tags.md` ledger and checked with:

```bash
uv run python scripts/check_spec_tags.py
```
