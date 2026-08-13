# UBS Pass 11 -- Hostile re-triage + ignore audit

Hunt: leftover real bugs in `zsx-node` (historically 0 tests), `zero-mcp` cancel/detach, bare `ubs:ignore`, `.ubsignore` hiding product source. `scripts/zs` only if a NEW UBS-class bug (MCP/plan already known).

Prior: 1-10 through `8266947` (shorthand destructure). No re-fix of those.

## Commands

| Command | Exit |
|---|---|
| `rg -n 'ubs:ignore' crates scripts bindings bench` | **0** -- 18 hits, every one has a why |
| `rg -n 'ubs:ignore\s*$'` (same roots) | **1** -- no matches (no bare ignores) |
| `ubs-rust.sh --no-cargo --format=json --ci crates/zsx-node .ubs-loop/pass11_zsx_node.json` | **0** -- critical=0 warning=3 info=13 files=7 |
| `ubs-rust.sh --no-cargo --format=json --ci crates/zero-mcp .ubs-loop/pass11_zero_mcp.json` | **1** -- critical=5 warning=149 info=89 files=2 (`--ci` on test `panic!`) |
| `ubs-rust.sh --no-cargo --format=text --ci --only=8,1,2,3,7,19,21 crates/zero-mcp` | **1** -- same 5 test `panic!`; cat 8 security 0 hits |

`--no-cargo` as required; no workspace cargo; no rch. Wrapper `ubs --only=rust` was not used (would run cargo). No gauntlet workspace.

Authoritative per-crate dumps (untracked, like prior passes): `.ubs-loop/pass11_zsx_node.json`, `.ubs-loop/pass11_zero_mcp.json`. Findings sidecars quote-break on some samples (same as pass 6/7); counts taken from `summary`.

No rival dirty tracked files. Untracked `.ubs-loop/pass1_*` .. `pass10_*` left untouched.

## Files changed (this pass)

None. **ZERO-CHANGE.**

Only this summary is new writing. Scan dumps stay untracked.

## Ignore audit

### `ubs:ignore` -- 18, all justified

| File | Count | Why present |
|---|---|---|
| `crates/zsx-core/src/fszero.rs` | 3 | AdapterBinding constants + documented-infallible constructor |
| `crates/zero-gate/src/two_phase.rs` | 6 | `BrokeredExecution::expect` matcher, not `Result::expect` |
| `crates/zerostack-machine-permit/src/lib.rs` | 2 | panicking `scoped_permit_base*`; lib path is `try_*` |
| `crates/zero-process/src/child.rs` | 3 | `JobHandle` Send/Sync + `siginfo_t` zeroed out-param |
| `crates/zero-process/src/identity.rs` | 3 | `assume_init` after full `proc_pidinfo` + `Handle` Send/Sync |
| `crates/zero-process/src/pipe.rs` | 2 | `LocalBuffer` Send/Sync |

No hit in `scripts/`, `bindings/`, `bench/`. No `ubs:ignore` with empty reason.

### `.ubsignore` -- does not hide `crates/` product source

Patterns are caches / local engines / archives: `target/`, `.zerostack/`, `.fszero/`, `.tokenzero/`, `.graphzero/`, `.asgrep/`, `.pi-subagents/`, `archive/`, `videos/`, `blobs/`, `.ruff_cache/`, `.unsafe-audit/`, `.ub-exorcism/`, `.jeffreyskills/`, `node_modules/`.

Under `crates/` the only basename collisions are empty engine dirs (`crates/zero-codemode/.fszero`, `.tokenzero`, `crates/zero-ledger/src/.fszero`, `crates/zero-gate/src/.fszero`) -- 0 `.rs` / `.py`. `archive/` is repo-root pruned history, not live product.

## Real -- fixed

None.

## UBS inventory (this pass)

### `crates/zsx-node` (exit 0)

| Count | Sev | Title | Verdict |
|---|---|---|---|
| 0 | critical | unwrap / panic / security | **Clean.** No `unwrap`/`expect`/`panic!` in the crate. |
| 3 | warning | String alloc in loop | **FP.** `format!` on request-id exhaustion, `to_string` on reconcile reason / serde error. Not a loop. |
| 1 | info | `unsafe` block | **Already-correct.** `JsEnvelope::to_napi_value` delegates to napi serde_json (pass 2 SAFETY). |
| 7 | info | `as` casts | **Already-correct.** `timeoutMs: Option<u32>`; status `generation`/`inflight`/`aborted` are documented u32 snapshots. |
| 2 | info | `clone()` | Envelope detail / result copy. |

### `crates/zero-mcp` (exit 1)

| Count | Sev | Title | Verdict |
|---|---|---|---|
| 5 | critical | `panic!` | **FP / tests.** All five sit in `#[cfg(test)]` (`callback did not observe timeout`, FastMCP content-shape). |
| 79 | warning | unwrap/expect | **FP / tests** except 3 lib `serde_json::to_string(Value).expect` (pass 7: `Value` serialize cannot fail). |
| 48 | warning | `assert!` | **FP.** Tests only. |
| 5 | warning | Direct indexing | **FP.** `alias_metadata[..index]` is the prefix already walked; test `content[0]` after `len` checks. |
| 6 | warning | serde unwrap | Tests. |
| 4 | warning | `clone()` in loops | Perf nit on FastMCP `definition()` / catalog. |
| 0 | -- | Cat 8 security | No token-`==`, no shell `-c`, no Command, no hardcoded secrets. |

`drop(worker)` on ignored cancel (`mcp_transport.rs:593`) is still the documented detach: permit stays in the worker until dispatch returns; `max_inflight` back-pressures. Gauntlet bead, not a new UBS crash.

## Already-correct (checked this scan, >=8)

1. All 18 `ubs:ignore` comments carry a why (AdapterBinding, `BrokeredExecution::expect`, `try_*` wrappers, FFI Send/Sync / `siginfo_t` / `assume_init`).
2. `.ubsignore` does not match any `crates/**/*.rs`. The four `.fszero`/`.tokenzero` dirs under crates are empty engine roots.
3. `crates/zsx-node` -- no unwrap/expect/panic in lib or build.rs. Mutex poison maps to typed `ZsxSessionFailureCode::Internal`.
4. `crates/zsx-node/src/session.rs` -- `timeoutMs` is `Option<u32>` (max ~49d). `DEFAULT_TIMEOUT_MS` is 30_000 and fits the `as u32` fallback.
5. `crates/zsx-node/src/core.rs` -- `allocate_request_id` rejects `candidate > MAX_REQUEST_ID` (`u32::MAX as u64`) instead of wrapping.
6. `crates/zsx-node` inflight pairing -- `begin_request` on the JS thread, `finish_request` in `Task::finally`. napi-rs `complete_impl` (`async_work.rs:179`) calls `finally` on both success and `napi_cancelled` (abort-before-start still decrements).
7. `crates/zsx-node/src/core.rs` `initialize` -- lock held across `build_canonical`; second `is_terminated` check before store. `shutdown` waits on the same lock, so a concurrent shutdown either sees no session or shuts the stored one.
8. `crates/zsx-core/src/session.rs` `replace` still calls `cancel_backend` (active token) before the Replace command. The addon comment that reconcile cancels in-flight is true at the core layer.
9. `crates/zero-mcp/src/mcp_transport.rs` -- `tool_timeout` validated (`<= MAX_MCP_TOOL_TIMEOUT`, 1h) before `Instant::now() + timeout`. `elapsed >= tool_timeout` is checked before `tool_timeout - elapsed`.
10. `crates/zero-mcp` Inflight -- `fetch_update` admits only under `maximum`; spawn-fail drops the permit; worker owns the guard; cancel detach keeps the slot occupied until dispatch returns.
11. `crates/zero-mcp` lib `expect` sites (lines 329, 691, 836) are `serde_json::to_string` on `Value` / owned `McpDispatchError`.
12. `scripts/zs` -- still list-form `subprocess.run([bin], ..., timeout=...)`, no `eval` / `shell=True`. MCP/plan catalog fallback is the known gauntlet behavior, not a new injection/leak.

## Leftover (not fixed)

- **Already-aborted `AbortSignal`.** napi-rs 3.12 `FromNapiValue` registers `onabort` and does not read JS `aborted`. An already-aborted signal will not fire, so `ExecuteTask.cancelled` stays false and compute runs. Typical host path is abort-after-start (works). Fix needs Env + the JS object (`AbortSignal` wrapper has no getter). Not a library panic; left as a binding-contract gap.
- MCP `drop(worker)` when dispatch ignores cancel -- same as passes 3/5/7. Inflight permit lives until the callback returns.
- Pass 10 leftover: `{ x, ...rest }` / `{ x = 1 }` still skipped in `object_pattern`.
- Pass 9 leftover: `host_contract.rs` `invalid_connector_json_is_rejected` is still `.is_err()` only.
- 771 crates-wide security-token `==` FPs (pass 6/10) -- not in `zsx-node` / `zero-mcp`.

## Tests

None. No production behavior change.

## Suggested commit paths

```
.ubs-loop/pass11_summary.md
```

Do not add `pass11_zsx_node*.json` / `pass11_zero_mcp*.json` unless a later pass wants them. Do not touch pass 1-10 artifacts or rival files.
