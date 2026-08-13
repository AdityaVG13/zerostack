# UBS Pass 2 -- Rust category 2 (Unsafe & Memory Operations)

`unsafe` / `transmute` / `assume_init` / `zeroed` / `forget` / `get_unchecked` / `from_raw_parts` / `from_utf8_unchecked` / unsafe Send/Sync / CStr unchecked in production `crates/`.

## Commands

| Command | Exit |
|---|---|
| `rg` shortlist of `unsafe` / `transmute` / `assume_init` / `zeroed` / `get_unchecked` / `from_raw_parts` in `crates/` | 0 |
| `ubs --only=rust ... crates/zero-process crates/zerostack-machine-permit crates/zsx-node crates/zsx-core` | **2** -- wrapper treats extra crate dirs as files (`file not found: crates/zerostack-machine-permit`) |
| `ubs --only=rust --skip-rust=1,3..24 --format=json --ci --skip-size-check crates/` | 1 -- cargo/clippy still runs; not cat-2 |
| `ubs-rust.sh --only=2 --no-cargo --format=json crates/` (before) | 1 -- critical=16 warning=6 info=384 |
| `ubs-rust.sh --only=2 --no-cargo --format=json crates/` (after) | **0** -- critical=0 warning=0 info=368 |
| `ubs-rust.sh --only=2 --no-cargo` on every `forbid(unsafe_code)` crate | **0** -- 0/0/0 |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-process -- --test-threads=1` | **0** -- 6 unit + 26 child tests |

Authoritative cat-2 artifacts: `.ubs-loop/pass2_cat2.json`, `.ubs-loop/pass2_cat2_findings.json` (before), `.ubs-loop/pass2_cat2_rescanned.json` (after).

No `transmute`, `mem::uninitialized`, `mem::forget`, `get_unchecked`, `from_utf8_unchecked`, `from_raw_parts`, or `CStr::from_bytes_with_nul_unchecked` in production crates.

## Files changed (this pass)

| File | Why |
|---|---|
| `crates/zero-process/src/identity.rs` | Replace 8 `FILETIME` `mem::zeroed` with a safe constructor. Replace `kevent` `zeroed` with a named literal. Document `assume_init` after a full `proc_pidinfo` write. Add missing `SAFETY` on geteuid/getsockopt/getpeereid/wait/pidfd/kqueue. Document `Handle` Send/Sync. |
| `crates/zero-process/src/pipe.rs` | Replace 3 `OVERLAPPED` `mem::zeroed` with a field constructor. Document `LocalBuffer` Send/Sync. |
| `crates/zero-process/src/child.rs` | `JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default()` instead of `zeroed`. `THREADENTRY32` field constructor. Keep `siginfo_t` `zeroed` (C union out-param) with ignore. Add missing `SAFETY` on owned-child kill + pidfd_send_signal. |
| `crates/zero-process/src/resource.rs` | Add missing `SAFETY` on the Darwin `pre_exec` setrlimit closure. |

Do not commit `.ubs-loop/` artifacts unless a later pass wants them.

**Rival dirty file (not this pass):** `crates/zsx-node/src/envelope.rs` appeared mid-pass -- 2-line change that lifts `error` serialization out of the `Ok` branch. Do not include it in a pass-2 commit.

## Cat-2 triage (production src)

### Real -- fixed (root cause)

- `crates/zero-process/src/identity.rs:266-269,479-482` (pre-fix) -- `FILETIME` is two `u32`s; `empty_filetime()` is a safe all-zero constructor (`identity.rs:511`).
- `crates/zero-process/src/identity.rs:350` (pre-fix) -- `kevent` output buffer is now a named struct literal, matching the changelist already built a few lines above.
- `crates/zero-process/src/pipe.rs:254,520,585` (pre-fix) -- `OVERLAPPED` is integer/handle POD + a documented union overlay; `overlapped_with_event()` (`pipe.rs:146`) builds it without `zeroed`.
- `crates/zero-process/src/child.rs:1032` (pre-fix) -- `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` has `Default`; now `::default()` (`child.rs:1033`).
- `crates/zero-process/src/child.rs:1429` (pre-fix) -- `THREADENTRY32` is seven integer fields; now a field constructor (`child.rs:1438`).
- `crates/zero-process/src/identity.rs:228` (pre-fix, now `:239`) -- `assume_init` after `proc_pidinfo` returns `sizeof(proc_bsdinfo)`. Init was already proven; the gap was a missing `SAFETY` comment.
- Missing `SAFETY` on production FFI: `identity.rs` geteuid/getsockopt/getpeereid/WaitForSingleObject/pidfd/kqueue/close, `child.rs` SIGTERM/SIGKILL + `pidfd_send_signal`, `resource.rs` Darwin `pre_exec`, `Handle`/`LocalBuffer` Send/Sync.

### FP / safe FFI -- ignored or left

- `crates/zero-process/src/child.rs:1247` `siginfo_t = mem::zeroed()` -- C union out-param for `waitid`. Cannot be a safe literal. `ubs:ignore` + `SAFETY` on the `si_pid()` read.
- `crates/zero-process/src/identity.rs:239` `assume_init` -- proven full write; `ubs:ignore`.
- `crates/zero-process/src/child.rs:1079,1081` `JobHandle` Send/Sync -- unique RAII job HANDLE; already had a SAFETY block; added ignore.
- `crates/zero-process/src/identity.rs:503,505` `Handle` Send/Sync -- unique RAII process HANDLE.
- `crates/zero-process/src/pipe.rs:141,142` `LocalBuffer` Send/Sync -- unique LocalAlloc owner.
- Remaining `unsafe { }` blocks (info=368) -- Windows/Unix FFI wrappers that already had `// SAFETY:` (or gained one this pass).

### Not present / out of scope

- Cats 1, 3-24
- Test-only `unsafe` in `zero-process/tests/{child,windows}.rs`
- `zerostack-machine-permit/src/lib_inline_tests.rs` "unsafe" is a directory name, not `unsafe` code

## Already correct (checked, no change)

1. `crates/zerostack-machine-permit/src/lib.rs:181` `geteuid` and `:1464-1783` inotify/kqueue/FindNextChangeNotification/`kill(pid, 0)` -- every block already has `// SAFETY:`.
2. `crates/zsx-node/src/tasks.rs:37-40` `ToNapiValue` -- delegates to napi serde_json; already has `// SAFETY:`. Crate docs note hand-written code is otherwise safe.
3. `crates/zero-process/src/random.rs:22-30` `BCryptGenRandom` -- already has `// SAFETY:`; system-preferred RNG, null algorithm handle.
4. `crates/zero-process/src/pipe.rs` token/SID/LocalAlloc/ConnectNamedPipe/ReadFile/WriteFile -- already documented FFI wrappers.
5. Every `#![forbid(unsafe_code)]` crate: `zero-abi`, `zero-store`, `zero-gate`, `zero-ledger`, `zero-codemode`, `zero-cert`, `zero-ref`, `zero-mcp`, `zero-gauge`, `zero-testkit`, `zsx-core`, `zsx` -- cat-2 0/0/0.

## forbid(unsafe_code) verification

| Crate | files | crit | warn | info |
|---|---|---|---|---|
| zsx-core | 16 | 0 | 0 | 0 |
| zero-abi | 18 | 0 | 0 | 0 |
| zero-store | 12 | 0 | 0 | 0 |
| zero-gate | 23 | 0 | 0 | 0 |
| zero-ledger | 7 | 0 | 0 | 0 |
| zero-codemode | 13 | 0 | 0 | 0 |
| zero-cert | 7 | 0 | 0 | 0 |
| zero-ref | 5 | 0 | 0 | 0 |
| zero-mcp | 2 | 0 | 0 | 0 |
| zero-gauge | 1 | 0 | 0 | 0 |
| zero-testkit | 17 | 0 | 0 | 0 |
| zsx | 3 | 0 | 0 | 0 |

## Tests

`rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-process -- --test-threads=1`

- lib unit: 6 passed (identity + resource)
- tests/child.rs: 26 passed
- tests/windows.rs: 0 run (not Windows)
- exit 0

## Commit paths

```
crates/zero-process/src/child.rs
crates/zero-process/src/identity.rs
crates/zero-process/src/pipe.rs
crates/zero-process/src/resource.rs
```

Optional artifacts (untracked, same as pass 1): `.ubs-loop/pass2_*`.
