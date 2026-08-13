# UBS Pass 6 -- Rust categories 8 and 23

Security Findings + Parsing & Validation Robustness.

Hunt: command injection, path traversal (`..`), untrusted parse, digest computed then ignored, eval/shell, TOCTOU, missing size caps on decode, `from_utf8_unchecked`, secret literals.

## Commands

| Command | Exit |
|---|---|
| `rg` `Command::new`, `.arg("-c"/-lc)`, `from_utf8_unchecked`, `include_str!`/`include_bytes!`, secret-shaped literals, `env::var(...).unwrap`, `ParentDir` / `canonicalize`, digest/`sha256` under `crates/` | 0 |
| `/opt/homebrew/bin/bash .../ubs-rust.sh --only=8,23 --no-cargo --format=json --ci --summary-json=.ubs-loop/pass6_sec_summary.json --emit-findings-json=.ubs-loop/pass6_sec_findings.json crates/ .ubs-loop/pass6_sec.json` | **1** -- critical=773 warning=122 info=1 |
| same after fix (`.ubs-loop/pass6_sec_rescanned.json`) | **1** -- critical=771 warning=122 info=1 |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-machine-permit --lib owner_cookie -- --test-threads=1` | **0** -- 1 passed |
| `rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack bash -lc 'cargo test -p zero-machine-permit --lib -- --test-threads=1 cookie && cargo test -p zero-codemode --lib interpreter::tests::json_parse -- --test-threads=1'` | **0** -- 4 cookie + 1 json_parse passed |

Authoritative cat-8/23 artifact: `.ubs-loop/pass6_sec.json` (crates-wide, before). Findings: `.ubs-loop/pass6_sec_findings.json`. After: `.ubs-loop/pass6_sec_rescanned.json`.

`--only=8,23` works on `ubs-rust.sh`. Wrapper `ubs --only=rust` is not cat-8/23-only (same as prior passes). `--no-cargo` as required; no workspace cargo.

UBS `--ci` exits 1 whenever `critical >= 1`. The remaining 771 "critical" after the fix are inventory FPs (see triage). Rescan dropped 2 criticals: the `RandomState` cookie hasher is gone.

## Files changed (this pass)

| File | Why |
|---|---|
| `crates/zerostack-machine-permit/src/lib.rs` | Permit/waiter cookies from OS CSPRNG (`zero_process::fill_random`). Mint failure is Fatal, not a weak fallback. Quarantine refuses to reclaim if entropy is unavailable. |
| `crates/zerostack-machine-permit/src/lib_inline_tests.rs` | Call sites handle `io::Result`. Add `owner_cookie_is_os_entropy_hex`. |
| `crates/zero-codemode/src/interpreter.rs` | `JSON.parse` now rejects input above `max_json_bytes` before `serde_json::from_str`. Add `json_parse_rejects_input_over_max_json_bytes`. |

No rival dirty tracked files. Untracked `.ubs-loop/pass1_*` .. `pass5_*` left untouched. New scan dumps stay untracked except this summary.

## Cat-8/23 triage (production src)

### Real -- fixed (root cause)

- `owner_cookie` hashed `pid` + wall time + a sequence through `RandomState` (SipHash, not a CSPRNG). The 32-hex cookie is the permit ownership fence. A local peer who can write the permit dir and guess the cookie can heartbeat/reclaim another holder. Now 16 OS-random bytes, hex-encoded. `/dev/urandom` / `BCryptGenRandom` failure is `TryPermit::Fatal` / `AcquireError::Fatal` (law 7). Quarantine is fail-closed (`return false`) if entropy is unavailable.
- Interpreter `JSON.parse` decoded unbounded strings. Connector results and `zero.*` args already honor `max_json_bytes` (default 1 MiB); `String.repeat` is only bounded by `memory_bytes` (default 64 MiB). A plan can `JSON.parse('"'+'x'.repeat(N)+'"')` and bypass the JSON budget. Decode now fails loud when the input exceeds `max_json_bytes`.

### Warning / critical inventory (UBS)

| Count | Severity | Title | Verdict |
|---|---|---|---|
| 717 (before) / ~715 | critical | Secret/token compared with `==`/`!=` | FP. Heuristic treats every `token` / protocol-field compare as timing-unsafe crypto. Samples: `mode == "ignore-cancel"`, `engine == GraphZero`, `protocol_version !=`. LLM tokens and ABI enums, not HMACs. |
| 27 | critical | Non-crypto randomness for security tokens | Mostly FP. `default_session_id` (`pid` + timestamp + sequence) is a local correlation label, not an auth secret. Tests use `process::id()`. The one real cookie site is fixed. |
| 18 | critical | JWT decode/validation bypass | FP. Hits every `fn decode` / `encode_decode` (ABI dispatch, process binding). No `jsonwebtoken`. |
| 5 | critical | Possible hardcoded secrets | FP. Env *names* (`ZEROSTACK_SESSION_TOKEN`), ABI version strings, test fixture bytes. No `sk-` / `ghp_` / PEM keys. |
| 4 | critical | Command executable from untrusted-looking value | FP. Tests/fixtures (`test_binary()`, `CARGO_BIN_EXE_*`, `cc`/`python3` in testkit). Production `Command::new(&spec.program)` is the registered worker binary after `validate_spec`. |
| 2 | critical | Shell `-c`/`-lc` | FP. `assembly_kat.rs` `python3 -c "import sys; print(sys.executable)"` (testkit). No `Command::new("sh")` / `bash` in crates. |
| 11 | warning | Path join/push with untrusted-looking segment | FP. Tests (`aggregate_world` seed paths). Production joins use numeric `g{generation}/r{request_id}/{seq}` or validated hex hashes. |
| 6 | warning | Predictable temp-file write race | Inventory. Permit `.identity-{cookie}.tmp` / `.heartbeat-{cookie}.tmp` then `rename`. Cookie is now OS-random. `create_new` already used in CAS / journals / program-evidence. |
| 98 | warning | serde/toml unwrap | Tests / `include_str!` fixtures / testkit vectors. Not lib parse of untrusted bytes. |
| 6 | warning | `env::var().unwrap()` | `worker_fixture` bin only. Process is spawned with those env vars set. |
| 1 | warning | `parse().unwrap()` | `zero-gauge` `#[cfg(test)]` grammar fixture. |
| 1 | info | Plain `http://` | Inventory. |

### FP / already correct -- left

- `sanitize_permit_class` remaps `..` / `/` / empty to `"invalid"` (tested). Containment, not escape. Silent remap is the documented contract; all bad classes share one basename on purpose.
- `read_attempt_manifest` `.ok()` skips unreadable journals during status listing. Incomplete dirs are expected.
- `FsZeroAdapter::new` `canonicalize().unwrap_or(root)` is an infallible constructor; `ZsxExecutor` already fails loud on canonicalize before adapters see the root.
- `classify_ref` accepts `..` as `RefKind::Path` -- syntactic classifier only; resolution stays in FSZero.
- `zero.token.shell` is TokenZero's surface (help text + tests). Hub does not spawn a shell.
- `scripts/` -- no rust this pass (polyglot is pass 8).

### Not present / out of scope

- Cats 1-7, 9-22, 24. No pass 1 unwrap redo, no pass 5 FD redo.
- `from_utf8_unchecked` / `from_str_unchecked` -- none in `crates/`.
- Hardcoded PEM / `sk-` / `ghp_` / `AKIA` literals -- none.
- `include_str!` of secrets -- fixtures and schema bytes only.
- JWT / CORS / open-redirect / Host-header / SQL / request-regex -- no HTTP server in hub crates.

## Already correct (checked, no change)

1. `crates/zero-store/src/cas.rs` -- `put_prehashed` re-derives SHA-256 and writes nothing on mismatch. `get_verified_limited` enforces `CAS_MAX_OBJECT_BYTES`.
2. `crates/zero-abi/src/zbf.rs` -- decode checks object/payload size, assembly digest, and payload digest before accepting bytes.
3. `crates/zero-abi/src/raw_worker.rs` -- `decode_*_frame` rejects over `max_frame_bytes`. `validate_handshake_request` compares engine, contract digest, optional registry digest, and revision.
4. `crates/zero-codemode/src/worker.rs` -- handshake compares `protocol_digest` to `raw_worker_protocol_digest_hex()`. `SESSION_TOKEN` / `SESSION_SHUTDOWN_TOKEN` are stripped from the child env.
5. `crates/zero-ref/src/lib.rs` -- `verify_and_select` checks content hash; `select` is deprecated. `unchecked_select` is documented as post-auth only.
6. `crates/zero-codemode/src/wrap.rs` -- plans over `max_plan_bytes` or containing NUL fail before the interpreter.
7. `crates/zero-store/src/store_root.rs` -- `validate_engine_file_name` requires one `Component::Normal` and rejects `/`, `\`, `..`.
8. `crates/zerostack-machine-permit/src/lib.rs` -- `sanitize_permit_class` rejects `""`, `"."`, `".."`, separators, and any byte outside `[A-Za-z0-9._-]`.
9. `crates/zsx-core/src/session.rs` -- session root / state root `canonicalize` errors instead of using a non-canonical path.
10. `crates/zero-process/src/random.rs` -- existing OS CSPRNG helper; permit cookies now use it.
11. `crates/zero-abi/src/{assembly,effect,cwir,reasoning,job}.rs` -- wire decode has explicit `MAX_*_BYTES` before `serde_json::from_slice`.
12. `crates/zero-store/src/session_wal.rs` -- frame decode caps `SESSION_WAL_MAX_RECORD_BYTES` and treats oversize as torn, not a panic.

## Tests

RCH admitted (`spark-1672` for the first cargo test; the combined cookie+json run executed locally after rch classified it as non-compilation). Targeted:

```
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack cargo test -p zero-machine-permit --lib owner_cookie -- --test-threads=1
```

1 passed (`owner_cookie_is_os_entropy_hex`).

```
rch exec -- env CARGO_TARGET_DIR=/tmp/rch_target_zerostack bash -lc 'cargo test -p zero-machine-permit --lib -- --test-threads=1 cookie && cargo test -p zero-codemode --lib interpreter::tests::json_parse -- --test-threads=1'
```

4 cookie tests + `json_parse_rejects_input_over_max_json_bytes` passed.

## Suggested commit paths

```
crates/zerostack-machine-permit/src/lib.rs
crates/zerostack-machine-permit/src/lib_inline_tests.rs
crates/zero-codemode/src/interpreter.rs
.ubs-loop/pass6_summary.md
```

Do not add `.ubs-loop/pass6_sec*.json*` (inventory dumps) unless a later pass wants them. Do not touch pass 1-5 artifacts or rival files.
