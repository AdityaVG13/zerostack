# UBS Pass 9 -- Rust category 11 (Tests & Benches Hygiene)

Hunt: `#[ignore]` hiding red, `assert!(true)`, empty tests, `let _ = result`, tests that swallow the bug they claim to test. Also `tests/` and crate `#[cfg(test)]`.

Prior: 1-8 including senpi reap `01099ab`.

## Commands

| Command | Exit |
|---|---|
| `ubs-rust.sh --only=11 --no-cargo --format=json --ci crates/ .ubs-loop/pass9_crates.json` | **0** -- critical=0 warning=0 info=0 files=143 |
| `ubs-rust.sh --only=11 --no-cargo --format=json --ci tests/ .ubs-loop/pass9_tests.json` | **0** -- critical=0 warning=0 info=0 files=23 |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-gate --test program_evidence fail_closed_reports_are_typed_and_distinct -- --test-threads=1` | **0** -- 1 passed |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zsx-core --features tokenzero --lib request_frame_roundtrip_keeps_trace_binding -- --test-threads=1` | **0** -- 1 passed |

Authoritative cat-11 artifact: `.ubs-loop/pass9.json` (crates + tests merged). Wrapper takes one `PROJECT_DIR`; `crates/ tests/` would treat `tests/` as `OUTPUT_FILE`. Per-dir dumps: `pass9_crates.json`, `pass9_tests.json`.

`--only=11` works on `ubs-rust.sh`. `--no-cargo` as required; no workspace cargo.

UBS cat 11 only counts literal `#[ignore]` and `todo!`/`unimplemented!` within 5 lines of `#[test]`. Combined info=0 is a **scanner miss**, not an empty ignore inventory.

No rch/workspace cargo besides the two targeted tests above.

## Files changed (this pass)

| File | Why |
|---|---|
| `crates/zsx-core/src/tokenzero.rs` | `request_frame_roundtrip_keeps_trace_binding` only validated a hand-built frame. Now binds a result and asserts the adapter echoes `request.trace`. |
| `crates/zero-gate/tests/program_evidence.rs` | `fail_closed_reports_are_typed_and_distinct` named a successful assemble `stale` and `let _ =` it. Now writes a foreign hub head and asserts `StaleHead`. |

No rival dirty tracked files. Untracked `.ubs-loop/pass1_*` .. `pass8_*` left untouched. New scan dumps stay untracked except this summary (and `pass9.json` as the required artifact).

## Real -- fixed

1. **TokenZero trace test did not exercise the adapter.** Name and comment claim the adapter echoes `request.trace` verbatim (the connector rejects any other binding). The body only called `validate_request_frame(&frame)`. Validation takes `&frame` and cannot drop the echo. A dropped `metadata.trace = request.trace.clone()` would still pass. Fix: `bind_outcome` + `assert_eq!(response.result.metadata.trace, request.trace)`.
2. **Program-evidence "stale" path was a successful assemble.** Comment: "Missing class vs stale head vs digest mismatch must be distinguishable." Code: `let stale = assemble_program_evidence(...).unwrap(); let _ = stale;` with a note that CLI tests cover stale. If `StaleHead` started succeeding, this test still passed. Fix: write a worker artifact bound to `head(0x99)` and assert `ProgramEvidenceFailureV1::StaleHead`. Keep the malformed-report case so the three codes stay distinct.

## Cat-11 / hunt inventory

| Count | Sev | Title | Verdict |
|---|---|---|---|
| 0 | info | `#[ignore]` (UBS regex `#\[ignore\]`) | **FP / scanner miss.** Three real attrs are `#[ignore = "opt-in ..."]`. |
| 3 | -- | `#[ignore = "..."]` in `aggregate_scale.rs` | **Already-correct.** Documented opt-in release scale (100k ops / 30-sample). Real asserts; not hiding red. |
| 0 | -- | `assert!(true)` / `assert!(false)` | None. |
| 0 | -- | Empty `#[test]` bodies | None. |
| 0 | -- | `todo!` / `unimplemented!` near `#[test]` | None. |
| 0 | -- | `#[should_panic]` | None. |
| many | -- | `let _ =` in tests | **FP / already-correct.** Almost all `remove_dir_all` cleanup, `child.kill`/`wait` reap, type pins, or `let _guard` Drop tests. See already-correct list. |
| 4 | -- | Criterion benches | **Already-correct.** All `black_box` inputs; `iter` returns the result (Criterion black-boxes the closure output). |

## Already-correct (checked, >=5)

1. `crates/zero-codemode/tests/aggregate_scale.rs` -- three `#[ignore = "opt-in ..."]` scale gates with real asserts; run via `--ignored`.
2. `crates/zero-codemode/tests/host_contract.rs` `cancel_timeout_entrypoint_is_available` -- compile-time existence pin of `Host::execute_with_cancel_timeout`.
3. `crates/zero-store/src/cas.rs` `short_and_non_hex_identities_never_panic` -- `get_verified` is typed `malformed`; `let _ = object_path(bad)` is the non-panic contract (`PathBuf`, not `Result`).
4. `crates/zero-store/src/gc_lock.rs` `acquisition_is_deadline_bounded` -- `let _sweep` holds the lock; asserts `WouldBlock` and elapsed bounds.
5. `crates/zero-codemode/src/interpreter.rs` `depth_guard_unwinds_after_early_error_return` -- `let _ = probe(..., true)` is the early `Err` path; depth is asserted back to 0.
6. `crates/zero-process/src/identity.rs` `owner_watch_waits_for_short_lived_child` -- `let _ = child.wait()` reaps after `watcher.wait()`; `!id.is_live()` is the property.
7. `crates/zero-gate/src/program.rs` `proof_is_opaque_and_linear` -- compile-time fence (no `Clone`/`Deserialize`); `let _ = proof.program_digest()` uses the value.
8. `crates/zero-cert/tests/golden_vectors.rs` `rejects_cross_query_parameter_substitution_for_every_witness` -- helper `rejects_bound_substitution` asserts `InvalidCompleteness` / `WitnessQueryMismatch`.
9. `crates/zerostack-machine-permit/src/lib_inline_tests.rs` `sleeping_fallback_preserves_release_correctness` / `native_wake_handle_closes_on_drop` -- `.expect` + `remove_dir` is the close-on-drop assertion.
10. `crates/zero-ref/benches/parse.rs`, `zero-ledger/benches/charge.rs`, `zero-cert/benches/verify.rs`, `zero-gate/benches/decide.rs` -- `black_box` + fail-loud on invalid bench inputs.
11. `tests/src/fake_substrate.rs` `let _ = self.mcp_request(...)?` -- `?` still fail-loud; discards initialize payload only.
12. `tests/tests/racc_gates.rs` -- `assert_fails` checks `CheckStatus::Fail` for every mutation class.

## Leftover (not a swallowed claimed bug)

- `host_contract.rs` `invalid_connector_json_is_rejected` still uses `.is_err()` only. Execute-Ok on accepted junk would fail the test; a different `Err` would not. Not fixed (claimed property is still gated).
- UBS cat 11 will keep reporting info=0 until the scanner matches `#[ignore = ...]`.
- CHAR pin tests (`char_*`) are compile/print fences, not behavioral asserts.

## Tests

| Filter | Result |
|---|---|
| `zero-gate --test program_evidence fail_closed_reports_are_typed_and_distinct` | 1 passed |
| `zsx-core --features tokenzero --lib request_frame_roundtrip_keeps_trace_binding` | 1 passed |

## Suggested commit paths

```
crates/zsx-core/src/tokenzero.rs
crates/zero-gate/tests/program_evidence.rs
.ubs-loop/pass9.json
.ubs-loop/pass9_summary.md
```

Do not add `pass9_crates.json` / `pass9_tests.json` / findings dumps unless a later pass wants them. Do not touch pass 1-8 artifacts or rival files.
