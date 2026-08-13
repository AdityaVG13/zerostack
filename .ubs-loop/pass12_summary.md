# UBS Pass 12 -- CONVERGENCE CONFIRMATION

Independent re-scan of the product surface after pass 11 ZERO-CHANGE (0 new genuine). Hunt: any remaining REAL UBS-class source bug that passes 1-11 missed. Do not stretch nits. Do not re-fix known FPs (771 token-compare, clone-in-loop, test unwraps).

Prior HEAD: `43731b0` (pass 11 ignore-audit, no source change). This pass also **ZERO-CHANGE**.

## Commands

| Command | Exit |
|---|---|
| `ubs-rust.sh --no-cargo --format=json --ci crates/zero-store .ubs-loop/pass12_zero_store.json` | **1** -- critical=83 warning=1608 info=326 files=12 |
| `ubs-rust.sh --no-cargo --format=json --ci crates/zero-process .ubs-loop/pass12_zero_process.json` | **1** -- critical=28 warning=217 info=431 files=9 |
| `ubs-rust.sh --no-cargo --format=json --ci crates/zero-codemode .ubs-loop/pass12_zero_codemode.json` | **1** -- critical=112 warning=946 info=422 files=13 |
| `ubs-rust.sh --no-cargo --format=json --ci crates/zsx-core .ubs-loop/pass12_zsx_core.json` | **1** -- critical=46 warning=936 info=334 files=16 |
| `ubs-rust.sh --no-cargo --format=json --ci crates/zsx-node .ubs-loop/pass12_zsx_node.json` | **0** -- critical=0 warning=3 info=13 files=7 |
| `ubs-rust.sh --no-cargo --format=json --ci crates/zerostack-machine-permit .ubs-loop/pass12_machine_permit.json` | **1** -- critical=22 warning=468 info=121 files=3 |
| same crates, `--exclude-tests --only=1,2,3,4,7,19,21,22,23` (skip 5 clone-in-loop + 8 token-`==`) | store **0**; process **1** (2 bin `panic!`); codemode **1** (1 fixture `panic!`); zsx-core **0**; zsx-node **0**; permit **1** (3 test `panic!`) |
| `ubs --only=python,js --format=json --ci --skip-size-check scripts/bench/senpi-zerostack` | **1** -- critical=3 warning=13 info=49 files=3 |
| `ubs --only=python,js --format=json --ci --skip-size-check bindings` | **0** -- critical=0 warning=7 info=9 files=1 |
| Independent `rg` leftover classes (`from_utf8_unchecked` / `transmute` / `forget` / `unwrap_unchecked` / `shell=True` / `os.system` / `eval(`) | **0** hits in crates + scripts + bindings + bench |

`--no-cargo` as required; no workspace cargo; no rch; no gauntlet workspace. Wrapper `ubs --only=rust` was not used (would run cargo). Findings sidecars quote-break on some samples (same as pass 6/7/10/11); counts taken from summary + title regex.

`--ci` exits 1 whenever `critical >= 1`. Remaining criticals are the known inventory (fixture/test `panic!` after `--exclude-tests`; all-cats still includes the 771 token-`==` class). See triage.

No rival dirty tracked files. Untracked `.ubs-loop/pass1_*` .. `pass11_*` left untouched.

## Files changed (this pass)

None. **ZERO-CHANGE.**

Only this summary is new writing. Scan dumps stay untracked.

## Real -- fixed

None.

## UBS inventory (this pass)

### Hot crates -- all cats (`--no-cargo`)

Same classes as pass 10 crates-wide inventory, split by crate. No new critical *class*.

| Crate | c / w / i | Remaining after `--exclude-tests --only=1,2,3,4,7,19,21,22,23` |
|---|---|---|
| `zero-store` | 83 / 1608 / 326 | c=0 w=69. Journal `expect` is the pass-1 invariant ("succeeded requires completion evidence"). |
| `zero-process` | 28 / 217 / 431 | c=2 -- `process_fixture` bin `panic!` (CLI contract), not lib. |
| `zero-codemode` | 112 / 946 / 422 | c=1 -- `worker_fixture.rs` `unknown engine` `panic!`. Lib `unreachable!` still the pass-1 `new_promise` invariant. |
| `zsx-core` | 46 / 936 / 334 | c=0. Fixture `unwrap` on `to_value` of owned structs. |
| `zsx-node` | 0 / 3 / 13 | c=0 w=0 on focused cats. 3 string-alloc warnings are request-id / serde, not a loop (pass 11). |
| `zerostack-machine-permit` | 22 / 468 / 121 | c=3 -- `lib_inline_tests.rs` `panic!`. Lib path is `try_*` (`ubs:ignore`). |

### Polyglot

| Count | Sev | Title | Verdict |
|---|---|---|---|
| 3 | critical | Secret/signature compared with `==`/`!=` (senpi) | **FP.** Same as pass 8. Public SHA-256 / protocol / semantic-contract digests, not HMAC. |
| 2 | warning | Popen never waited (`run.py:438,448`) | **FP / already-correct.** Interprocedural: `_reap_process` in `start_arms` `finally` unless handshake hands off (pass 8 fix still present). |
| 1 | warning | `json.loads` without try | **FP.** Bench worker frames: outer `except` writes a failed receipt. |
| 1 | warning | `open()` missing `with` | **FP.** Same class as pass 8 (`os.open` / already-closed). |
| 1 | warning | `is` with literals | **Already-correct.** Rejects truthy non-bools from JSON. |
| 5 | warning | Switch missing break (bindings) | **FP.** `loader.js` every `case` `return`s; `default` returns `null`. |
| 2 | warning | `fs.*Sync` (bindings) | **Already-correct.** Sync addon resolve at `require` time. No I/O loop. |
| 0 | -- | `eval` / `shell=True` / `os.system` / tainted sink | **Clean** on senpi + bindings. |

## Already-correct (checked this scan, >=10)

1. `crates/zero-store/src/session_wal.rs` -- `replay_segment` refuses `meta.len() > *remaining` before `read_to_end`. `read_frame` rejects `len > SESSION_WAL_MAX_RECORD_BYTES` and requires matching trailer; torn frames stop replay.
2. `crates/zero-store/src/cas.rs` -- `put_prehashed` re-derives SHA-256 and writes nothing on mismatch. `get_verified_limited` checks `symlink_metadata` + regular-file + `effective_limit = limit.min(CAS_MAX_OBJECT_BYTES)` before allocating.
3. `crates/zero-store/src/gc.rs` -- `read_gc_json` `take(GC_MAX_RECORD_BYTES + 1)` and refuses oversize before `from_slice`.
4. `crates/zero-store/src/store_root.rs` / `cas.rs` -- `.zerostack` marker and object fan-out use `symlink_metadata`; a symlink is refused, not followed.
5. `crates/zero-process/src/random.rs` -- `buffer.len() > u32::MAX` rejected before the Windows `as u32` CSPRNG fill. Unix is `/dev/urandom` `read_exact`.
6. `crates/zero-process/src/child.rs` -- `wait_for_exit` uses `Instant::now().checked_add(timeout).unwrap_or_else(Instant::now)` (no overflow panic). Worker Drop grace is `shutdown_timeout.min(100ms/250ms)`.
7. `crates/zero-codemode/src/interpreter.rs` -- `JSON.parse` still rejects `encoded.len() > max_json_bytes`. `Object.defineProperty` still clones `get`/`value` out before `borrow_mut`. `object_pattern` still snapshots fields, then binds shorthand via the bare node (pass 10).
8. `crates/zero-codemode/src/host.rs` / `zsx-core/src/connector.rs` -- execute timeout is `timeout.min(wall_timeout)`. Production `host_limits()` is `Duration::from_secs(30)`. `Instant::now() + timeout` cannot overflow on that budget.
9. `crates/zsx-core/src/graphzero.rs` -- `engine_context` remaining comes from connector `now_ms().saturating_add(context.remaining())` (the 30s host budget). Past deadlines are rejected by `deadline_expired` before `engine_context`. TokenZero uses `WallDeadline::from_elapsed_ms`, not `Instant + remaining`.
10. `crates/zsx-core/src/connector.rs` -- `wait_for_dispatch_idle` is a fixed `Duration::from_secs(5)` from session shutdown.
11. `crates/zsx-node/src/core.rs` -- `allocate_request_id` rejects `candidate > MAX_REQUEST_ID` (`u32::MAX as u64`) instead of wrapping. `timeoutMs` is `Option<u32>` (max ~49d); `DEFAULT_TIMEOUT_MS` is 30_000.
12. `crates/zerostack-machine-permit/src/lib.rs` -- `metadata_text` caps 1024 bytes and walks back to a UTF-8 boundary. Cookies still go through `fill_random` (pass 6).
13. `crates/zero-codemode/src/worker.rs` -- child env still strips `ZEROSTACK_SESSION_TOKEN` / `ZEROSTACK_SESSION_SHUTDOWN_TOKEN`. `checked_deadline` still rejects `as_millis() > i64::MAX` and uses `Instant::checked_add`.
14. `scripts/zs` -- `subprocess.run([spec["bin"]], ..., timeout=..., cwd=root)` list-form, no `shell=True`. `ZS_TIMEOUT_MS` must be a positive int or the CLI dies. `-C` is `resolve(strict=True)` + `is_dir()`.
15. `scripts/bench/senpi-zerostack/run.py` -- `start_arms` still reaps both children in `finally` unless handshake `handed_off`. `_reap_process` kill+wait.
16. `bindings/node/loader.js` -- no `eval`, no spawn, no download. Every `switch` arm `return`s. Missing addon is one install error. `ZSX_NATIVE_ADDON` is an operator path, `existsSync` then `require`.
17. Independent leftover hunt: no `from_utf8_unchecked` / `unwrap_unchecked` / `unreachable_unchecked` / `mem::transmute` / `mem::forget` / `get_unchecked` / `from_raw_parts` in `crates/`. No `shell=True` / `os.system` / `eval(` in `scripts/` `bindings/` `bench/`.

## Leftover (not fixed -- same as 1-11)

- `{ x, ...rest }` / `{ x = 1 }` in `object_pattern` still `filter_map`-skip (no rest/default support). Fail-loud would be a language-surface change.
- Pass 9 leftover: `host_contract.rs` `invalid_connector_json_is_rejected` is still `.is_err()` only.
- Already-aborted `AbortSignal` -- napi-rs 3.12 does not read JS `aborted` (pass 11). Binding-contract gap, not a library panic.
- MCP `drop(worker)` when dispatch ignores cancel -- inflight permit lives until the callback returns.
- 771 crates-wide security-token `==` FPs (pass 6/10).
- UBS Popen warning on senpi -- scanner cannot see `_reap_process`.
- `HostLimits.wall_timeout` has no *upper* bound (only nonzero). Production `host_limits()` is 30s. `Duration::MAX` Instant overflow is operator-misconfig, not a product crash.

## Tests

None. No production behavior change.

## Suggested commit paths

```
.ubs-loop/pass12_summary.md
```

Do not add `pass12_*.json` dumps unless a later pass wants them. Do not touch pass 1-11 artifacts or rival files.

## Convergence

| | |
|---|---|
| New genuine | **0** |
| Already-correct (this scan) | **17** listed |
| Source files changed | **0** |
| HEAD at start | `43731b0` |
| CONVERGED | **YES** |
