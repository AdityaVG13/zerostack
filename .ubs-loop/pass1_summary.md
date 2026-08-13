# UBS Pass 1 -- Rust category 1 (Ownership & Error Handling)

Baseline + library unwrap/expect/panic!/unreachable!/todo!/unimplemented! in `crates/`.

## Commands

| Command | Exit |
|---|---|
| `ubs doctor` | 0 |
| `ubs --only=rust --format=jsonl --ci --skip-size-check .` | 1 |
| `ubs --only=rust --format=json --ci --skip-size-check --report-json=.ubs-loop/pass1_summary.json .` | 1 |
| `ubs --only=rust --skip-rust=2..24 --format=text --ci --skip-size-check crates/` | 1 |
| `ubs --only=rust --skip-rust=2..24 --files=<changed>` | 1 (remaining hits are cfg(test) or ignored FPs) |

Full rust scan totals: 175 files, critical=991, warning=11555, info=3850.
Category 1 on `crates/`: 176 panic!, 6193 unwrap/expect, 10 unreachable!.

## Files changed (this pass)

| File | Why |
|---|---|
| `crates/zero-abi/src/digest.rs` | Replace `from_digit().unwrap()` with a hex table (same as assembly.rs). |
| `crates/zero-abi/src/schema.rs` | `items` used `is_array()` + `as_array().unwrap()`; now `match as_array()`. |
| `crates/zero-abi/src/dispatch.rs` | Stage-machine `operation().expect` and schema `as_object`/`as_array`/`unreachable!` now return `DispatchContractError` / `Err(String)`. |
| `crates/zero-codemode/src/host.rs` | `ZeroResultV1::inline(...).expect` -> `map_err(HostError::Json)?`. |
| `crates/zero-codemode/src/worker.rs` | Handshake `Path::to_str().expect` -> `Result` + `Configuration` error. |
| `crates/zsx-core/src/fszero.rs` | Approval grant `is_some()` + `as_ref().expect` -> `if let`; remaining constructor expects ignored. |
| `crates/zsx-core/src/connector.rs` | Meter double-lock `as_mut().expect` -> single lock + `if let` error. |
| `crates/zsx-node/src/envelope.rs` | Build `Map` instead of `json!` + `as_object_mut().expect`. |
| `crates/zsx-node/src/tasks.rs` | Same Map construction for control outcomes. |
| `crates/zero-gate/src/two_phase.rs` | Plan `position().expect` after count check -> `ok_or_else(KernelError)`. `self.expect(...)` is a method; ignored. |
| `crates/zero-gate/src/transaction.rs` | `Unsupported => unreachable!()` -> same typed `UnsupportedIsolation` error as the early check. |
| `crates/zerostack-machine-permit/src/lib.rs` | Waiter `parent().expect` -> `AcquireError`. Documented panicking `scoped_permit_base*` wrappers ignored (`try_*` is the lib path). |
| `crates/zero-process/src/pipe.rs` | `take().expect("instance present")` -> `io::Error`. |
| `crates/zero-store/src/cas.rs` | Path `parent().expect` -> `CasError::Malformed`. Temp-create `find_map().expect` -> loop + `CasError::Io`. |

Do **not** commit `crates/zsx-core/src/session.rs` -- rival dirty file (8-line BackendUnavailable change), not this pass.

## Cat-1 triage (priority library src)

### Real -- fixed
- `crates/zero-abi/src/schema.rs:92` unwrap after `is_array()`
- `crates/zero-abi/src/dispatch.rs:525,532,549,574,862,876,947` expect/unreachable on stage + schema
- `crates/zero-abi/src/digest.rs:20-21` hex nibble unwrap
- `crates/zero-codemode/src/host.rs:460` expect on `Result`
- `crates/zero-codemode/src/worker.rs:55` UTF-8 expect (now matches `validate_spec`)
- `crates/zsx-core/src/fszero.rs:416` guarded `as_ref().expect`
- `crates/zsx-core/src/connector.rs:980` meter expect after a separate lock
- `crates/zsx-node/src/envelope.rs:108-126` and `tasks.rs:244-252` `as_object_mut().expect` on `json!` objects
- `crates/zero-gate/src/two_phase.rs:1441,1445` `position().expect` after count
- `crates/zero-gate/src/transaction.rs:267` `unreachable!` for `Unsupported` (already rejected above)
- `crates/zerostack-machine-permit/src/lib.rs:707` waiter parent expect
- `crates/zero-process/src/pipe.rs:301` pending instance expect
- `crates/zero-store/src/cas.rs:59,343,440` path/temp-create expects

### FP / already correct -- left or ignored
- `zero-gate/src/two_phase.rs:1801+` `self.expect(ControllerInstruction::...)` is `BrokeredExecution::expect`, not `Result::expect` (`ubs:ignore`)
- `zerostack-machine-permit` `scoped_permit_base*` -- documented panicking wrappers; production uses `try_*` (`ubs:ignore`)
- `zsx-core/src/fszero.rs` AdapterBinding + thread spawn expects -- constructors documented infallible (`ubs:ignore`)
- `zero-codemode/src/interpreter.rs:1996,2518` `unreachable!` -- `new_promise` always returns `Value::Promise`
- `zero-store/src/store_root.rs:536` `CurDir` is never retained
- `zero-store/src/attempt_journal.rs:1031-1042` terminal states require matching evidence
- `zero-ledger/src/fresh_work.rs:192-193` ppm <= PPM_ONE by component-sum invariant
- `zero-ledger/src/lib.rs:702` `sha256_hex` always yields 64 lowercase hex
- `zero-gate/src/evidence.rs:572,622-633` serialize / validated-report slot expects
- `zero-abi/src/cache_entry.rs:291` `CacheKeyV1` is `Serialize`
- `zsx-core/src/fixture.rs` fixture adapter (known-valid bindings / CAS)
- `zsx-core/src/graphzero.rs:137` AdapterBinding constants
- Almost all remaining unwrap/panic in priority crates sit under `#[cfg(test)]` modules

### Not fixed this pass (later / out of scope)
- Cats 2-24
- Test-only unwrap/panic (should panic on unexpected None)
- `zero-testkit` / `zero-mcp` / `zero-gauge` (not priority)

## Already correct (checked, no change)

1. `zero-codemode/src/interpreter.rs` `new_promise` always inserts `Value::Promise`; the `unreachable!` arms cannot fire unless the constructor is rewritten.
2. `zero-store/src/store_root.rs` `Component::CurDir` is stripped before the match; `unreachable!` is the leftover exhaustiveness hole.
3. `zero-gate/src/evidence.rs` `report_digest` runs only after `attach_report` filled that class slot.
4. `zero-ledger/src/fresh_work.rs` `eta_action_ppm` divides `fresh * PPM_ONE / total` with `fresh <= total` by construction.
5. `zero-abi/src/cache_entry.rs` `canonical_key_json` serializes a fully-owned typed struct.

## Tests

RCH `exec` was paused (`admission_closed=true`, daemon restart remediation; many stale client leases). Targeted local tests with `CARGO_TARGET_DIR=/tmp/rch_target_zerostack`:

- `cargo test -p zero-abi --lib -- --test-threads=1` -- 119 passed
- `cargo test -p zero-store --lib cas -- --test-threads=1` -- 29 passed
- `cargo test -p zero-gate --lib two_phase:: -- --test-threads=1` -- 10 passed
- `cargo test -p zero-machine-permit --lib -- --test-threads=1` -- 39 passed

## Suggested commit paths

```
crates/zero-abi/src/digest.rs
crates/zero-abi/src/schema.rs
crates/zero-abi/src/dispatch.rs
crates/zero-codemode/src/host.rs
crates/zero-codemode/src/worker.rs
crates/zsx-core/src/fszero.rs
crates/zsx-core/src/connector.rs
crates/zsx-node/src/envelope.rs
crates/zsx-node/src/tasks.rs
crates/zero-gate/src/two_phase.rs
crates/zero-gate/src/transaction.rs
crates/zerostack-machine-permit/src/lib.rs
crates/zero-process/src/pipe.rs
crates/zero-store/src/cas.rs
.ubs-loop/pass1_summary.md
```

Do not add `.ubs-loop/*.json*` (large enum dumps) or `crates/zsx-core/src/session.rs`.
