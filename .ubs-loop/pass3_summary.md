# UBS Pass 3 -- Rust categories 3 and 20

Concurrency & Async Pitfalls + Async Locking Across Await.

Focus: `.lock().unwrap()`, `.lock().expect()`, `Rc<RefCell>`, `thread::sleep` in async, `block_on` in async, spawn without join, Mutex held across `.await`.

## Commands

| Command | Exit |
|---|---|
| `rg` `.lock().(unwrap\|expect)`, `thread::sleep`, `block_on`, `Rc<RefCell>`, `thread::spawn`, `async fn`, `.await` under `crates/` | 0 |
| `ubs-rust.sh --only=3,20 --no-cargo --format=json --ci --exclude-tests crates/` | **0** -- critical=0 warning=26 info=10 |
| `ubs-rust.sh --only=3,20 --no-cargo --format=text --ci --exclude-tests crates/` | **0** -- same totals |
| `ubs --only=rust --skip-size-check --ci --files=<10 shortlisted>` | **1** -- wrapper runs all rust cats (1-24); 53/607/724 is cat-1 unwrap noise, not cat 3/20 |
| `ubs-rust.sh --only=3,20` on `interpreter.rs` after fix | **0** -- critical=0 warning=5 (remaining `Rc<RefCell>` inventory) |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-codemode --lib interpreter:: -- --test-threads=1` | **0** -- 16 passed |

Authoritative cat-3/20 artifact: `.ubs-loop/pass3_cat3.json` (crates-wide). Findings: `.ubs-loop/pass3_cat3_findings.json`. After: `.ubs-loop/pass3_cat3_rescanned.json`.

`--only=3` works on `ubs-rust.sh`. `--only=3,20` is the right pair for this mission. Wrapper `ubs --only=rust` without `--skip-rust=` is not cat-3-only.

No `async fn`, no `block_on`, no `tokio::sync` / `parking_lot`, no `tokio::spawn` in `crates/`. Cat 20 is clean.

## Files changed (this pass)

| File | Why |
|---|---|
| `crates/zero-codemode/src/interpreter.rs` | Drop `RefCell` borrows before recursive bind / before `defineProperty` mutation so aliased objects cannot panic. Add 3 regression tests. |

No rival dirty tracked files. Untracked `.ubs-loop/pass1_*` and `pass2_*` left untouched.

## Cat-3/20 triage (production src)

### Real -- fixed (root cause)

- `interpreter.rs` `bind` `object_pattern` / `array_pattern` -- held `borrow()` across recursive `bind`. `const { self: { x: y } } = obj` when `obj.self === obj` (or `arr.push(arr); const [[x]] = arr`) re-enters the same `Rc<RefCell>` and panics. Snapshot fields/items, drop the guard, then recurse.
- `interpreter.rs` `Object.defineProperty` -- held `target.borrow_mut()` while `descriptor.borrow()`. `Object.defineProperty(o, "x", o)` panics. Clone `get`/`value` out first, then mutate.

### FP / already correct -- left

- 20x `Mutex::lock().unwrap()/expect()` -- all tests (`zero-codemode/tests/worker_adapter.rs`, `zero-store/src/gc.rs` `#[cfg(test)]`). `--exclude-tests` does not strip the regex inventory. Tests should panic on poison.
- 6x `Rc<RefCell>` -- single-threaded CodeMode JS values + env + metrics. Inventory warning remains after the aliasing fix.
- 10x `Arc<Mutex>` -- info only; contention check, not a crash.
- `thread::sleep` -- sync poll/backoff only (no `async fn` in `crates/`).
- `block_on` -- none.
- Cat 20 lock-across-await -- none (no async fns, no tokio locks).

### Not fixed this pass (later / out of scope)

- Cats 1, 2, 4-19, 21-24 (no cat 1/2 redo).
- MCP cancel `drop(worker)` in `zero-mcp/src/mcp_transport.rs:593` -- already filed as a gauntlet bead. Intentional: dispatch ignored cancel; do not block the handler. Inflight permit stays in the worker until it finishes. Not a UBS cat 3/20 hit (no tokio, no async).
- `zero-codemode/src/worker.rs` stdin/stdout/stderr `thread::spawn` with dropped `JoinHandle` -- pipe-owned IO threads exit on EOF after `WorkerClient` kills the child. Cat 19 lifecycle, not a library crash.
- `ZsxSession::drop` sends shutdown and does not join; `shutdown()` joins. Non-blocking drop.
- Shorthand `{ x }` inside `object_pattern` still skips (no `key`/`name` field). Pre-existing bind gap, not cat 3. Cycle test uses `{ self: { x: y } }`.

## Already correct (checked, no change)

1. `zero-process` child/pipe and `zsx-core/src/tokenzero.rs` recover poison with `unwrap_or_else(|poisoned| poisoned.into_inner())` -- no library-path lock unwrap.
2. `zsx-core` connector/session and `zsx-node/src/core.rs` map poison to typed errors (`if let Ok` / `map_err`).
3. `interpreter.rs` `property()` clones the getter and drops the object borrow before `self.call`. `to_json_depth` inserts the Rc pointer into `active` before borrowing so cycles error instead of re-borrowing. Array `map`/`filter` snapshot before callbacks.
4. `ZsxConnector::drop` and `FsZeroAdapter::drop` join their worker threads. `MachinePermitHeartbeat` signals then joins.
5. `zerostack-machine-permit` `WAKE_CACHE` is thread-local `RefCell`; `new`/`Drop` do not nest borrows (NativeWake drop does not touch the cache).

## Tests

RCH admitted (`spark-1672`). Targeted:

```
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-codemode --lib interpreter:: -- --test-threads=1
```

16 passed (including 3 new: cyclic object destructure, cyclic array destructure, self-descriptor `defineProperty`).

## Suggested commit paths

```
crates/zero-codemode/src/interpreter.rs
.ubs-loop/pass3_summary.md
```

Do not add `.ubs-loop/pass3_cat3*.json*` (inventory dumps) unless a later pass wants them. Do not touch pass 1/2 artifacts or rival files.
