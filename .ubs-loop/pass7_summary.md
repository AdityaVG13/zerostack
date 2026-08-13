# UBS Pass 7 -- Rust categories 21 and 15

Panic Surfaces & Unwinding + API Misuse.

Hunt leftover *library* panic surfaces that pass 1 missed: `unwrap_or_else(|_| panic!())`, `unreachable!` in lib, `todo!` in lib, dropped `Result` (`let _ =` on security-sensitive I/O), `mem::forget` leftovers, `std::process::exit` in libs.

## Commands

| Command | Exit |
|---|---|
| `rg` `unwrap_or_else`+`panic!`, `unreachable!`, `todo!`, `unimplemented!`, `mem::forget`/`ManuallyDrop`, `process::exit`, `unwrap_unchecked`/`unreachable_unchecked`, `DefaultHasher`, `let _ =` on File/write/fsync/lock, `write_all`/`sync_all`, Drop impls under `crates/` | 0 |
| `/opt/homebrew/bin/bash .../ubs-rust.sh --only=21,15 --no-cargo --format=json --ci --summary-json=.ubs-loop/pass7_summary.json --emit-findings-json=.ubs-loop/pass7_findings.json crates/ .ubs-loop/pass7.json` | **0** -- critical=0 warning=3236 info=463 |

Authoritative cat-21/15 artifact: `.ubs-loop/pass7.json` (crates-wide). Findings: `.ubs-loop/pass7_findings.json`. Summary: `.ubs-loop/pass7_summary.json`.

`--only=21,15` works on `ubs-rust.sh`. Wrapper `ubs --only=rust` is not cat-21/15-only (same as prior passes). `--no-cargo` as required; no workspace cargo.

UBS `--ci` exits 1 whenever `critical >= 1`. Exit 0 here: no `unreachable_unchecked` / `unwrap_unchecked`. The 3236 warnings are inventory (see triage).

## Files changed (this pass)

None. ZERO-CHANGE.

No rival dirty tracked files. Untracked `.ubs-loop/pass1_*` .. `pass6_*` left untouched. New scan dumps stay untracked except this summary.

No rch test: no behavior change.

## Cat-21/15 triage (production src)

### Real -- fixed

None.

### Warning / info inventory (UBS)

| Count | Severity | Title | Verdict |
|---|---|---|---|
| 2780 | warning | `assert!` macros present | FP. Production `assert!` outside `#[cfg(test)]` / `#[test]` is only `zero-testkit` fault-matrix helpers. Remaining hits are tests + `debug_assert!(pgid > 1)` in `child.rs`. |
| 456 | warning | Direct indexing/slicing may panic | FP. Heuristic matches every `[..]`. Inspected lib samples: `token[start..]` after `str::find` (in-bounds); `serde_json::Value` `args["key"]` returns `Null`, does not panic; `windows(2)` / hex tables / proven slices. |
| 434 | info | `unwrap_err`/`expect_err` | Tests only (gate/ledger/codemode fixtures). No lib path. |
| 29 | info | `unwrap_or_default` | Inspected. Optional JSON fields, empty-query help, clock-before-epoch timestamps (session id still unique via pid+seq), GC `duration_since` fail-closed (age 0 => keep), schema missing keys => empty vec. Not silent security I/O. |
| 0 | -- | `DefaultHasher` | Not present. |

### `rg` leftover-panic hunt (pass 1 leftovers)

| Pattern | Lib hits | Verdict |
|---|---|---|
| `unwrap_or_else(... panic!)` | 2 production + tests | Permit `scoped_permit_base*` -- documented panicking wrappers, `ubs:ignore`, callers use `try_*`. Tests: `raw_worker` field inventory, connector reachability. |
| `unreachable!` | 4 lib + 1 testkit | Already-correct invariants (pass 1). See below. |
| `todo!` / `unimplemented!` | none | Only a `//!` example in `adapter.rs` (`# todo!()`). |
| `mem::forget` / `ManuallyDrop` | none | Same as pass 5. |
| `unwrap_unchecked` / `unreachable_unchecked` | none | Matches UBS critical=0. |
| `std::process::exit` | bins only | `zsx/src/main.rs`, `process_fixture`, `worker_fixture`. No lib. |
| `catch_unwind` | `zsx-node` | Contains backend panic into a typed envelope. Not a new panic surface. |

### Dropped `Result` (`let _ =` on File/write/fsync/lock)

No `let _ = file.write_all` / `sync_all` / `fsync` in lib. Production `write_all`/`sync_all` use `?` (`cas`, journals, `fs_replace`, WAL, permit `write_file`).

| Site | Why left |
|---|---|
| `gc_lock.rs:172` `file.unlock()` | Drop. Kernel releases the flock on close. |
| `cas.rs:551` `sync_dir` after quarantine rename | Documented: object is already out of the tree; dir fsync failure is durability, not a failed move. |
| `fs_replace.rs:187` post-publish `sync_dir` | Same: dest is published; returning Err would lie. |
| `cas.rs:381` `touch_path` | Best-effort mtime on CAS dedup. Put already verified. |
| `cas`/`journals`/`metadata` `remove_file(temp)` | Cleanup after publish or failed publish. create_new ownership. |
| `session_wal.rs:231` `clear_wal` | After snapshot publish. Leftover WAL replays; fail-safe. |
| `quarantine_exact` rollback `rename` | Returns `false` if identity mismatches. Fail-closed. |
| `legacy_exclusive_busy` `reclaim_dead` | Re-checks `looks_like_legacy_exclusive_permit`. Fail-closed (Busy). |
| `indeterminate_mutation_journal` | Already returning `Err`. Recovery treats DispatchCrossed without evidence as Indeterminate. |
| `write!(out, "{b:02x}")` | `fmt::Write` to `String` is infallible. |
| `create_dir_all` in `tokenzero.rs:145` / `fszero.rs:561` | Best-effort pre-create. Session already `create_dir_all`+`canonicalize`s `state_root` and maps errors. TokenZero `AdapterContractError` is binding-only; later persist fails loud. FSZero constructor is documented infallible; `build_canonical` refuses `degraded()`. |
| channel `send` / `join` / `kill` / `revoke` | Drop/shutdown best-effort. Not security I/O. |

### FP / already correct -- left

- Permit `scoped_permit_base*` -- pass 1 ignore; `try_*` is the lib path.
- `two_phase.rs` `self.expect(ControllerInstruction::...)` -- method, not `Result::expect`.
- `zsx-core` AdapterBinding / thread-spawn expects -- documented infallible constructors.
- `zero-mcp` `serde_json::to_string(Value).expect` -- `Value` serialize cannot fail. `McpDispatchError` is a owned typed struct.
- `zsx/src/main.rs` serialize expect -- bin.
- `zero-testkit` fixture expects / `unreachable!` -- test helpers, not hub lib.
- `zero-gauge` unwraps sit under `#[cfg(test)]` (line 392+).
- `fixture.rs` CAS publish expect -- fixture adapter, known-valid bindings.
- Clock `duration_since(UNIX_EPOCH).unwrap_or_default()` -- timestamp 0 if clock is pre-epoch; not a panic and not a dropped I/O Result.

### Not present / out of scope

- Cats 1-14, 16-20, 22-24. No pass 1 unwrap redo unless a *new* panic remained (none did).
- `scripts/` -- no rust this pass (polyglot is pass 8).
- CLI bins (`process_fixture`, `worker_fixture`, `zsx`, `program-evidence`).

## Already correct (checked, no change)

1. `crates/zero-codemode/src/interpreter.rs:2550-2561` -- `new_promise` always inserts `Value::Promise`; the `unreachable!` arms cannot fire unless the constructor is rewritten.
2. `crates/zero-store/src/store_root.rs:526-537` -- `Component::CurDir` is stripped before the match; `unreachable!` is the leftover exhaustiveness hole.
3. `crates/zero-store/src/attempt_journal.rs:1019-1043` -- `terminal_entry` only builds succeeded/failed/indeterminate; expects fire only if the typed caller omits required evidence.
4. `crates/zerostack-machine-permit/src/lib.rs:119-132` -- panicking `scoped_permit_base*` wrappers; production uses `try_scoped_permit_base*`.
5. `crates/zero-store/src/gc.rs:1022-1036` -- `push_bounded_set` inserts the truncated marker then removes a non-marker; the set always has a removable item while `len > MAX`.
6. `crates/zero-store/src/fs_replace.rs:168-193` -- `atomic_write_file_with_sync` deletes the sibling temp when replace fails; post-publish dir fsync is best-effort so a successful write is not reported as failed.
7. `crates/zero-store/src/gc_lock.rs:169-173` -- `StoreLock::drop` unlocks; kernel also releases on close.
8. `crates/zsx-core/src/fszero.rs:204-207` -- `token[start..]` only after `str::find(prefix)`, so `start` is in-bounds.
9. `crates/zero-store/src/cas.rs` + journals -- `write_all`/`sync_all`/`create_dir_all` propagate via `?`; temp `remove_file` is cleanup only.
10. `crates/zero-process/src/child.rs:1159` -- `debug_assert!(pgid > 1)` is a spawn invariant, not an input-facing `assert!`.
11. `crates/zero-mcp/src/mcp_transport.rs:328-330,691,836` -- `serde_json::to_string` on `Value` / owned error structs.
12. No `mem::forget`, `ManuallyDrop`, `unwrap_unchecked`, `unreachable_unchecked`, or lib `process::exit` in `crates/`.

## Tests

None. No production behavior change.

## Suggested commit paths

```
.ubs-loop/pass7_summary.md
```

Do not add `.ubs-loop/pass7.json` / `pass7_findings.json` / `pass7_summary.json` (inventory dumps) unless a later pass wants them. Do not touch pass 1-6 artifacts or rival files.
