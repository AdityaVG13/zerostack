# Worker loop: batching, compile tax, permit contention

Evidence (fleet 07-30): worker lanes spent 35-66 `zero_execute` calls and 14-21 min on
single beads. Three seams dominate. This doc is the fix, and `scripts/dev/zdev-bench-loop`
is the executable proof.

## 1. Batching: one plan does find + read + edit + verify

A plan is a JavaScript program, not a single tool call. Every `zero.fs.compound(...)` and
`zero.token.shell(...)` inside one plan costs zero extra `zero_execute` calls. The
anti-pattern is one micro-call per turn (search, then read, then edit, then test).

Canonical shape — the whole loop in one call:

```js
const t0 = Date.now();
const hit  = await zero.fs.compound('search', { query: 'fn permit_backoff', path: 'crates' });
const src  = await zero.token.shell("sed -n '1,40p' crates/x/src/lib.rs", {});
await zero.fs.compound('mutate', { path: 'crates/x/src/lib.rs', edits: [{ old: 'A', new: 'B' }] });
const test = await zero.token.shell('cargo test -p x --lib name_of_test', {});
await zero.token.shell('git checkout -- crates/x/src/lib.rs', {});   // scoped revert, never 'git checkout .'
return { hit, test, ms: Date.now() - t0 };
```

Rules that keep it to one call:
- Collect every read you can predict up front; `sed -n 'A,Bp;C,Dp'` beats two reads.
- Chain shell steps with `&&` / `;` inside a single `shell` action when they share a cwd.
- Return refs and small summaries, not whole files; the plan return value is your context.
- Only split into a second call when a later step's *content* depends on what you just read.
- Verify and revert in the same plan as the edit, so no turn ends mid-mutation.

## 2. Compile tax: never pay a full build inside the verify step

Repeated `cargo build`/`cargo test` invocations dominate wall time, and concurrent
workers editing one crate invalidate each other's incremental artifacts while cargo's
`target/` lock serializes their runs.

- **One writer per repo per wave.** Dispatch policy; a second writer in the same crate
  turns every verify into a cold rebuild.
- **Prewarm before the edit loop:** `cargo test -p <crate> --no-run` (or `cargo check`)
  once, then each verify is a link-and-run instead of a full compile.
- **Reuse the shared `target/`** for the single writer. Do *not* set a per-worker
  `CARGO_TARGET_DIR` for a lone writer: a fresh dir is a guaranteed cold build (minutes).
  Per-worker `CARGO_TARGET_DIR` is only correct when the policy is violated and two
  workers must touch the same crate concurrently — it trades disk for lock contention.
- **Scope every verify:** `-p <crate> --lib <test_name_filter>`. Never a full suite.
- Prefer `cargo check` when the change cannot alter runtime behaviour.

## 3. Permit contention

After `e2fea61` (fsync barriers dropped) the wake path in
`crates/zerostack-machine-permit/src/lib.rs` is event-driven: `acquire_slots_with_wake`
publishes a `WaiterIntent`, then blocks in `PermitWake::wait` on a native directory
event, and only the FIFO head retries immediately — younger waiters keep exponential
backoff (`permit_backoff`: 20/40/80/160/200 ms) so one event cannot cause an N-way scan
storm. Uncontended acquire is sub-ms. No remaining serialization was found in this crate.

The residual cost is scoping, not the crate: heavy-op permits must not gate trivial shell
or fs calls. That gate lives in the orchestrator (no consumer of `MachinePermit` exists in
this repo), so it is tracked separately; the worker-side mitigation is batching, which
collapses N permit acquisitions into one.
