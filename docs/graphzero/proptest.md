# Proptest seed and replay contract

GraphZero's randomized tests use `proptest` (crates `graphzero-query`,
`graphzero-reserve`, `graphzero-store`). This page is the machine-checked
contract for how they run, how failures replay, and how regression seeds are
committed. `scripts/check_proptest_contract.py` enforces it and runs in CI.

## Random by default, exact replay on demand

- Generation stays random. No test file hard-codes a fixed proptest seed.
- Set the numeric seed to replay a run exactly:

  ```bash
  PROPTEST_RNG_SEED=123456789 rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero \
    cargo test -p graphzero-store store::query::delta_codec::tests::prop_encode_decode_symbol_identity \
    --lib -- --exact --test-threads=1
  ```

- When a property fails, proptest emits the failing seed and writes it to the
  crate's regression file. Replay that failure exactly by re-running with the
  emitted numeric `PROPTEST_RNG_SEED`.
- `PROPTEST_DISABLE_FAILURE_PERSISTENCE` is forbidden in CI and in any release
  or receipt-producing check: it silently discards the evidence that a
  property failed. The contract checker fails any proptest site or script that
  references it.

## Committed regression layout

Shrunk failures are committed under the affected crate's local layout:

- `crates/graphzero-query/proptest-regressions/`
- `crates/graphzero-reserve/proptest-regressions/`
- `crates/graphzero-store/proptest-regressions/`

Rules:

- The directory exists from day one with only a `README.md`. It stays
  README-only until a real proptest failure generates a `.txt` regression
  file. An empty layout means no failing seed is persisted; it does not prove
  that no property has ever failed.
- Never hand-edit or fabricate regression lines. They are written only by
  proptest, one seed per line, in proptest's persistence format.
- A committed regression file must mirror the source path under its crate
  (for example `crates/graphzero-store/proptest-regressions/store/query/delta_codec.txt`
  mirrors `crates/graphzero-store/src/store/query/delta_codec.rs`).
- Library properties map through proptest's default `SourceParallel` layout
  (they sit under `src/`, where proptest finds `src/lib.rs`).
- Integration-test properties (`tests/`) cannot use `SourceParallel`: proptest
  finds no `lib.rs` walking up from `tests/` and would fall back to a
  `*.proptest-regressions` file next to the test file. They therefore pin the
  exact crate-local persistence path with
  `FileFailurePersistence::Direct(concat!(env!("CARGO_MANIFEST_DIR"),
  "/proptest-regressions/tests/<file>.txt"))`. Their shrunk failures land
  directly in the committed layout at
  `crates/<crate>/proptest-regressions/tests/<file>.txt`; nothing is moved
  manually. Any other `Direct` target is rejected by the checker.

## Manual `TestRunner` sites

Library properties built with `TestRunner` directly must construct an explicit
`Config` that sets `source_file: Some(file!())` and otherwise uses
`Config::default()` (default failure persistence; `PROPTEST_RNG_SEED` honored;
no fixed seed). `TestRunner::default()` is forbidden because its missing
`source_file` makes regression persistence inert.

## Targeted replay examples

Library property (store):

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero \
  cargo test -p graphzero-store store::query::delta_codec::tests::prop_encode_decode_symbol_identity \
  --lib -- --exact --test-threads=1
```

Integration-test property (query):

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero \
  cargo test -p graphzero-query --test blast_proptest parse_intent_never_panics -- --test-threads=1
```
