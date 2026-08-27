# proptest-regressions (graphzero-store)

Committed shrunk proptest regressions for `graphzero-store`.

This directory is intentionally empty except for this README. A `.txt` file
appears here only when a real proptest failure generates one. An empty layout
means no failing seed is persisted; it does not prove that no property has
ever failed.

Rules (enforced by `scripts/check_proptest_contract.py`):

- Never hand-edit or fabricate regression lines. Proptest writes them, one
  failing seed per line, in proptest's persistence format.
- `graphzero-store` properties live in `src/` module tests, so proptest's
  default `SourceParallel` layout writes here automatically, mirroring the
  source path (for example `store/query/delta_codec.txt` mirrors
  `src/store/query/delta_codec.rs`).
- Replay a failing seed exactly with `PROPTEST_RNG_SEED=<emitted>`.
  `PROPTEST_DISABLE_FAILURE_PERSISTENCE` is forbidden in CI and release
  checks. See `docs/proptest.md`.
