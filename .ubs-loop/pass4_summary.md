# UBS Pass 4 -- Rust categories 4, 5, 22

Numeric & Floating-Point + Collections & Iterators + Suspicious Casts & Truncation.

Hunt: `/ 0`, `% 0`, float `==`, `as u8`/`as u32` truncation, empty-iter `unwrap` in lib paths, off-by-one, `get_unchecked`.

## Commands

| Command | Exit |
|---|---|
| `rg` `/ 0`, `% 0`, float `==`/`!=` literals, `as u8`/`u16`/`i8`/`i16`, `len()/count() as` narrow, `try_into().unwrap`, `get_unchecked`, iterator `.next/.first/.nth/.last.unwrap`, `step_by`/`chunks`/`windows`, integer `/` under `crates/` | 0 |
| `/opt/homebrew/bin/bash .../ubs-rust.sh --only=4,5,22 --no-cargo --format=json --ci --summary-json=.ubs-loop/pass4_cat4_summary.json --emit-findings-json=.ubs-loop/pass4_cat4_findings.json crates/ .ubs-loop/pass4_cat4.json` | **0** -- critical=0 warning=170 info=2254 |

Authoritative cat-4/5/22 artifact: `.ubs-loop/pass4_cat4.json` (crates-wide). Findings: `.ubs-loop/pass4_cat4_findings.json`.

`--only=4,5,22` works on `ubs-rust.sh`. Wrapper `ubs --only=rust` is not cat-4/5/22-only (same as prior passes). `--no-cargo` as required; no workspace cargo.

No `get_unchecked`, no `try_into().unwrap/expect`, no `nth(0)`, no literal `/ 0` or `% 0` in production src.

## Files changed (this pass)

None. ZERO-CHANGE candidate.

No rival dirty tracked files. Untracked `.ubs-loop/pass1_*` / `pass2_*` / `pass3_*` left untouched. New scan dumps stay untracked except this summary.

No rch test: no behavior change.

## Cat-4/5/22 triage (production src)

### Real -- fixed

None.

### Warning inventory (170)

| Count | Category | Title | Verdict |
|---|---|---|---|
| 161 | Collections & Iterators | `clone()` inside loops | Perf nit, not overflow/truncation/empty-iter. Many are tests (`zsx-core/tests/request_cancellation.rs`). Left. |
| 9 | Suspicious Casts & Truncation | `len()/count() as` narrow | All 9 inspected. 7 lib + 2 examples. All bounded or FFI-sized. Left. |

The 9 narrow-cast sites:

1. `crates/zero-process/src/pipe.rs:280` -- `handles.len() as u32` on a 2-element `[HANDLE; 2]`. Always 2.
2. `crates/zero-process/src/random.rs:28` -- `buffer.len() as u32` after `if buffer.len() > u32::MAX` (`random.rs:9-14`).
3. `crates/zero-gate/src/two_phase.rs:1878` -- `staged_effects.len() as u32`. One `StageEffect` per plan instruction; mutating plans require exactly one (`two_phase.rs:1480-1488`). Cannot approach `u32::MAX`.
4. `crates/zero-gate/src/transaction.rs:529-531` -- `len() as u16` after `request.validate()` (`:448`) which rejects `> TRANSACTION_MAX_RESOURCES_V1` (256) (`:29`, `:369-378`). Subsets `external` / `external_debt` cannot exceed that.
5. `crates/zero-gate/src/transaction.rs:694` -- `durable_profile_id.as_str().len() as u16`. `DurableProfileIdV1` is four short constants (`portable_strict` ... `ntfs_strict`).
6. `crates/zero-testkit/examples/native_broker_boundary.rs:189` -- example path prefix, not a lib path.
7. `crates/zero-testkit/examples/native_durable_journal.rs:136` -- same.

### FP / already correct -- left

- Cat 4 float `==` (info=302) -- JS Number semantics in `interpreter.rs` (`raw_count == 0.0`, `fract() == 0.0` with `is_finite` + 2^53 bound before `as i64`). Not IEEE-epsilon equality of measurements.
- Cat 4 `/ var` (info=411) -- regex also hits comments, URLs, paths. Real integer `/` sites are guarded (see below).
- Cat 4 `% var` (4 hits) -- 1 comment (`zero-ledger`); 3 real, all guarded.
- Cat 5 `clone()` / `collect::<Vec<_>>()` -- inventory / perf. Out of scope.
- Cat 22 `as` inventory (info=335) -- `#[repr(u8)]` discriminants (`Guard`, `FailureCode`, `PeerOwner`, `ExecutionSurface`), test fixture `index as u8`, `bool as u8`.
- `cas.rs:729` `sequences.next().unwrap()` -- `#[cfg(test)]` fixture iterator of two items.
- `interpreter.rs:3541` `.as_array().unwrap().first().unwrap()` -- test walking a 64-deep array it just built.
- `interpreter.rs` `args.next().unwrap_or(Undefined)` -- JS optional args, not empty-iter panics.

### Not present / out of scope

- Cats 1-3, 6-21, 23-24. No pass 1 unwrap redo, no pass 2 `zeroed`, no pass 3 `RefCell`.
- `clone()`-in-loop perf (161 warnings).
- Testkit / examples except as named FPs.
- Windows-only `cpu_seconds.saturating_mul(10_000_000) as i64` (`child.rs:1039`) -- default 300 s fits; `validate()` does not cap `cpu_seconds`, so a huge policy could wrap to a negative `LARGE_INTEGER`. Not a lib-path panic; cannot exercise Job Objects on this Mac. Left.

## Already correct (checked, no change)

1. `crates/zero-process/src/resource.rs:31-54` -- `share` returns `InvalidInput` when `workers == 0` before `/ workers`.
2. `crates/zero-gauge/src/lib.rs:180-198` -- `capacity` is `NonZeroU64`; `allocation / capacity` and `% capacity` cannot divide by zero.
3. `crates/zero-gate/src/recovery.rs:1286-1292` -- Euclidean `gcd` is `while right != 0 { left % right }`.
4. `crates/zero-codemode/src/interpreter.rs:2466-2471` -- empty pad returns before `needed / pad_chars` and `% pad_chars`.
5. `crates/zero-ledger/src/lib.rs:1183-1191` and `fresh_work.rs:186-190` -- `raw == 0` / `total_tokens == 0` return before `/`.
6. `crates/zero-cert/src/lib.rs:1145-1161` -- empty `pattern` is `InvalidCompleteness` before `windows(pattern.len())`.
7. `crates/zero-codemode/src/interpreter.rs:2415-2426` -- empty `String.repeat` returns; `memory_bytes / value.len()` and `count as usize` sit behind finite/non-negative/max checks.
8. `crates/zero-ref/src/lib.rs:547-566, 647-656` -- byte/line spans error on reverse/empty/OOB before `as usize` indexing. 1-based inclusive line math uses `line_starts[start-1]` after `start >= 1`.
9. `crates/zero-abi/src/zbf.rs:584-598` -- `take(1)?[0]` only indexes a proven 1-byte slice.
10. `crates/zsx-core/src/help.rs:307-366` -- `limit` is clamped; huge `offset` makes `skip` empty so `offset + shown` cannot overflow; `saturating_sub` for remaining.

No `get_unchecked` anywhere in `crates/`. `windows(2)` / `chunks_exact(2)` never use a zero window.

## Tests

None. No production behavior change.

## Suggested commit paths

```
.ubs-loop/pass4_summary.md
```

Do not add `.ubs-loop/pass4_cat4*.json*` (inventory dumps) unless a later pass wants them. Do not touch pass 1-3 artifacts or rival files.
