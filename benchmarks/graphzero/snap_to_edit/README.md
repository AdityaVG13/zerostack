# graphzero-tcx3 snap-to-edit acceptance evidence

Date: 2026-07-24

## Scope

A Snapshot-owned immutable index resolves exact symbols, qualified path/symbol targets, and short
natural-language intents to edit-ready anchors. Index construction materializes paths, one-based lines,
identifier byte spans, definition kinds, enclosing block spans, and exact gz evidence refs. After the
OnceLock is warm, resolve performs no repository traversal or filesystem I/O.

The anchor field names (`path`, `line`, `byte_span`) deliberately match FSZero
snapshot vocabulary. `z.find` returns the structural target and evidence ref;
`z.read` binds exact file bytes before a guarded edit.

## Committed corpus

benchmarks/gold/snap-to-edit/queries.tsv contains 50 non-cherry-picked, globally unique definitions from
the GraphZero self-repo: 20 exact names, 15 qualified path/name queries, and 15 intent queries. The
ignored acceptance test rebuilds a fresh self-repo index before timing. The current repository contains
344 Rust source files; this is larger than the bead's historical 289-file target.

## Results

- Spark targeted unit contracts: 3/3 passed (exact/qualified/intent, ambiguity/alternates, warm latency).
- Spark indexed fixture contract: 1/1 passed (line/span/kind/path/evidence shape, 1,000 warm resolves).
- Spark 50-query self-repo bakeoff: top-1 50/50 (100.0%), p50 33.568 µs, p95 1.880412 ms, losses: none.
- Actual ripgrep 15.1.0 baseline on the same committed corpus (Mac, explicit real binary
  /opt/homebrew/bin/rg --type rust -l PATTERN .): top-1 50/50 (100.0%), p50 13.142 ms,
  p95 13.978 ms, losses: none.

The ripgrep number is reported on a different host because the Spark worker does not install rg. The
committed test attempts rg when available and reports unavailability honestly; no cross-host latency
speedup claim is made.

## Reproduction

    rch exec -- cargo test -p graphzero-store 'snap_edit::tests' --lib
    rch exec -- cargo test -p graphzero-store snap_to_edit
    rch exec -- cargo test -p graphzero-store --test snap_to_edit_gold -- --ignored --nocapture

No full local cargo test suite was run.
