# proptest-regressions (graphzero-reserve)

Committed shrunk proptest regressions for `graphzero-reserve`.

This directory is intentionally empty except for this README. A `.txt` file
appears here only when a real proptest failure generates one. An empty layout
means no failing seed is persisted; it does not prove that no property has
ever failed.

Rules (enforced by `scripts/check_proptest_contract.py`):

- Never hand-edit or fabricate regression lines. Proptest writes them, one
  failing seed per line, in proptest's persistence format.
- This crate's properties are integration tests, where proptest's default
  `SourceParallel` finds no `lib.rs` above `tests/`. The macro config pins
  `FileFailurePersistence::Direct(concat!(env!("CARGO_MANIFEST_DIR"),
  "/proptest-regressions/tests/reserve_proptest.txt"))`, so shrunk failures
  land in this directory directly (as `tests/reserve_proptest.txt` mirrors
  `tests/reserve_proptest.rs`). No manual moving is needed or allowed.
- Replay a failing seed exactly with `PROPTEST_RNG_SEED=<emitted>`.
  `PROPTEST_DISABLE_FAILURE_PERSISTENCE` is forbidden in CI and release
  checks. See `docs/proptest.md`.
