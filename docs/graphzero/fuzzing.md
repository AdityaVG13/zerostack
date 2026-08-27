# Fuzzing (cargo-fuzz)

GraphZero ships a small root `fuzz/` package with three libFuzzer targets.
They are parser/signature **robustness** fuzzers. They search generated inputs
for decoder panics and check decode/re-encode and fixture-key sign/verify
invariants. They do **not** prove full semantic correctness or absence of all
crashes in the query layer, packs, or SCIP ingestion.

## Targets

| Target | Entry | Invariant |
|---|---|---|
| `delta_codec` | `fuzz/fuzz_targets/delta_codec.rs` | Successful `decode_symbol`/`decode_edge` must re-encode and decode back to the identical semantic tuple; trailing bytes are not required to be canonical. |
| `pack_sign` | `fuzz/fuzz_targets/pack_sign.rs` | Parse failures are normal; the input's own key/signature fields are never trusted. A fixture-signed manifest must verify with the fixture key, and flipping its first signature hex nibble must fail verification. |
| `scip_parse` | `fuzz/fuzz_targets/scip_parse.rs` | `decode_scip_bytes` must never panic on arbitrary bytes; decode errors are normal. |

## Run

All Rust verification is through RCH with the shared target dir. Smoke runs
are bounded; corpus/artifacts are gitignored (`/fuzz/corpus/`, `/fuzz/artifacts/`,
`/fuzz/target/`).

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo fuzz list
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo fuzz run delta_codec -- -runs=64 -max_len=4096
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo fuzz run pack_sign -- -runs=64 -max_len=4096
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero cargo fuzz run scip_parse -- -runs=64 -max_len=4096
```

Formatting (RCH):

```bash
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_graphzero rustfmt --edition 2024 \
  fuzz/fuzz_targets/delta_codec.rs fuzz/fuzz_targets/pack_sign.rs fuzz/fuzz_targets/scip_parse.rs
```
