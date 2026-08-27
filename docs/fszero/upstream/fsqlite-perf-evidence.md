# fsqlite performance evidence (fszero-1zi / fszero-kfl / fszero-fi2)

Consolidated, reproducible evidence for an upstream fsqlite report. All
numbers measured on Apple M5 Max / 18 cores, fsqlite 0.1.11, FSZero commits
noted inline. Prepared for filing upstream -- NOT yet filed (external
communication needs owner approval).

## 1. Btree insert path dominates bulk inserts (fszero-1zi)

`sample`d during a 10k-file cold index at FSZero 8adcab5 (before the AST
sidecar migration), ~200k small-row INSERTs inside one explicit txn using
`execute_with_params_skip_statement_savepoint_in_explicit_txn`:

- 1241/2226 samples inside `insert_ast_symbol_node` -->
  `execute_precompiled_prepared_insert_fast` --> `BtreeCursorOps::index_insert`
- 679/1063 engine samples inside `balance_for_insert` --> `balance_nonroot`,
  with heavy `Vec<GatheredCell>` alloc/drop churn per insert.
- Net effect: `ast_persist` phase = 8.3s of a 12.8s build (65%); fsqlite's
  own CREATE INDEX backfill of the same rows costs another ~5.5s.
- Control: migrating the identical rows/statements to rusqlite (bundled
  sqlite) in one txn: 0.5-0.7s -- a 12-16x gap on the same workload
  (FSZero 6f0aa20, benchmarks/index-scaling.md).

Original fszero-1zi findings (FSZero 174c109, samply, GTNH 208MB corpus)
remain relevant: `drop_elements<PageNumber>` 19% (per-statement savepoint
table, O(n^2) in big txns -- the skip API works around it) and
`CellSlotCache::insert_slow` 32-34% for >=4KB cells in any insert order.

Repro: `git checkout 8adcab5 && cargo build --release && python3
benchmarks/gen_corpus.py --files 10000 --out /tmp/c10k --seed 42 &&
FSZERO_ROOT=/tmp/c10k FSZERO_STARTUP_INDEX=1 FSZERO_INDEX_PHASES=1
FSZERO_INDEX_MAX_FILES=11000 target/release/fszero codemode 'return{ok:true}'
--root /tmp/c10k` then `sample <pid>` during the ast_persist phase.

## 2. Historical CLI store-open delta (observational, fszero-kfl adjacent)

`benchmarks/cli_store_open.py` reproduces this observational CLI scenario;
see its contract in `benchmarks/store-open.md`. The historical measured
artifact `benchmarks/store-open.json` (runner lineage c88344a, artifact commit
8822dfab) recorded a trivial CLI call at 13.5ms against an 8.8MB repository
store versus 8.1ms against a fresh store -- a ~5.4ms root-to-root delta,
consistent with fsqlite loading/reloading its memdb at `Connection::open`.
`benchmarks/store_open.py` now names a different durable large-store reopen
gate. A lazy pager would eliminate the hypothesized size-dependent component.

## 3. Cross-process visibility / contention (fszero-fi2)

`benchmarks/concurrent_spawn.py` (committed): with N processes sharing one
store,
- committed-but-recent writes from process A are not visible to process B
  even through a FRESH `Connection::open` (B re-runs a full cold build the
  winner just finished; N=4 locked storm: 4/4 cold instead of 1 cold + 3
  warm). Suspects: WAL not read cross-process, or the outer batch COMMIT
  failing silently under cross-process contention.
- merely HOLDING the db open in other processes slows a serialized writer
  ~6x (14s vs 2.3s solo per cold build at 2k files).
- unlocked concurrent writers: N=8 costs 418s aggregate CPU for one usable
  index (thermal incident reproduced).

Repro: `python3 benchmarks/concurrent_spawn.py --files 2000 --fanout 4
--runs 1` and inspect cold/warm child counts.

## Suggested upstream asks

1. Cheaper btree insert path (balance frequency / GatheredCell churn) or a
   documented bulk-load mode.
2. CREATE INDEX backfill via sort+build rather than per-row insert.
3. Lazy/mmap pager so `Connection::open` cost stops scaling with db size.
4. Cross-process WAL read visibility (or a loud error if unsupported) and
   non-silent COMMIT failure under contention.
