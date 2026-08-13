# UBS Pass 8 -- polyglot Python / JS / shell

`scripts/`, `bindings/`, `bench/` only. Prior rust passes 1-7 untouched.

Hunt: injection (`eval`, `os.system`, unsanitized paths), resource leaks, missing error checks. Test fixtures ignored.

## Commands

| Command | Exit |
|---|---|
| `ubs --only=python,js --format=json --ci --skip-size-check scripts bindings bench` | **2** -- wrapper rejects multiple roots (`file not found: bindings`) |
| same, `scripts` | **1** -- critical=8 warning=63 info=172 files=11 |
| same, `bindings` | **0** -- critical=0 warning=7 info=9 files=1 |
| same, `bench` | **1** -- critical=4 warning=2 info=155 files=3 |
| `ubs --only=python --format=json --ci --skip-size-check scripts/bench/senpi-zerostack` after fix | **1** -- Popen warning remains (interprocedural) |
| `python3 -m unittest scripts.bench.senpi-zerostack.test_run -v` | **0** -- 4 passed |

Authoritative combined artifact: `.ubs-loop/pass8_polyglot.json` (per-dir JSON merged; wrapper cannot take three roots). Per-dir dumps: `pass8_scripts.json`, `pass8_bindings.json`, `pass8_bench.json`. jsonl companions left untracked for samples.

`--ci` exits 1 whenever `critical >= 1`. Remaining criticals are public-hash `==` and one taint FP (see triage). No `--skip=N` (category numbers are not stable). No `--skip-python=` / `--skip-js=`.

No rch / workspace cargo. Python unittest only.

## Files changed (this pass)

| File | Why |
|---|---|
| `scripts/bench/senpi-zerostack/run.py` | Handshake / ready-frame failure after `Popen` leaked both children. `_reap_process` now kills+waits on the start path and on `main` teardown. |
| `scripts/bench/senpi-zerostack/test_run.py` | `test_handshake_failure_reaps_both_children`. |

No rival dirty tracked files. Untracked `.ubs-loop/pass1_*` .. `pass7_*` left untouched. New scan dumps stay untracked except this summary.

## Real -- fixed

- `start_arms` spawned `tsx` + `zerostack-host` then `read()` / protocol-check could raise. `main()` only assigned `arms` after return, so the `finally` reaper never ran. Local orphan processes. Now `start_arms` reaps unless handshake hands off.

## Critical / warning inventory (UBS)

| Count | Sev | Title | Verdict |
|---|---|---|---|
| 7 py + 4 js | critical | Secret/signature compared with `==`/`!=` | **FP.** Public SHA-256 / protocol / ABI / receipt checksums (`install_zerostack.py:212`, handshake `protocol_digest` / `semantic_contract_digest`, `validate.mjs` corpus/addon/sidecar). Not HMAC/API/CSRF secrets. Timing-safe compare does not apply. |
| 1 | critical | User input reaches subprocess | **FP.** `fszero-raw-worker/run.py:145` is `subprocess.run([str(binary), "capabilities", "--json"], ..., timeout=10)` -- list argv, `shell=False`. `binary` is the operator `--binary` path (resolved, must be a file). UBS taint `error -> receipt -> binary` is `gate_receipt` reading `receipt["binary"]` dict for a missing sha256, not executed. |
| 26 | warning | Division by variable | **FP.** `pathlib.Path /` join. Samples are `repo / rel`, `root / MANIFEST`. |
| 9+2 | warning | `is` with literals | **Already-correct.** `is True` / `is not False` reject truthy non-bools from JSON. |
| 1 | warning | `type(x) is not int` | **Already-correct.** `install_zerostack.py:204` rejects `bool` (`isinstance(True, int)`). |
| 2 | warning | Subprocess no timeout | **Leftover, not fixed.** `git ls-files` CI gate; `minisign -Vm ... -P` (no tty prompt). Not injection. |
| 4+8 | warning | `json.loads` without try | **FP / already-correct.** `install_zerostack.py:117,335` and `scrub_beads_export.py:117` wrap `JSONDecodeError`. Bench worker frames: outer `except` writes a failed receipt (law 7). |
| 6 | warning | `open()` missing `with` | **Already-correct.** `os.open` dirfd + `os.close` in `finally` (`install_zerostack`, `install_zs`); `tarfile.open` immediately `with package`. |
| 3 | warning | Popen never waited | **Partial FP.** Happy path: `RawWorkerClient.close()` / `JsonLineArm.shutdown()` / `main` finally. Real hole was `start_arms` failure (fixed). UBS still cannot see `_reap_process`. |
| 5 | warning | Switch missing break | **FP.** `bindings/node/loader.js` every `case` `return`s. |
| 2 | warning | `fs.*Sync` | **Already-correct.** Sync addon resolve at `require` time. No I/O loop. |
| 2 | warning | `JSON.parse` unguarded | **Already-correct.** `validate.mjs` is a CLI receipt gate; throw is the contract. |
| 2 | warning | Deprecated API | **FP.** No `imp` / `get_event_loop`. |

## Shell glance (`scripts/*.sh`, `scripts/zs`, `scripts/install_*.py`)

No `eval`, no `os.system`, no `shell=True`.

| File | Why already-correct |
|---|---|
| `scripts/install_zerostack.py` | List argv to minisign; launchers use `shlex.quote`; `safe_relative` + resolve-escape on archives; HTTPS-only `urlopen` with timeout and size cap. |
| `scripts/install_zs.py` | No subprocess. Atomic copy + dir `fsync`/`close`. |
| `scripts/zs` | `subprocess.run([bin], ..., timeout=...)`. Pin is an operator override, not a shell string. |
| `scripts/build-node-prebuild.sh` | `set -euo pipefail`; hardcoded cargo; no eval. |
| `scripts/check-portability.sh` | `set -uo pipefail`; `$gate` is a hardcoded pair of repo scripts. |
| `scripts/zerostack-cpu-watchdog.sh` | Allowlisted executables; identity fence before `kill`; `pid` from `ps`. No eval. |
| `bench/aggregate-codemode/bench.mjs` | `execFileSync("git", args)` list form. |

## Already-correct (sampled, >=5)

1. `install_zerostack.py` -- path-safe extract, signed-manifest verify, no shell interpolation.
2. `scripts/zs` -- list-form engine spawn, bounded timeout, fail-loud on missing binary.
3. `bindings/node/loader.js` -- exhaustive switch returns; missing addon fails with one install error.
4. `install_zerostack.py` / `scrub_beads_export.py` -- `json.loads` already fail-closed.
5. `fszero-raw-worker/run.py` -- `finally` always `client.close()`; `close()` kills leftover workers.
6. `senpi-zerostack/run.py` `main` finally -- reaps leftover arms after a measured run.
7. `check_feature_universe_weights.py` -- `tomllib` in try; `fail()` exits 1.
8. `check_no_literal_tilde_paths.py` -- `OSError` mapped to exit 2; no silent skip.
9. `validate.mjs` -- digest + sidecar mismatch throws.
10. `native-warm-read/bench.mjs` -- `parseInt(..., 10)`; `finally { await session.shutdown(); }`.

## Test fixtures -- ignored

- `scripts/bench/*/test_run.py` and the in-test fake worker that `json.loads` stdin. Not production.
- `scripts/bench/senpi-zerostack/senpi-driver.ts` -- TypeScript; `--only=js` does not scan TS.
- `scripts/dev/zdev-*` -- printed CodeMode plans, not executed shell.

## Leftover (not injection / leak / missing check)

- Unbounded `git ls-files` / `minisign` timeouts (CI/dev, not user-tainted).
- UBS still emits Popen-not-waited after the interprocedural reap.
- Public-hash `==` inventory will keep `--ci` at exit 1.

## Commit paths

`scripts/bench/senpi-zerostack/run.py`
`scripts/bench/senpi-zerostack/test_run.py`
`.ubs-loop/pass8_summary.md`
