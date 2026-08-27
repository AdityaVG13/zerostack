# Edge accuracy gold set methodology

This directory is the committed measuring stick for GraphZero edge precision. Each `edges.jsonl` row names a fixture, an anchor string, the expected structural-only behavior, and the expected typed/LSP behavior. Rows include positive edges and confirmed non-edges so precision work measures false positives and false negatives.

Minimum diversity gate: the executable metrics test records total rows, corpora, languages, fixtures, relations, edge kinds, case labels, true edges, and confirmed non-edges. Until a broader corpus lands, the published report must label these numbers as a contract-only static-fixture adapter-contract gold set, not a production precision claim.

Integrity rules:
- Fixtures are small excerpts, not generated benchmark-only code paths in product code.
- Every row has an anchor string that must appear in its fixture.
- Structural and typed expectations are recorded before tuning the fusion layer against the set.
- Future measurements must report losses; do not drop rows to improve rates.

Current committed measurement lives in `edge_accuracy_report.json` (historical `generated_by` still names the retired `gold_edge_accuracy_metrics` scorer). Live enforcement is schema/row invariants in `tests/cli/gold_edge_validation.rs`, not a re-run of that scorer. The report records 70 rows across 10 fixtures and 5 corpora covering calls, imports, and implements with 18 confirmed non-edges. Case coverage spans higher-order arguments, fn-pointer arguments, trait dispatch, default trait-method bodies, generic-bound dispatch, associated functions, interface method dispatch, receiver type resolution, re-exports, type-only imports, and cross-module references. Structural-only extraction reports 0 FP / 26 FN (50.00% FN); the adapter-contract fused graph reports 0 FP / 2 FN (3.84% FN). The false-positive gate in the published report is 100 bps (1%) on both arms and the fused FN gate is 1000 bps (10%).

Published losses (rows the fused arm still misses, so the report never overstates the win):
- `gz-td-expand-dispatches-to-impl-seed` — a default trait-method body reaching a concrete impl method. The gold resolution names `BfsExpander::seed`, but both the trait method and the impl method are extracted under the local name `seed`, so the fused edge lands on whichever `seed` node the adapter matches first. Distinguishing them needs impl-qualified symbol names in the extractor, not a better resolver.
- `foreign-ts-reg-reexport-checks` — an `export { X } from "./mod.js"` re-export. The typed arm records an import edge, but the structural pass never produced a path node for the re-exported module because the tree-sitter query only matches `import_statement`, so there is no structural edge at that span to supersede.

## Live LSP arm

`crates/graphzero-extract/src/rust_analyzer_lsp.rs` is the concrete `TypedResolver` behind the typed-fusion install point: it spawns a language server, opens the blob, and issues `textDocument/definition` per referenced identifier. `cargo test -p graphzero-extract --test live_rust_analyzer_fusion` proves the live path against a real stub language-server subprocess (always runs) and against the real rust-analyzer binary where it is installed. `GRAPHZERO_REQUIRE_LIVE_LSP=1` makes a missing rust-analyzer a failure instead of a skip. The published `edge_accuracy_report.json` numbers remain adapter-contract numbers: they are scored from the committed gold resolution spans, not from a live server sweep over all 10 fixtures.

## Freshness manifest

edge_accuracy_report.json is an adapter-contract/static-fixture measurement. Its freshness block records the SHA-256 of this methodology file, edges.jsonl, schema.json, and the Rust scorer test that generates the published metrics. The cargo gate recomputes those digests and the metrics, so row, schema, methodology, or scorer changes fail until the report is regenerated.
