# UBS Pass 10 -- Full crates/ rust re-scan

Hunt: NEW genuine bugs in production crates that passes 1-9 missed. Not the 771 security-token `==`/`!=` FPs.

Prior: 1-9 including senpi reap `01099ab` and test-hygiene `f76dbce`.

Wrapper `ubs --only=rust --comparison=` was not used: it would run cargo/clippy/audit on the workspace (forbidden here). Comparable baseline is `.ubs-loop/pass1_summary.json` (whole-repo + cargo). This pass used `ubs-rust.sh --no-cargo` on `crates/` and diffed counts by hand.

## Commands

| Command | Exit |
|---|---|
| `/opt/homebrew/bin/bash .../ubs-rust.sh --no-cargo --format=json --ci --summary-json=.ubs-loop/pass10_summary.json --emit-findings-json=.ubs-loop/pass10_findings.json crates/ .ubs-loop/pass10.json` | **1** -- critical=948 warning=10030 info=3554 files=143 |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-codemode --lib shorthand_object_destructure_binds_the_property -- --test-threads=1` | **0** -- 1 passed |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-codemode --lib interpreter::tests:: -- --test-threads=1` | **0** -- 18 passed |

Authoritative artifact: `.ubs-loop/pass10.json` (crates-wide, `--no-cargo`). Findings dump is quote-broken (same as pass 6/7); counts parsed from groups. Summary sidecar: `.ubs-loop/pass10_summary.json`.

`--no-cargo` as required; no workspace cargo. Cats 12-14 (fmt/clippy/check/deps) are cargo-only and empty here.

`--ci` exits 1 whenever `critical >= 1`. The 948 criticals are inventory (177 `panic!` mostly tests + 771 security FPs). See triage.

## Diff vs prior rust scans

| Scan | Scope | files | critical | warning | info |
|---|---|---|---|---|---|
| pass 1 (`ubs --only=rust` + cargo) | `.` | 175 | 991 | 11555 | 3850 |
| pass 6 after cookie fix (`--only=8,23`) | `crates/` | 143 | 771 | 122 | 1 |
| pass 7 (`--only=21,15`) | `crates/` | 143 | 0 | 3236 | 463 |
| pass 9 (`--only=11`) | `crates/` | 143 | 0 | 0 | 0 |
| **pass 10 (all cats, --no-cargo)** | `crates/` | **143** | **948** | **10030** | **3554** |

No new critical *class* vs passes 1-9. Security criticals still 771 (717 token `==` + 25 non-crypto RNG + 18 JWT + 5 secret literals + 4 Command + 2 shell `-c`). Ownership `panic!` 177 vs pass 1's 176 (test-vector `panic!` inventory). Warnings match prior inventories (unwrap 6212, assert 2782, indexing 457, clone-in-loop 161, serde unwrap 98, string-in-loop 239).

Cats never given a dedicated pass: 6 (0 hits), 9 (1 rustdoc `# todo!()`), 10 (wildcard/`pub use` info), 16 (`from_str` info), 17 (0), 24 (perf nits). None hid a new crash.

## Files changed (this pass)

| File | Why |
|---|---|
| `crates/zero-codemode/src/interpreter.rs` | `object_pattern` skipped `{ x }` shorthand (no `key`/`name` field). Bind the bare node. Add `shorthand_object_destructure_binds_the_property`. |

No rival dirty tracked files. Untracked `.ubs-loop/pass1_*` .. `pass9_*` left untouched. New scan dumps stay untracked except this summary (and `pass10.json` as the required artifact).

## Real -- fixed

1. **Shorthand object destructure never bound.** `bind` on `object_pattern` required `child_by_field_name("key"|"name")`. Tree-sitter's `{ x }` child is a bare `shorthand_property_identifier_pattern` with neither field, so `filter_map` dropped it. `const { x } = { x: 7 }; return x` evaluated as undefined (or a later lookup fault) while `{ x: y }` worked. Pass 3 noted this as a leftover bind gap and did not fix it. Same path serves `for (const { x } of …)` and function params via `bind_in`. Fix: treat the shorthand/identifier node as the key. Renamed `{ x: y }` still uses the pair fields.

## Critical / warning inventory (UBS, this scan)

| Count | Sev | Title | Verdict |
|---|---|---|---|
| 717 | critical | Secret/token compared with `==`/`!=` | **FP.** Same as pass 6. Protocol/ABI/enum compares, not HMAC. |
| 25 | critical | Non-crypto randomness for tokens | **FP / already-correct.** Cookie site is CSPRNG (pass 6). Rest are session labels / tests. |
| 18 | critical | JWT decode bypass | **FP.** Hits `fn decode`. No `jsonwebtoken`. |
| 5 | critical | Hardcoded secrets | **FP.** Env *names*, ABI strings, fixture bytes. |
| 4 | critical | Command from untrusted-looking value | **FP.** Fixtures / `CARGO_BIN_EXE_*` / validated worker spec. |
| 2 | critical | Shell `-c`/`-lc` | **FP.** testkit `python3 -c`. No `sh`/`bash` in crates. |
| 177 | critical | `panic!` macros | **FP / tests.** Golden-vector fail-loud. Lib `unreachable!` still the pass 1 invariants. |
| 6212 | warning | unwrap/expect | **FP / tests.** Lib leftovers are documented constructors or `ubs:ignore` (pass 1). |
| 2782 | warning | `assert!` | **FP.** Tests + testkit. |
| 457 | warning | Direct indexing | **FP.** Heuristic `[..]`. Inspected slices are proven or `Value` index. |
| 161 | warning | `clone()` in loops | Perf nit. |
| 239 | warning | String alloc in loop | **FP / tests.** Cat 24 inventory. |
| 98 | warning | serde unwrap | Tests / `include_str!` fixtures. |
| 34 | warning | `lock().unwrap()` | **FP.** Tests + `gc.rs` `#[cfg(test)]` hook. Lib poison is typed/`into_inner`. |
| 9 | warning | `len() as` narrow | **Already-correct.** Same 9 sites as pass 4 (256-resource cap, `[HANDLE; 2]`, `u32::MAX` guard). |
| 8 | warning | `unreachable!` | **Already-correct.** Pass 1 invariants + tests. |
| 6 | warning | `Rc<RefCell>` | Inventory after pass 3 aliasing fix. |
| 6 | warning | Predictable temp write | Permit `File::create` of own metadata; cookie files are OS-random. |
| 1 | warning | `parse().unwrap()` | `zero-gauge` `#[cfg(test)]`. |
| 1 | info | Technical debt marker | rustdoc `# todo!()` example in `adapter.rs`. |

## Already-correct (checked this scan, >=8)

1. `crates/zero-store/src/cas.rs` -- `put_prehashed` re-derives SHA-256; `get_verified_limited` enforces `CAS_MAX_OBJECT_BYTES`.
2. `crates/zero-store/src/session_wal.rs` -- `read_to_end` only after `meta.len() > remaining` stop; frames cap `SESSION_WAL_MAX_RECORD_BYTES`.
3. `crates/zero-process/src/random.rs` -- `buffer.len() > u32::MAX` rejected before the Windows `as u32` CSPRNG fill.
4. `crates/zero-gate/src/transaction.rs` -- `len() as u16` sits behind `TRANSACTION_MAX_RESOURCES_V1` (256).
5. `crates/zero-codemode/src/host.rs` -- `finalize_visible_error` / spill preview walk `is_char_boundary` before slicing.
6. `crates/zero-codemode/src/interpreter.rs` -- `JSON.parse` still rejects over `max_json_bytes` (pass 6). Cyclic `{ self: { x: y } }` still snapshots before recurse (pass 3).
7. `crates/zerostack-machine-permit/src/lib.rs` -- cookies via `fill_random`; `sanitize_permit_class` rejects `..` / separators; metadata truncate is UTF-8 safe.
8. `crates/zero-mcp/src/mcp_transport.rs` -- `tool_timeout` capped at `MAX_MCP_TOOL_TIMEOUT` (1h) before `Instant + timeout`. `to_string` expects are `Value` serialize.
9. `crates/zero-gauge/src/lib.rs` -- `Gauge::allocate` / `allocation` use `checked_add`/`checked_mul`; `capacity` is `NonZeroU64`.
10. `crates/zsx-node/src/session.rs` -- `timeoutMs` is `Option<u32>` (max ~49d), not an unbounded `Duration`.
11. `crates/zero-codemode/src/worker.rs` -- `checked_deadline` uses `Instant::checked_add` and rejects `as_millis() > i64::MAX`.
12. `crates/zero-abi/src/zbf.rs` / `raw_worker.rs` -- decode still has object/payload/`max_frame_bytes` caps.

## Leftover (not fixed)

- `{ x, ...rest }` / `{ x = 1 }` in `object_pattern` still `filter_map`-skip (no rest/default support). Fail-loud would be a language-surface change.
- Pass 9 leftover: `host_contract.rs` `invalid_connector_json_is_rejected` is still `.is_err()` only.
- 771 security-token FPs remain until the scanner stops treating ABI field names as HMAC secrets.

## Tests

| Filter | Result |
|---|---|
| `zero-codemode --lib shorthand_object_destructure_binds_the_property` | 1 passed |
| `zero-codemode --lib interpreter::tests::` | 18 passed |

## Suggested commit paths

```
crates/zero-codemode/src/interpreter.rs
.ubs-loop/pass10.json
.ubs-loop/pass10_summary.md
```

Do not add `pass10_findings.json` / `pass10_summary.json` unless a later pass wants them. Do not touch pass 1-9 artifacts or rival files.
