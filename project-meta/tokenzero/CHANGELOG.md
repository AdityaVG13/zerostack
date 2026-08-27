# Changelog

All notable changes to TokenZero will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

Tracked tip on `main`: `68826cb` (post-`v1.4.0`). Includes permit/CodeMode batch, expand-arg coercion, session-resume CPU harden (`d3bc9bd`), and merged PR #26 telemetry (`tokenzero-f409`). Append here until the next tagged release; do not fold into `[1.4.0]`. Follow-up PR drains remaining ready beads; `tokenzero-readme-northstar-rebaseline-9c6c` refreshed README/northstar numbers (snapshot `20260717T014139.658900Z-ab0b3ca3090a`).

### Fixed (2026-07-16 zerostack incident follow-ups)
- **Working-set budgets are best-effort at the marker floor**: `a7d53ad` made `admit` hard-error (`BudgetUnsatisfiable`) whenever eviction could not compress below the ~40-token marker floor, rolling back the admission and stranding full inline text at the caller; sub-floor budgets now serve at the floor (the token-unit victim comparison from `tokenzero-g3y.19` stays).
- **Session alias rewriting survives multibyte text**: byte-wise scanning sliced mid-UTF-8-char (panic) and `as char` casts mojibaked multibyte output; ref matching now anchors to char boundaries and copies characters verbatim.
- **Test contract drift from `tokenzero-g3y` batch**: shape tests updated to the `tz://s/` visible alias contract (`c9b1ca0`), and `fresh_arg_bypasses_dedup_via_tools_call` gained the initialize-lifecycle mark that `87797a3` added to sibling suites (the six "Mac-only" failures were real regressions everywhere).
- **Per-repo machine permits with fairness**: permit bases scope to the workspace root (hash suffix) via hub-provided root envs; a releasing process yields one poll window before re-acquiring, so a busy session cannot monopolize a class (observed live: one session starved every other for 11+ minutes). Test permit dirs are per-thread (fixes the RUST_TEST_THREADS=2 remove_dir_all/acquire flake).
- **Env-tunable hard wall**: `TOKENZERO_CODEMODE_HARD_MAX_WALL_MS` (clamped 1s..300s) raises the 5s ceiling that permit waits consumed entirely under contention.
- **Server hygiene**: MCP/CodeMode servers renice to +5 (`TOKENZERO_NO_RENICE` opts out) and the per-op MCP surface refuses to start when a CodeMode hub marks the repo active (`.zerostack/codemode.active`, `TOKENZERO_ALLOW_DUAL=1` overrides).
- **Fold-once acks**: scalar results fold into the ack exactly once across v2/v3/FastMCP assembly (was: `ok tz0 - =true =true t:...`).

### Added
- **CLI CodeMode Tier B trampoline**: stdin/`--stdin`/PLAN=`-` (plus non-TTY auto-read), `--budget` alias for `--max-visible-tokens`, same `tokenzero.codemode.v1` envelope/refs as MCP, typed validation errors for conflicting plan sources (`tokenzero-cli-trampoline-tier-b-6b6`).
- **Wind-tunnel replay MVP**: `benchmarks/wind_tunnel/` loads plan journals (or fixtures), replays under baseline vs candidate policy stubs, diffs action sequences, and exits non-zero on divergence (`tokenzero-wind-tunnel-replay-tyq`; stubs only, no model re-execution).
- **Token-turn session ledger (DPMT)**: `session-ledger-v2` prices visible/raw mass × turns_remaining and reports decisions per million visible token-turns as the headline metric (`tokenzero-token-turn-mass-ledger-cy2`).
- **Family-wide CodeMode CPU budget**: machine-wide analysis permit, fair multi-tenant slots, frozen permit contract v1 with index class, and shared crate `zerostack-machine-permit` (`tokenzero-nk6u`, `tokenzero-9tle`).
- **CodeMode envelope v3**: mechanical ack path for scalar/structured results (`tokenzero-envelope-v3-mechanical-ack-my1`).
- **Short session ref aliases**: visible capsules may use `tz://s/<16hex>` (`tokenzero-short-ref-aliases-dle`).
- **Shared schemas**: freeze `zerostack.derivation-provenance` and `zerostack.entity-novelty` (recovery novelty fusion with GraphZero).
- **Public claim gate**: `benchmarks/claim_public_gate.py` blocks treating the northstar 99% fixed-suite headline as a population/release claim (`tokenzero-g3y.18`).
- **Opt-in usage telemetry**: default-off; when enabled, records only `{execution_path, raw_tokens, spent_tokens}` for MCP/CodeMode (`tokenzero-f409`, https://github.com/AdityaVG13/tokenzero/pull/26).

### Changed
- **README / northstar**: regenerated fixed-suite tables from snapshot `20260717T014139.658900Z-ab0b3ca3090a` on warm `target/release/tokenzero` (`tokenzero-readme-northstar-rebaseline-9c6c`; SHA `b9c80c5`).
- **Permit scheduling**: light CodeMode gated behind the analysis permit; expand-only plans ungated; in-process slot waits bound to wall deadline; `MachinePermit` extracted under the 1k containment limit (`tokenzero-wawf`, `tokenzero-jn1i`, `tokenzero-zcy8`).
- **Northstar trend gates**: per-workload compression floors; expand size-class set equality; p50/p95/p99 non-regression so aggregate or p50 wins cannot mask regressions (`tokenzero-g3y.11`, `tokenzero-g3y.23`).
- **README claims**: 99.0% northstar total scoped as fixed-suite point estimate only (`tokenzero-g3y.18`).

### Fixed
- Pulse sync regression asserts a record completed before sync invocation appears in the successful SQLite snapshot; durable linearizability checker kept under benchmarks (`tokenzero-g3y.12`, P14-001).
- Deployment telemetry evidence package + reducer/gate attach sampling-frame and uncertainty for README Pulse totals; historical ~20k figures stay gated until a matching ledger is checked in (`tokenzero-g3y.9`, P17-002).
- Queue arrival/service gate keeps expand latency ESTIMATE_ONLY until observed lambda, E[S], and Var[S] are captured; micro harness records timestamped arrivals for the evidence path (`tokenzero-g3y.8`, P03-MG-002).
- Paired wall-time gate refuses treating the northstar token-count ceiling as runtime speedup until complete paired raw/TokenZero wall samples exist; micro harness documents the evidence path without a northstar cold rebuild (`tokenzero-g3y.2`, P03-MG-001).
- Corpus `materialize.py` stages every gzip member and renames `expanded/` only after the full set validates, so a malformed later member cannot leave a partially refreshed corpus (`tokenzero-g3y.17`, P16-002).
- Per-workload token gate refuses universal savings claims when any northstar workload expands, allowlisting only the known cargo-test counterexample until a compressor fix + full northstar rebaseline (`tokenzero-g3y.1`, P03-001; no full northstar run).
- Corpus `materialize.py` streams gzip members with explicit max expanded bytes and expansion-ratio ceilings, writing via temp+atomic replace so a hostile member cannot force unbounded live space (`tokenzero-g3y.26`, P16-001).
- Non-Unix shell cleanup joins inherited-pipe readers after Windows Job Object terminate (plus a final join grace / reaper), so blocked stdout/stderr workers are never detached when a descendant keeps the pipes open (`tokenzero-g3y.24`, P07-001).
- Recovery working-set `enforce_budget` compares resident tokens to replacement-floor tokens (not bytes) and returns `BudgetUnsatisfiable` when the budget cannot be met (`tokenzero-g3y.19`, P13-F1).
- npm shim suite kills removal of PATH `looksLikeNpmShimInvocation` via `package/npm/test/coverage_law_shim_detection.py`, covering distinct-shim vs npm-wrapper behavior (`tokenzero-g3y.16`, CE-P12-01).
- Spill prune bounds metadata work with a scanned-entry budget and wall deadline, and engine construction coalesces automatic cache maintenance so concurrent constructors do not multiply spill-dir scan/sort (`tokenzero-g3y.22`, P22-001).
- Homebrew formula `version`/download URLs track workspace release `1.4.0` so the install compatibility hard gate is feasible (`tokenzero-g3y.13`, P10-001).
- Completed-split compatibility entry points validate module directories (and refuse leftover monoliths) instead of opening removed `.rs` files (`tokenzero-g3y.6`, P08-002).
- Visible-budget truncation keeps every line the budget proof counted (`prefix_end_for_kept_lines` used `keep-2` and dropped one fitting line when `keep >= 2`) (`tokenzero-g3y.10`, P01-001).
- npm shim self-recursion detection uses realpath for `TOKENZERO_BIN` and a bounded prefix + npm cmd-shim structure match on PATH, so distinct binaries that merely mention the shim path are not refused and large candidates are not fully read (`tokenzero-g3y.3`).
- Delimiter-free `tokenzero run` keeps trailing `--json`/`--jsno`/`--jason` in child argv instead of promoting them to the parent envelope (`tokenzero-g3y.4`).
- MCP stdio/JSON-RPC gates `tools/list` (and other post-init methods) until `initialize` plus `notifications/initialized` (`tokenzero-g3y.15`).
- CodeMode host ops (find/search/expand and session resume load) abort on `hard_max_wall_ms` mid-call via cooperative checkpoints, not only between QuickJS microtasks (`tokenzero-y1gi`).
- Permit `Fatal` maps to non-retryable substrate errors (`tokenzero-091s`).
- CodeMode STATUS/INDEX markers match API shapes; busy responses use `isError` without a missing envelope (`tokenzero-vt7s`).
- Shell cwd defaults to `call_root` and is always echoed (`tokenzero-shell-cwd-default-q73`).
- Over-cap results autopage with terminal continuation; v3 scalar fold keeps `structuredContent.value` (`tokenzero-result-cap-autopage-be8`, `tokenzero-result-not-surfaced-jhh`).
- `Promise.all` runs host ops concurrently (`tokenzero-codemode-parallel-broken-z28`).
- CodeMode surface exclusivity: hide per-op tools; expand fallback stays internal (`tokenzero-surface-exclusivity-1r9`).
- Install rollback refuses when post-install config drifts (`tokenzero-g3y.5`).
- Shared-CAS schema fixtures reject duplicate keys (`tokenzero-g3y.7`).
- Cat rewrites preserve raw shell argument spans (`tokenzero-g3y.14`).
- Secret masking covers `authorization:` and `bearer` (`tokenzero-g3y.20`).
- JSON-RPC batch panics isolated per item with preserved ids (`tokenzero-g3y.21`).
- Windows migrate catch auto-restores checkout after archive failure (`tokenzero-g3y.25`).
- `zero.token.expand` coerces `{ref}` / array args and returns typed signature errors instead of opaque QuickJS failures (`tokenzero-expand-arg-coercion-inh`).
- Session resume uses local/CAS reachability only (`has_ref_local`) so large session memories cannot reload multi-MB recovery journals thousands of times and peg a core; session journal compact threshold lowered and record cap tightened.

### Docs
- RFC draft for cas-gc vNext derivation provenance (paired with the frozen schema).

## [1.4.0] -- 2026-07-15

### Added
- **Embeddable recovery API**: `TokenZeroStore` exposes the shared recovery store as a reusable Rust handle with byte-exact put, get, expand, pin, and lifecycle contracts.
- **Conservative shared-CAS maintenance**: mark-and-sweep GC, orphan repair, durable pin metadata, and cross-engine reachability preserve live ZeroRef payloads.
- **Reproducible performance evidence**: automated northstar rebaselining, release-binary provenance, source-state fingerprints, find backend crossover measurements, and retained history make benchmark claims auditable.

### Changed
- **Lower MCP latency**: retained before/after evidence measures p50 reductions of 53.7% for read, 58.1% for find, and 57.1% for expand while preserving advisory locking and JSONL append semantics.
- **Smaller implementation**: behavior-preserving consolidation and generated corpus materialization reduce code across `crates`, `benches`, `benchmarks`, and `scripts` from 110,723 to 59,072 lines (46.6%).
- **Search routing**: deterministic crossover evidence retains the internal scanner for small trees and `rg` for larger directory searches.
- **Benchmark integrity**: northstar runs now use one release binary for every component, fail closed on stale reuse, and record binary SHA-256 and source provenance.

### Fixed
- Recovery publication, garbage collection, concurrent writer synchronization, stale portable-reference hashes, malformed repeated fragments, and orphan segment cleanup now fail safely without exposing corrupted bytes or deleting live data.
- Windows CodeMode journal persistence preserves I/O errors and durable replacement semantics.
- MCP working-set admission, capability descriptor revisioning, async plan parsing, release telemetry audits, package-audit fixtures, and deleted regression coverage were restored and hardened.
- Release verification is portable across Windows command-length limits, path separators, platform-specific warnings, and recovery-cache resolution.
- Parallel MCP tests isolate ref-index overrides and content fixtures so short-lived test stores cannot interfere with one another.
- Workspace package manifests declare the crates.io version for the pinned `fastmcp-rust` dependency.

## [1.3.0] -- 2026-07-12

### Added
- **ZeroRef v1 contract**: portable blob refs of the form
  `(tz|fz|gz)://blob/<sha256>[#fragment]` with full-hash identity, digest
  verification before fragment selection, and a stable error taxonomy
  (`malformed`, `missing`, `corruption`, `unsupported`, …). Spec and golden
  vectors live under `docs/codemode.md`.
- **Shared-CAS adapter**: canonical content-addressed storage for ZeroRef v1
  blobs, with reachability/pin schema v1 frozen so GC and multi-engine
  expand share one truth.
- **Cross-engine blob expand**: `fz://` and `gz://` **blob** refs minted by
  fszero or graphzero expand via the shared CAS and, on miss, sibling engine
  stores under the same unified `.zerostack` root. Non-blob portable refs are
  still unsupported. Release evidence is the retained merged CI artifact; the
  checked-in fixture is only a reproducible host snapshot.
- **Strict fragment algebra**: typed `#Bstart-end` (byte, half-open) and
  `#Lstart-end` (line, inclusive) selectors with structured OOB errors.
- **Capsule-default expand**: expand returns preview + ref by default instead
  of shipping the full body into the model context.
- **Session-delta protocol**: watermark, tombstones, and byte telemetry so
  multi-turn sessions only ship what changed (`tokenzero.ledger.v1` curves
  and observatory evidence included).
- **Queryable session ledger** (`tokenzero.ledger.v1`): fail-open JSONL cost
  stream with visible/raw/prevented token mass, rotation, and CLI queries for
  repo/window cost, version delta, and per-agent spend. Pulse CLI aggregates
  per-session cost.
- **Sub-100-token manifest+delta session boot**: TZ/1 sidecars, demand-paged
  session memory, session-boot MCP resource and `tokenzero session-open` CLI
  (measured ~21 tokens on large corpora).
- **Loss-free working-set span eviction**: LRU-bounded resident set with
  TZ-EVICT markers, demand-paged rehydration, and byte-exact expand
  round-trips.
- **BlobEntry Inline/FileRef storage**: large reads store path+fingerprint
  pointers instead of duplicating payloads; FileRef verifies content on
  rehydrate.
- **Pay-once user ref index + cross-session memory**: same content across
  cache roots resolves to one user CAS object; privacy/scoping audit
  documents isolation boundaries.
- **Crash-safe plan-scoped mutation journaling** with bounded journal segment
  rotation (sealed generations + snapshot compaction).
- **CodeMode heavy-execution containment**: machine-wide permit, bounded
  queue, identical-plan dedup, and tracked lifecycle for background shell
  jobs.
- **Bounded session recipe registry**: `zero.register` / `zero.run` /
  `zero.list` for named parameterized plans with size and mutation gates.
- **Per-model tokenizer registry** and boundary-aware packing (provider-
  qualified model ids; residual budget packed to token boundaries).
- **Telemetry**: granular envelope token attribution, prevented-read bytes,
  prefix-cache hit rate, expand accounting contracts.
- **Legacy ref migration**: command + complete lifecycle for pre-v1 refs.
- **Portable engine binary discovery** and PR18 capability descriptor as sole
  tool/ZeroRef policy owner.
- **Bench harnesses**: ZeroRef 3x3 binary/store conformance matrix, ledger
  regression gate, byte-stable prefix suite, delta-encoding evidence,
  expand latency by size class, competitor bake-off / 1M-line navigation
  frameworks, elision predictor evaluation.

### Changed
- **MCP policy ownership**: single owner for CodeMode list/call; tools/call
  unified behind `gate_tools_call`; Classic surface gate restored.
- **Store resolution**: single workspace store resolver for CLI and MCP;
  recovery cache isolated per call root; store-root precedence tests frozen.
- **Expand surface**: `parse_ref` accepts only `tz://` after canonicalize for
  the portable path; sibling-engine fallback handles `fz://`/`gz://`.
- **Read/search path**: source-backed admission for large files cuts peak RSS;
  chunked bounded-memory expand reads; Pulse lock metadata skips redundant
  durability barriers; auto literal search on direct files runs in-process.
  Retained before/after evidence measures MCP p50 reductions of 53.7% for read,
  58.1% for find, and 57.1% for expand.
- **Program footprint**: behavior-preserving consolidation and generated corpus
  materialization reduce code across `crates`, `benches`, `benchmarks`, and
  `scripts` from 110,723 to 59,072 lines (46.6%).
- **Binary resolution**: typed `BinaryResolution` Result; require executable
  bit for env/PATH and well-known binaries.
- **Write recovery ladder** on CodeMode edit failure with QuickJS deny ladder
  parity.

### Fixed
- Cross-engine `fz://`/`gz://` expand no longer fails with ref-not-found when
  the blob lives only in a sibling engine store under the unified root.
- Evict livelock: victim selection no longer pins CAS-reachable refs forever
  after `drop_ref`.
- Relative CLI search paths resolve against the call root, not cwd.
- Allowlist escape for MCP-supplied roots; expand health signal on
  zeroref-malformed; search backend-parity keys.
- Shell mutation classification by command position (data is not intent);
  orchestration env scrubbed from user command children.
- SurfaceHealth shared across plan engines; crash-only expand unlocked when
  surface unhealthy; default recovery cache shared with expand.
- Journal lowering scope, exact explicit expand, routed execution roots.
- Observatory ref regex so ledger replay emits full expand accounting.
- Tokenizer metadata matches provider-qualified model ids
  (`openai/gpt-4o…`).
- `tz_report_tool_issue` menu cluster restored to the seven-entry jsonrpc
  contract; accepts `zero_execute`.

### Security / privacy
- User-scoped session memory and ref index with 0700/0600 permissions;
  cross-user isolation via home directory; documented threat model and
  known gaps in `docs/pulse.md`.
- h2c-style orchestration env scrub on user-command spawns.

## [1.2.0] -- 2026-07-05

### Added
- **FastMCP dual-mode transport**: CodeMode plans are delivered through both
  streamable-HTTP and stateless JSON-RPC FastMCP modes. The v2 ref-first
  envelope is the default for all `tz_execute_code` paths.
- **Envelope v2**: structured two-part wire protocol (primary text + compact
  JSON payload) with per-op ref tracking, telemetry scoring, and payload
  envelope token attribution.
- **CodeMode composition benchmark**: seven reproducible workloads measuring
  plan-based CodeMode execution against equivalent raw subprocess output and
  classic per-op MCP tool calls. Artifact committed as
  `demo/composition_benchmark.json`.
- **Per-user ref index**: a cross-cache-root SQLite index that maps every
  `tz://blob/*` ref to its owning cache path, making refs durable across
  engine restarts and cache directory moves.
- **Expand exactness guarantee**: `zero.token.expand` always returns byte-exact
  original content, verified via SHA-256 stored at compact time and checked on
  every expand.
- **Shell inline economics**: shell output is now policy-scored for compact
  rendering, with token savings reported per call in the `visible_tokens` and
  `raw_tokens` telemetry fields.
- **Corrective-hint errors**: the engine's non-ref expand error now suggests
  the correct API (`zero.fs.compound('read',{path})`) instead of just
  rejecting the input. Invalid refs on compact/expand include the malformed
  value for quick diagnosis.
- **README command audit**: every documented command in the README is verified
  by a CI gate (`make readme-command-audit`) that runs each command against
  the installed binary and checks for non-zero exit or unexpected output.

### Changed
- **Store resolution hygiene**: relative `ZEROSTACK_STORE_ROOT` env values are
  now resolved against the passed `repo_root`, never `current_dir()`,
  eliminating cwd contamination.
- **Object compact fidelity**: `zero.token.compact` of a non-string value
  JSON-serializes it with stable key ordering before storage; the expanded
  result is the exact JSON text (or parsed object in plan context).
- **Statement parser**: literal `return <scalar>;` expressions (`12345`,
  `"x"`, `true`) now fold correctly in lowered plans, no longer treated as
  variable references.
- **Benchmark determinism**: scale workloads use deterministic synthetic
  payloads instead of live git state. Two consecutive benchmark runs produce
  identical JSON except wall-time fields.

### Fixed
- `zerostack_store` tests no longer observe cwd-level `.zerostack` directories
  when resolving paths for a tempdir root.
- `zero.token.compact(someObject)` no longer stores `"[object Object]"`;
  objects are JSON-serialized before compression.
- `return 12345;` in a statement-plan no longer errors with "undefined
  variable: 12345".
- Non-ref expand errors now include a corrective hint pointing to
  `zero.fs.compound('read',{path})`.

## [1.0.x] -- earlier releases

- TokenZero 1.0.0 through 1.0.2: initial public release with CLI tools, MCP
  transport, QuickJS sandbox, and content-aware compression.
