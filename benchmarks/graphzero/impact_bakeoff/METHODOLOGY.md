# Impact-analysis bake-off methodology

This benchmark publishes current GraphZero impact-analysis losses against the committed gold set.

- `benchmarks/gold/edges.jsonl` is the fixed corpus (70 rows, 10 fixtures, 5 corpora). Rows are not filtered by outcome.
- `graphzero_structural` is the current product path: tree-sitter structural extraction without LSP enrichment. It measures 0 FP and 26/52 FN (50.00%) on the expanded corpus.
- `rust_analyzer_and_tsserver_adapter_contract` is the typed LSP-equivalent contract from the gold set. It uses committed rust-analyzer/tsserver resolution spans and measures 0 FP and 2/52 FN (3.84%). It is not reported as a live LSP subprocess measurement.
- The live LSP subprocess resolver itself ships in `crates/graphzero-extract/src/rust_analyzer_lsp.rs` and is proven end-to-end against a real language-server child process by `cargo test -p graphzero-extract --test live_rust_analyzer_fusion`. That test is a behavioural proof of the live path, not the source of the accuracy numbers here; `live_lsp_processes_invoked` therefore stays false in this report.
- `run.py` records whether live `rust-analyzer`/`tsserver` binaries are present, but it does not fabricate live adapter numbers when the live bridge did not produce them.
- Losses stay in `report.json`; a benchmark that drops losing rows is invalid. Both remaining typed-arm false negatives are named in the report and explained in `benchmarks/gold/METHODOLOGY.md`.

Run:

```bash
python3 benchmarks/impact_bakeoff/run.py
cargo test -p graphzero-cli --test impact_bakeoff_report
```

## Freshness manifest

report.json is an adapter-contract/static-fixture report, not a live LSP measurement. Its freshness block records the SHA-256 of run.py, this methodology file, the gold rows, schema, gold methodology, and the gold accuracy report consumed by the bake-off. The report gate recomputes those digests so fixture, methodology, generator, or gold-report changes fail until the bake-off report is regenerated.
