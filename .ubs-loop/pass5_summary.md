# UBS Pass 5 -- Rust categories 7 and 19

Filesystem & Process + Resource Lifecycle Correlation.

Hunt: unclosed files, Command without wait, temp files left behind, leaked FDs, missing Drop, File::open without close, std::fs in hot paths that leak.

## Commands

| Command | Exit |
|---|---|
| `rg` `File::open`/`create`, `OpenOptions`, `Command::new`, `.spawn(`, `thread::spawn`, `impl Drop`, `into_raw_fd`, `libc::open/close/kqueue/pidfd`, `NamedTempFile`, `mem::forget` under `crates/` | 0 |
| `/opt/homebrew/bin/bash .../ubs-rust.sh --only=7,19 --no-cargo --format=json --ci --summary-json=.ubs-loop/pass5_cat7_summary.json --emit-findings-json=.ubs-loop/pass5_cat7_findings.json crates/ .ubs-loop/pass5_cat7.json` | **0** -- critical=0 warning=0 info=109 |
| same after fix (`.ubs-loop/pass5_cat7_rescanned.json`) | **0** -- critical=0 warning=0 info=110 |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-process --lib identity:: -- --test-threads=1` | **0** -- 4 passed |

Authoritative cat-7/19 artifact: `.ubs-loop/pass5_cat7.json` (crates-wide, before). Findings: `.ubs-loop/pass5_cat7_findings.json`. After: `.ubs-loop/pass5_cat7_rescanned.json`.

`--only=7,19` works on `ubs-rust.sh`. Wrapper `ubs --only=rust` is not cat-7/19-only (same as prior passes). `--no-cargo` as required; no workspace cargo.

Cat 7 is inventory only (`std::fs::` + fully-qualified `std::process::Command::new`). Cat 19 compares `std::thread::spawn` vs `.join(` counts per file -- it misses imported `thread::spawn`.

## Files changed (this pass)

| File | Why |
|---|---|
| `crates/zero-process/src/identity.rs` | Adopt pidfd/kqueue via `OwnedFd` so `is_live()?` cannot leak the watcher fd. Add `owner_watch_waits_for_short_lived_child`. |

No rival dirty tracked files. Untracked `.ubs-loop/pass1_*` / `pass2_*` / `pass3_*` / `pass4_*` left untouched. New scan dumps stay untracked except this summary.

## Cat-7/19 triage (production src)

### Real -- fixed (root cause)

- `identity.rs` `wait_for_exit` (Linux pidfd + macOS kqueue) opened a raw fd then used `id.is_live()?`. `is_live` calls `capture`, which returns `Err` on `/proc` I/O, short `proc_pidinfo`, or a missing boot id. That `?` skipped every manual `libc::close`. Windows already used `Handle` RAII. Unix now uses `OwnedFd` via `adopt_owned_fd`.

### Warning inventory (0)

UBS cat 7/19 emitted no warnings. Info only:

| Count | Category | Title | Verdict |
|---|---|---|---|
| 105 | Filesystem & Process | `std::fs::` present | Inventory. Almost all `read`/`write`/`create_dir_all`/`rename` -- no handle to leak. |
| 4 (5 after test) | Filesystem & Process | `std::process::Command::new` | 3 tests (`lib_inline_tests.rs`, `worker_adapter.rs`) + after-fix the new identity test. Production spawn is `Command::new` after `use` (`worker.rs:384`) -- UBS regex misses it. |

### FP / already correct -- left

- Cat 19 spawn/join imbalance -- none reported. Heuristic requires the `std::thread::spawn` prefix.
- `worker.rs` stdin/stdout/stderr `thread::spawn` with dropped `JoinHandle` -- pipe-owned IO threads exit on EOF after `WorkerClient` Drop kills/reaps the child. Intentional detach, not an FD leak.
- `ZsxSession::drop` sends shutdown and does not join; `shutdown()` joins. Non-blocking drop.
- MCP cancel `drop(worker)` in `mcp_transport.rs:593` -- already filed; dispatch ignored cancel. Inflight permit lives in the worker until it finishes.
- `zerostack-machine-permit` `.identity-*.tmp` / `.heartbeat-*.tmp` -- same cookie path, `File::create` truncates; `cleanup_owned` / `quarantine_exact` remove the permit dir. Not an accumulating leak.
- `mem::forget` / `ManuallyDrop` -- none in `crates/`.
- `TcpStream` / `tokio::spawn` -- none in `crates/`.
- CLI bins (`process_fixture`, `worker_fixture`, `program-evidence`) -- out of scope.

### Not present / out of scope

- Cats 1-6, 8-18, 20-24. No pass 1 unwrap redo, no pass 2 `zeroed`, no pass 3 `RefCell`, no pass 4 numeric.
- `scripts/` -- no rust this pass (polyglot is pass 8).
- `clone()`-in-loop / async std::fs (cat 3 inventory).

## Already correct (checked, no change)

1. `crates/zero-process/src/child.rs:874-925` -- `VerifiedChildInner::drop` SIGKILLs an unsettled Unix tree and bounded-reaps; Windows job is kill-on-close.
2. `crates/zero-process/src/child.rs:1326-1389` -- `escalate_detached` closes the pidfd on every path through `finish`.
3. `crates/zero-process/src/identity.rs` Windows `Handle` / `JobHandle` / `LocalBuffer` -- Drop closes exactly once; `OwnerWatcher::new` drops the handle on identity mismatch.
4. `crates/zerostack-machine-permit/src/lib.rs:1466-1710` -- `NativeWake` Drop closes inotify/kqueue/FindClose; constructor error paths close before return.
5. `crates/zero-store/src/cas.rs:404-408` and `metadata.rs:79-86` -- exclusive temp + `remove_file` after publish (success or fail). `reap_stale_temps` sweeps leftovers.
6. `crates/zero-store/src/fs_replace.rs:168-181` -- `atomic_write_file_with_sync` deletes the sibling temp when replace fails.
7. `crates/zero-store/src/{attempt,durable}_journal.rs` -- `published` flag; unpublished temps are removed.
8. `crates/zero-store/src/gc_lock.rs:169-173` -- `StoreLock::drop` unlocks; kernel also releases on close.
9. `crates/zero-codemode/src/worker.rs:1371-1381` -- `WorkerClient::drop` shutdowns then `kill_and_reap` / `revoke`. Partial spawn uses `cleanup_partial`.
10. `crates/zsx-core/src/{connector,fszero}.rs` and `MachinePermitHeartbeat` -- Drop joins dispatcher / session / heartbeat threads.
11. `crates/zero-process/src/random.rs:18` -- `File::open("/dev/urandom")` is statement-scoped RAII.

## Tests

RCH admitted (`spark-1672`, Linux pidfd path). Targeted:

```
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-process --lib identity:: -- --test-threads=1
```

4 passed (including new `owner_watch_waits_for_short_lived_child`).

## Suggested commit paths

```
crates/zero-process/src/identity.rs
.ubs-loop/pass5_summary.md
```

Do not add `.ubs-loop/pass5_cat7*.json*` (inventory dumps) unless a later pass wants them. Do not touch pass 1-4 artifacts or rival files.
