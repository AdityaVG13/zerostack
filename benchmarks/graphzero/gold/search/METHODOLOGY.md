# Search-scale gold corpus methodology (graphzero-aluu)

This directory is the committed measuring stick for GraphZero **symbol/path
substring search** latency and correctness under `GRAPHZERO_SEARCH_BIGRAM`.

It is **not**:

- the edge-accuracy excerpt set under `benchmarks/gold/corpora/` (~2.4 KB)
- the synthetic spike under `crates/graphzero-query/tests/search_bigram_spike.rs`
  / `benchmarks/search_bigram_spike_result.json`

## What "search-scale" means

Pinned floors in `manifest.json` `scale_gates`:

| Gate | Floor |
|------|------:|
| files | ≥ 200 |
| corpus bytes | ≥ 512 KiB |
| indexed symbols (after `index_repo`) | ≥ 10_000 |

Query classes in `queries.jsonl`:

- **rare** — planted unique needle; few hits
- **common** — dense substring (`parse` / `parse_alpha_` / `mod_`)
- **absent** — guaranteed zero hits

## Integrity

1. Corpus is produced by `gen_corpus.py` with the seed/params in `manifest.json`.
2. `corpus.sha256` binds the tree (sorted relative path + file bytes).
3. The bakeoff harness refuses to publish numbers if scale gates fail, SHA
   mismatches, or label sets diverge between scan and bigram paths.
4. Do not drop queries to improve rates. Publish losses.

## Regeneration

```bash
python3 benchmarks/gold/search/gen_corpus.py
python3 benchmarks/gold/search/gen_corpus.py --check
```

## Measurement surface

`benchmarks/search_bigram_bakeoff/` (Python driver) +
`crates/graphzero-query/tests/search_bigram_gold_bakeoff.rs` (release microbench).

Default-on remains **off** until a later decision bead re-opens with published
gold numbers (and densify/publish-time follow-ups if gates still fail).
