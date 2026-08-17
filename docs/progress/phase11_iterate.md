# Phase 11 iterate -- close CONFIRMED_GAP remediations

**Date:** 2026-08-17
**HEAD intent:** `phase11: close confirmed-gap remediations`
**Mode:** in-hub smallest-correct remediations only

## Card outcomes

| Rank | ID | Feature | Outcome | Status |
|---:|---|---|---|---|
| 1 | SURF-0003 | `F-REF-SERDE-FROMSTR` | `FromStr` + Display-form serde for `ZeroRefV1` | **CLOSED** / present |
| 2 | CONF-0006 | AGENTS.md preflight hash | Dropped from certifying pin; advisory yellow only | **CLOSED** |
| 3 | SURF-0001 | `F-FUZZ` | `fuzz/` cargo-fuzz targets `zeroref_parse` + `abi_frame_decode` | **CLOSED** / present |
| 4 | SURF-0007 | `F-ABI-PROPTEST-ROUNDTRIP` | `proptest!` encode/decode on Handshake/Shutdown | **CLOSED** / present |
| 5 | SURF-0009 | `F-STORE-ENSURE-LAYOUT` | `ensure_layout` creates `blobs/` and `gc/` | **CLOSED** / present |
| 6 | SURF-0004 | `F-REF-CAPABILITY-NEGOTIATION` | public `negotiate(major, minor)` | **CLOSED** / present |
| 7 | SURF-0012 | `F-REF-ERROR-TAXONOMY` | IncompatibleVersion wired; 4 store classes reserved | **CLOSED** / **partial** (honest) |
| 8 | SURF-0002 | `F-MIRI-NARROW` | `scripts/run_miri_narrow.sh`; host miri green | **CLOSED** / **partial** (rch did not offload) |

## Ranked item notes

1. **F-REF-SERDE-FROMSTR -- done.** `FromStr` delegates to `parse()`. Serde serializes the Display string. Tests in `crates/zero-ref/tests/zeroref_api.rs`.
2. **AGENTS.md preflight -- done.** `certifying = false` on `agents-law`. `verify_spec_source_hashes` skips it. Preflight reports `spec_source_sha256_advisory` as yellow on drift/absent. Did not rewrite AGENTS.md. Did not bless a moving gitignored hash.
3. **F-FUZZ -- done.** Real cargo-fuzz workspace. `cargo fuzz list` names `zeroref_parse` and `abi_frame_decode`. Seed corpus from the existing untrusted-bytes vectors.
4. **F-ABI-PROPTEST-ROUNDTRIP -- done.** `crates/zero-abi/tests/abi_proptest.rs`.
5. **F-STORE-ENSURE-LAYOUT -- done.** Creates `engine_dir`, `blobs/`, and `gc/` under `cas_host()`. Tests in `crates/zero-store/tests/ensure_layout.rs`.
6. **F-REF-CAPABILITY-NEGOTIATION -- done.** Major mismatch -> `IncompatibleVersion`. Minor is additive. Not a wire protocol.
7. **F-REF-ERROR-TAXONOMY -- partial (honest).** Parser/selector emit 5 classes. `negotiate` emits `IncompatibleVersion`. `Missing`/`Io`/`PolicyDenied`/`LegacyAmbiguity` are reserved for store/resolution (not parse errors). ALL is constructible; parser never emits reserved classes.
8. **F-MIRI-NARROW -- partial.** Script exists. Host run: 8 api + 2 proptest + 1 doctest green. rch warned `exec called with non-compilation command` and ran locally. retry_condition = `miri test -p zero-ref green on rch`.

## Verification

```
feature-universe ok: features=77 present=66 partial=8 missing=0 excluded=3 weight_sum=1.000000000000
feature-coverage-dashboard ok: effective=0.946058 strict=0.934426 gate=red
golden-integrity ok: checksums=12 artifacts=11 tier1=5 schema=1.0.0
cargo fuzz list: abi_frame_decode, zeroref_parse
cargo test -p zero-ref: 8 api + 2 proptest + 1 doctest ok
cargo test -p zero-abi: 13 lib + 2 proptest ok
cargo test -p zero-store: 2 ensure_layout ok
cargo test -p zerostack-harness --test golden_invariants: 6 ok
cargo test -p zerostack-harness --test oracle_smoke: 9 ok
scripts/run_miri_narrow.sh: host green (rch local fallback)
```

Gate stays **red** (partial + excluded remain). Partial never rounded up.

## Remaining

- CONFIRMED_GAP: 5 (`SURF-0006` CI tests, `SURF-0010` cancel hub test, `SURF-0011` Q99 residual, plus two non-ranked)
- OPEN: 19
- Out of repo / do-not: engine ClampEnd, conformance CLI, rival-dirty tree, fat-LTO
