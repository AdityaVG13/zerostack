# Crosswalk Audit: ZS-ADAPTER-001..011, ZS-SESSION-001..005, ZS-EXEC-001..007

Audited against repo `/Users/aditya/AI/ZeroStack` (crates: `zsx`, `zsx-core`, `zsx-node`, `zero-codemode`, `zero-mcp`, `zero-process`, `zero-abi`). Requirement text from `racc/v6/implementation/IMPLEMENTATION_BACKLOG_V6.csv`.

## Layer map (what exists)

| Layer | Where | What |
|---|---|---|
| Outer exec surface | `crates/zsx/src/main.rs`, `exec.rs`; `crates/zsx-node/src/session.rs` | `zsx exec -C ROOT [--file PLAN] [--timeout-ms N]`; N-API `NativeZsxSession.execute(plan, timeoutMs, signal?)` |
| Typed envelope | `crates/zsx-node/src/envelope.rs` | `{protocol: zerostack.zsx.v1, ok, generation, request_id, result?, error?{code, detail, retry_after_ms?}}` |
| Session authority | `crates/zsx-core/src/session.rs` | generation, bounded queue (cap 8), per-request cancellation, replacement, shutdown, approvals, verdict loop |
| In-process adapter contract | `crates/zsx-core/src/adapter.rs` | `DomainAdapter`, `AdapterCall{request, cancellation: &CancellationSignal}`, `AdapterBinding` (digests) |
| Aggregate connector | `crates/zsx-core/src/connector.rs` | dispatch with `deadline_unix_ms`, approval consumption, mutation-attempt journals, Indeterminate recovery |
| CodeMode plan interpreter (private composition) | `crates/zero-codemode/src/interpreter.rs`, `host.rs`, `wrap.rs`, `limits.rs` | tree-sitter JS subset, capability tree, fuel/deadline/cancel/memory bounds, result spill + step receipts |
| Raw-worker adapter | `crates/zero-codemode/src/worker.rs` | `WorkerClient::dispatch_with_cancel`, handshake binding pins, cancel/deadline |
| MCP adapter | `crates/zero-mcp/src/mcp_transport.rs` | FastMCP stdio, `McpCallContext` cancel/deadline, dynamic catalog |
| ABI types | `crates/zero-abi/src/raw_worker.rs`, `surface.rs`, `result.rs` | `CallRequest.deadline_unix_ms`, `WorkerResult/WorkerError`, `SurfaceRegistration` (codemode|mcp), `ZeroResultV1` |

**Key negative findings (grep across all 7 crates):**
- `DecisionRequired`, `RejectedNoMutation`, `BaselineFallbackRequired`, `EvidenceExpansionRequired`, `VerificationUnknown`, `FailedNoAuthority` — **zero matches** in crates (only `ExactEvidenceExpansion`/`DominanceUnresolved` in zero-gate's reinvestment model, unrelated to the outcome vocabulary).
- No continuation handle type, no resume API, no `Bound/Snapshotted/Resolved/...` state machine (only lifecycle states `accepting/replacing/terminating` + failure codes + zero-store attempt states `Prepared/DispatchCrossed/...`).
- No operation DAG, no decision-boundary node marking, no contingent-policy type, no renderer roots, no cross-adapter conformance suite.
- `fs.plan`/`fs.world` are registered surface ops (`zsx-core/src/lower.rs:18-45`) but their semantics live in the FSZero engine repo (`fs-zero` path dep, `zsx-core/Cargo.toml`), out of this repo's scope.

---

## ZS-ADAPTER-001 — Stable Zero Execute outer tool

**STATUS: partial**

**EVIDENCE**
- `crates/zsx/src/exec.rs:18` `pub const ZSX_PROTOCOL: &str = "zerostack.zsx.v1"`; `exec()` at `exec.rs:29-50` returns fixed envelope `{protocol, ok, generation, request_id, result}`.
- `crates/zsx/src/main.rs:52-76` fixed CLI arg grammar (`-C`, `--file`, `--timeout-ms`) — no project-dependent flags.
- `crates/zsx-node/src/session.rs:71-101` `NativeZsxSession::execute(plan, timeout_ms, signal)` — fixed 3-arg schema; project details flow through args/plan, not schema mutation.
- `crates/zsx-node/src/envelope.rs:27-40` fixed envelope shape; `to_value()` at 101-137.

**GAP**: This is a binary/addon API, not a registered model-facing tool definition ("conceptually zero.execute") with a versioned tool-definition schema; no test proves schema bytes identical across 10,000 randomized tasks (acceptance criterion untested). Envelope field set is fixed by construction, so the property is plausible but unattested.

**CONFIDENCE: high** (surface exists and is fixed; schema-identity acceptance not proven)

---

## ZS-ADAPTER-002 — Structured task submission

**STATUS: partial**

**EVIDENCE**
- Submission is the plan string + typed transport: `ZsxSession::execute_with_approvals` (`crates/zsx-core/src/session.rs:844-862`), `ZsxSession::execute` (`~817-841`), `ZsxCommand::Execute` (`session.rs:201-211`) carries generation/request_id/timeout/approval_grants/verdict_envelope.
- CLI transport: `crates/zsx/src/main.rs:79-110`; native plugin: `crates/zsx-node/src/session.rs:71-101`; MCP transport: `crates/zero-mcp/src/mcp_transport.rs:516-500` `execute_bounded`.
- Request validation before dispatch: `connector.rs:929-1067` (`dispatch`) — typed `CallRequest` at `zero-abi/src/raw_worker.rs:239-251`.

**GAP**: There is no explicit "canonical Task Contract" type; the contract is a raw plan string + envelope. No round-trip fixture that preserves "every semantic field across CLI, RPC, native plugin, and optional MCP transports" — per-transport conformance fixtures exist (`conformance/fixtures/canonical_dispatch_vectors.json`, `raw_worker_v2_frames.json`) but no cross-transport round-trip assertion.

**CONFIDENCE: medium** (transports exist and are typed; canonical Task Contract + round-trip fixtures absent)

---

## ZS-ADAPTER-003 — Typed result envelope

**STATUS: conflicting**

**EVIDENCE**
- Envelope with typed failure codes: `crates/zsx-node/src/envelope.rs:27-137` — `ok|commit_race|cancelled|from_zsx_error|panic`; `EnvelopeError{code, detail, retry_after_ms}`.
- Failure-code vocabulary: `ZsxSessionFailureCode` (`crates/zsx-core/src/session.rs:48-90`): `cancelled`, `commit_race`, `stale_generation`, `backpressure`, `verdict_rejected`, `internal_panic` (napi), etc.
- Cancellation/timeout cannot render as Completed: `ExecuteTask::compute` (`crates/zsx-node/src/tasks.rs:61-106`) maps abort→`cancelled`/`commit_race`; deadline→`ZsxSessionFailureCode` via `backend_failure_code` (`session.rs:93-100`); interpreter `HostError::DeadlineExceeded`/`Cancelled` (`crates/zero-codemode/src/interpreter.rs:281-288, 565-572`).

**GAP**: The mandated 8-outcome vocabulary (Completed, DecisionRequired, EvidenceExpansionRequired, VerificationUnknown, BaselineFallbackRequired, RejectedNoMutation, Cancelled, FailedNoAuthority) does not exist. The repo uses its own envelope `ok:true/false` + code strings. `DecisionRequired`/`RejectedNoMutation`/`BaselineFallbackRequired`/`EvidenceExpansionRequired`/`VerificationUnknown`/`FailedNoAuthority` have zero occurrences in crates. This is a naming/semantics conflict, not just a gap: harness adapters built against this envelope will not emit the spec's outcomes.

**CONFIDENCE: high** (envelope exists and is typed, but vocabulary conflicts with spec)

---

## ZS-ADAPTER-004 — Opaque continuation handles

**STATUS: missing**

**EVIDENCE**
- No handle type, no handle→backend mapping. Closest artifacts are not continuation handles:
  - Durable mutation-attempt journals + `reconcile_request`/`reconcile_all_attempts` (`crates/zsx-core/src/session.rs:1032-1080`) — crash recovery, not resumable execution.
  - `SessionReplacementReceipt`/generation (`session.rs:1083-1186`) — generation bump, not a handle.
  - `zero-store/src/durable_journal.rs:474` `ContinuationCartridgeV1` — journal-transaction cartridge for mutation recovery, no harness-visible handle, no tenant/session scoping or revocation API.

**GAP**: Everything in the acceptance is absent: forged/cross-tenant/stale handle rejection, resume-without-retransmitting-evidence, revocability, resolve-through-backend-only. "Valid handles resume identical backend state without retransmitting prior project evidence" is unimplemented.

**CONFIDENCE: high**

---

## ZS-ADAPTER-005 — Native-tool coexistence

**STATUS: partial**

**EVIDENCE**
- The addon is a standalone N-API class (`crates/zsx-node/src/session.rs:44-49` `NativeZsxSession`); it registers nothing globally and contains no shadowing/hiding of native tools. Registration is additive by construction (`zsx-core/src/lower.rs:18-45` `GlobalRegistration::zero`; `zero-abi/src/surface.rs:96-137` `DomainAdapterRegistration`).
- "Disabling adapter leaves native functionality unchanged" is trivially true (no global hooks), but there is no harness-integration code or test in this repo proving model selection of either path in the same session.

**GAP**: No test or harness adapter layer demonstrating coexistence in a real session; the property is architectural, not evidenced.

**CONFIDENCE: medium** (no code could violate it in-repo; also nothing proves it)

---

## ZS-ADAPTER-006 — Cancellation and deadline propagation

**STATUS: implemented** (with concern)

**EVIDENCE**
- Shared per-request signal: `crates/zero-codemode/src/cancellation.rs:8-23` `CancellationSignal`; `ZsxSessionCancellationSignal` merges host `Arc<AtomicBool>` + worker signal (`crates/zsx-core/src/session.rs:274-296`); `cancel_request` at `session.rs:674-696` (pre-start cancel recorded; in-flight cancel via token; fresh token per request).
- Deadline propagation into adapters: `CallRequest.deadline_unix_ms` (`zero-abi/src/raw_worker.rs:244`) computed from `DispatchContext.remaining()` at `crates/zsx-core/src/connector.rs:991-1005`; re-checked before adapter call (`connector.rs:415-417` `dispatch_lapsed`, `connector.rs:1120-1125`).
- Adapter contract: `AdapterCall.cancellation` (`crates/zsx-core/src/adapter.rs:188-200`); FSZero adapter checks cancel/deadline before and after dispatch (`crates/zsx-core/src/fszero.rs:259-296`, `deadline_expired` at 110-115).
- Interpreter enforces: `tick()` at `crates/zero-codemode/src/interpreter.rs:565-572` (`Cancelled`/`DeadlineExceeded`); MCP: `McpCallContext` (`zero-mcp/src/mcp_transport.rs:133-163`).
- No-partial-authority: mutation journals `Prepared`→`DispatchCrossed`→terminal `Indeterminate` (`connector.rs:19-22, 405-427`); cancellation recorded; `commit_race` envelope keeps committed result visible (`envelope.rs:53-66`).
- Tests: `tests/rust/zsx-core/request_cancellation.rs` (TestAdapter honoring signal+deadline, 58-120); `tests/rust/zsx/one_process.rs:257` `deadlines_and_cancellation_are_enforced_in_process`; `tests/rust/zero-codemode/host_contract.rs:537` `external_cancel_interrupts_sync_loop`, `:555` `sync_loop_hits_deadline`, `:568` `explicit_timeout_is_bounded_by_host_limit`; `tests/rust/zero-codemode/worker_adapter.rs:230` `deadline_kills_reaps_and_terminal_client_rejects_dispatch`, `:246` `external_cancellation_interrupts_pending_call_and_reaps`.

**GAP/CONCERN**: Cancellation is cooperative — a mid-operation engine call that ignores the signal is only checked before/after by the FSZero adapter (`fszero.rs:259-296`); "cancel during read/build/test/verify/commit" holds only where the engine polls. Budget changes mid-flight (not just fixed timeout) are not propagated.

**CONFIDENCE: high** (best-implemented area of the audit; residual risk is engine cooperativity)

---

## ZS-ADAPTER-007 — Streaming without transcript flooding

**STATUS: partial**

**EVIDENCE**
- Bounded model-visible results: 12 KiB inline budget `SESSION_VISIBLE_RESULT_BYTES` (`crates/zsx-core/src/session.rs:43-46`), spill to CAS with 512-byte preview + exact expansion handle (`crates/zero-codemode/src/host.rs:125` `RESULT_SPILL_SCHEMA`, `spill_result` at `host.rs:564`, `directly_expands_one_spill_ref` in interpreter `execute_inner`).
- Bounded progress accounting: `attach_step_receipt` → `zerostack.codemode.step_receipt.v1` with `head_sha256` + `execution_id` (`crates/zero-codemode/src/interpreter.rs:~300-330`).
- Long-running jobs: `token.job`/`token.shell` background seams (`crates/zsx-core/src/tokenzero.rs:34-36, 615-618`).

**GAP**: There is no progress/streaming channel to UI or logs at all; step receipts are emitted with the final result, not streamed. "Long build produces large UI logs but bounded model result" — the bounded-result half exists; the streaming half does not.

**CONFIDENCE: high**

---

## ZS-ADAPTER-008 — Cross-harness semantic/render split

**STATUS: missing**

**EVIDENCE**
- No renderer roots, no harness-independent semantic decision views stored under rooted renderers.
- Closest: MCP alias metadata preserves a "visible face" (`McpAliasMetadata`, `zero-mcp/src/mcp_transport.rs:63-81`) and `McpErrorPresentation::{Structured,PlainMessage}` (`mcp_transport.rs:98-106`) — presentation variants, not rooted renderings.
- `conformance/contracts/supported_surface_matrix.toml` declares the feature matrix but no render-root model.

**GAP**: "Switching Pi/Codex/Claude/Cursor adapters reuses semantic roots; only render cache is invalidated" — no semantic root for results exists to reuse; whole acceptance absent.

**CONFIDENCE: high**

---

## ZS-ADAPTER-009 — Adapter conformance suite

**STATUS: missing**

**EVIDENCE**
- Per-layer contract tests exist: `tests/rust/zsx/one_process.rs`, `tests/rust/zsx-core/request_cancellation.rs`, `tests/rust/zsx-core/aggregate_world.rs` (one call atomically commits 100 files, `:83`), `tests/rust/zero-codemode/host_contract.rs`, `worker_adapter.rs`, `edit_protocol_conformance.rs`, `tests/rust/zero-mcp/unit/mcp_transport.rs`.
- Fixtures: `conformance/fixtures/canonical_dispatch_vectors.json`, `raw_worker_v2_frames.json`; contract docs `conformance/contracts/supported_surface_matrix.toml`.

**GAP**: No suite runs the *same rooted task* through every adapter (CLI/RPC/native/MCP) and compares protected outcome, project root, receipts, decision boundaries, semantic resource projection. Golden semantic equivalence is not asserted anywhere; only component-level tests exist.

**CONFIDENCE: high**

---

## ZS-SESSION-001 — Rooted continuation state

**STATUS: missing**

**EVIDENCE**
- Session does persist *some* state under `state_root` (`ZsxBuilder::with_state_root`, `crates/zsx-core/src/session.rs:463-470`): shared CAS, spill store, mutation attempt journals (`attempts_root_for`, `connector.rs:262`), reachability snapshots with epochs (`connector.rs:1364-1377`), verdict receipts.
- Session lifecycle state struct: `ZsxSessionState` (`session.rs:152-163`) — generation, accepting/replacing/terminating, seen/active request ids, consumed approvals. `replace()` clears `seen_request_ids` (`session.rs:1129-1130`).

**GAP**: No single continuation root binding task root, project root, evidence roots, decision state, candidate state, verifier state, baseline reserve, resource ledger. `replace()` deliberately discards request history — resume-after-restart without replaying model-visible history is impossible; only mutation-attempt *recovery* (Indeterminate classification) exists.

**CONFIDENCE: high**

---

## ZS-SESSION-002 — Continuation state machine

**STATUS: missing**

**EVIDENCE**
- No states `Bound|Snapshotted|Resolved|DecisionRequired|Planned|Executing|DeltaSealed|Verifying|Authorized|Committed|Restored|Rejected|Unknown|Cancelled` anywhere in crates (grep negative).
- Existing approximations: session lifecycle flags (accepting/replacing/terminating, `session.rs:152-163`), `ZsxSessionFailureCode` (`session.rs:48-90`), attempt states `Prepared/DispatchCrossed/Recovered/Terminal` in zero-store (`zero-store/src/durable_journal.rs`), verdict `Pass/Fail` (`crates/zsx-core/src/verdict.rs:62-81`).

**GAP**: No state machine with a total allowed-transition check; "every event has a total allowed-transition check; illegal transitions rejected and audited" unimplemented (session has ad-hoc guards, not a transition table).

**CONFIDENCE: high**

---

## ZS-SESSION-003 — Branching continuations

**STATUS: missing**

**EVIDENCE**
- Only artifact: `SessionReplacementReason::BeforeFork` label (`crates/zsx-core/src/session.rs:18-31`) — a reason string for generation replacement, no fork semantics.

**GAP**: No child-continuation creation, no parent immutability, no rooted equivalence/merge rules, no "one verified child can commit while losing branches leave no authority."

**CONFIDENCE: high**

---

## ZS-SESSION-004 — Continuation compaction without semantic loss

**STATUS: missing**

**EVIDENCE**
- Journals are append-only/immutable by design (`zero-store/src/durable_journal.rs`, `immutable_receipts` at `:1572`); no compaction API exists.
- No snapshot/root sealing API for internal event logs; step receipts are per-execution (`interpreter.rs` step receipt), not a session log.

**GAP**: Whole acceptance (replay before/after internal compaction yields identical authoritative state and audit roots) is unimplemented — no compaction to speak of, so nothing to test.

**CONFIDENCE: high**

---

## ZS-SESSION-005 — Durable continuation compatibility

**STATUS: missing**

**EVIDENCE**
- Ingredients that *partially* anticipate it: ABI/session pins — `AdapterBinding` digests (`adapter.rs:42-100`), worker handshake pins `expected_worker_revision/expected_contract_digest/expected_registry_digest` (`zero-codemode/src/worker.rs:45-65`), `raw_worker_protocol_digest_hex`, session_id binding in grants/traces, state_root isolation.
- No continuation handle exists to bind ABI/task/project/security/model-rendering/epoch roots (see ADAPTER-004).

**GAP**: "Handles survive process restart and compatible adapter changes; stale/cross-project/incompatible-ABI/forged handles fail without mutation" — handle layer absent; the pinning machinery is not exposed through any handle.

**CONFIDENCE: high**

---

## ZS-EXEC-001 — Mechanical operation graph

**STATUS: missing** (in-repo; engine dependency)

**EVIDENCE**
- Surface registers `fs.plan`/`fs.world` (`crates/zsx-core/src/lower.rs:18-45`) but they dispatch into FSZero (`fszero.rs:259-296` `dispatch_codemode_method`, via `fs-zero` path dep — separate repo). zsx-core deliberately refuses planner ops as "planner/JavaScript/MCP" names (`fszero.rs:157-170` `is_forbidden_operation`).
- The interpreter executes a linear JS-subset script with `Promise.all/race` (`crates/zero-codemode/src/interpreter.rs`), not a rooted DAG; connector serializes per-engine with a bounded dispatch queue (`connector.rs:929-1067`).

**GAP**: No rooted operation DAG with dependency edges; no decision-boundary node marking; "operation graph never crosses a labeled semantic decision without a supplied contingent policy" unimplemented (there is no contingent policy concept at all — see EXEC-004).

**CONFIDENCE: high** for in-repo absence; `fs.plan` semantics must be re-audited in the FSZero repo.

---

## ZS-EXEC-002 — Private executor

**STATUS: partial**

**EVIDENCE**
- The private-composition core exists and is real: plans execute inside the confined interpreter; intermediate connector results never enter model history; final result is normalized public JSON (`PUBLIC_RESULT_FIELDS`, `host.rs:185`; `normalize_public_result` `host.rs:446`) with optional spill refs and per-execution `step_receipt` (`execution_id` `cm://exec/…`) — all intermediates remain recoverable via refs/CAS.
- Tests: `tests/rust/zsx/one_process.rs:92` in-process dispatch proves one process/no worker; `tests/rust/zsx-core/aggregate_world.rs:83`; `tests/rust/zero-codemode/host_contract.rs` (inline vs ref results at `:302`).

**GAP**: It executes *scripts*, not "eligible operation DAG nodes" (EXEC-001's DAG does not exist). No test asserting "model receives same protected decision information as primitive trace on fixture tasks" — the model-visible result is a single envelope, which is the *effect* of private composition but the DAG-level acceptance is untested.

**CONFIDENCE: high** (private execution implemented; DAG framing absent)

---

## ZS-EXEC-003 — Decision boundary return

**STATUS: missing**

**EVIDENCE**
- `DecisionRequired` has zero occurrences in crates; `HostError` has no decision-boundary variant (`zero-codemode/src/host.rs`; interpreter error mapping `interpreter.rs:281-288`).
- The interpreter evaluates `if`/`ternary`/`switch` privately (`interpreter.rs` statement/eval handlers) with no mechanism to return an unresolved branch to the model with alternatives/evidence + continuation handle.

**GAP**: "Adversarial tasks requiring k adaptive decisions produce at least k returns unless model supplied a total contingent policy" — no decision returns, no policy, no handle. Entire acceptance unimplemented.

**CONFIDENCE: high**

---

## ZS-EXEC-004 — Contingent policy execution

**STATUS: missing**

**EVIDENCE**: No policy type exists: grep for `policy` in zsx-core/zero-codemode/zsx-node finds only `policy_digest` in approval grants (`zero-abi/src/raw_worker.rs:266`) and `ProcessResourcePolicy` (`zero-process/src/resource.rs:11`) — neither is a typed policy over observation classes. No "unhandled observation → return" path.

**GAP**: "Policy-covered branches stay within one call; injected unhandled observation returns DecisionRequired" — unimplemented.

**CONFIDENCE: high**

---

## ZS-EXEC-005 — Critical-path scheduler

**STATUS: partial**

**EVIDENCE**
- Bounded concurrent dispatch exists: `MAX_INFLIGHT_CONNECTOR_CALLS = 64` (`zero-codemode/src/host.rs:60-63`), per-engine serialization locks (`connector.rs:1127-1130` `engine_locks`), permit classes with core-based slots (`connector.rs:127-153` `dispatch_permit_class/slots`), verdict-loop dispatches.
- No dependency analysis, no critical path, no deterministic-parallel-vs-sequential test.

**GAP**: "Parallel schedule matches sequential rooted outputs and reduces wall-clock only where dependencies permit" — no schedule exists to compare; parallelism is queue-level, not dependency-level. Deterministic output is achieved by per-engine serialization, not by DAG scheduling.

**CONFIDENCE: high**

---

## ZS-EXEC-006 — Tool adapter registry

**STATUS: partial**

**EVIDENCE**
- Registry machinery is substantial:
  - `GlobalRegistration`/capability tree (`zero-abi/src/surface.rs:15-53`; zsx-core registration `connector.rs:1396-1401` from `lower.rs:18-45` METHODS — fs/graph/token/help, 26 ops incl. `ast_search_many`, `fs.edit`/`write`/`transact`, `token.shell`).
  - `DomainAdapterRegistration`/`CanonicalRegistry` with typed dispatch + effect/approval policy (`zero-abi/src/surface.rs:86-137`; `zero-abi/src/dispatch.rs` `EffectClass`, `ApprovalRequirement`).
  - Side-effect annotations on MCP tools: `ToolAnnotations{read_only, destructive}` (`zero-mcp/src/mcp_transport.rs:330-347`).
  - Version/tool change invalidation: binding digests (`adapter.rs:42-100`), handshake pins (`worker.rs:45-65`), ABI digest; worker tests `handshake_fails_closed_for_every_binding_pin_and_ref_scheme` (`tests/rust/zero-codemode/worker_adapter.rs:206`).
  - Undeclared effect → Unsafe: approval-required ops fail without grants (`connector.rs:1143-1161`; `zero-abi/src/raw_worker.rs:275-330` `validate_approval_grant`).
- Edit protocol contract: `crates/zero-codemode/src/edit_protocol.rs` + `conformance/contracts/zero-edit-protocol-v1.md`.

**GAP**: Registry covers filesystem, search, AST, shell (external command) and read/query surfaces. Build/test/package/static-analysis/profiler/external-service *adapters* are not present in this repo (would be engine-owned in FSZero/GraphZero/TokenZero repos). "Version/tool change invalidates dependent artifacts" is enforced at handshake/registration, but no artifact-level invalidation beyond bindings.

**CONFIDENCE: high**

---

## ZS-EXEC-007 — Unresolved semantic decision gate

**STATUS: missing**

**EVIDENCE**: No gate exists: no observation-contingent branch detection, no "uniquely resolving verifier" concept in the executor, no pre-authority DecisionRequired return (see EXEC-003/004). The trivalent verdict work lives in `zero-gate` (kernel-level, not executor-level) and is not wired to any decision gate.

**GAP**: "Injected hidden semantic branches cannot be privately selected; purely mechanical branches remain privately composable" — the mechanical-branch part is true today (interpreter evaluates all branches privately), but that is precisely what makes the *absence* of the gate dangerous: there is nothing preventing private selection of hidden semantic branches.

**CONFIDENCE: high**

---

## Summary table

| ID | STATUS | Core evidence | Primary gap |
|---|---|---|---|
| ZS-ADAPTER-001 | partial | zsx/src/exec.rs:18,29-50; zsx-node/session.rs:71-101 | no registered tool schema; 10k-task schema-identity test absent |
| ZS-ADAPTER-002 | partial | session.rs:844-862; connector.rs:929-1067 | no canonical Task Contract; no cross-transport round-trip fixture |
| ZS-ADAPTER-003 | conflicting | envelope.rs:27-137; session.rs:48-90 | mandated 8-outcome vocabulary absent; own code vocabulary instead |
| ZS-ADAPTER-004 | missing | session.rs:1032-1080 (recovery ≠ handle) | no continuation handles at all |
| ZS-ADAPTER-005 | partial | zsx-node/session.rs:44-49 | additive-by-construction; no coexistence test |
| ZS-ADAPTER-006 | implemented | cancellation.rs:8-23; session.rs:674-696; connector.rs:991-1005,1120-1161; fszero.rs:259-296; tests | cooperative-only mid-op cancel; no budget-change propagation |
| ZS-ADAPTER-007 | partial | host.rs:125,564; session.rs:43-46; interpreter step receipt | no progress/streaming channel |
| ZS-ADAPTER-008 | missing | mcp_transport.rs:63-81,98-106 (presentation only) | no semantic/render root split |
| ZS-ADAPTER-009 | missing | per-layer tests only | no golden cross-adapter suite |
| ZS-SESSION-001 | missing | session.rs:152-163; connector.rs:262 | no continuation root; replace() discards history |
| ZS-SESSION-002 | missing | session.rs:48-90 (codes ≠ states) | no 14-state machine / transition table |
| ZS-SESSION-003 | missing | session.rs:18-31 (BeforeFork label) | no branching continuations |
| ZS-SESSION-004 | missing | durable_journal immutable receipts | no compaction/snapshot-seal API |
| ZS-SESSION-005 | missing | worker.rs:45-65 pins (ingredients) | no handles to bind roots to |
| ZS-EXEC-001 | missing | lower.rs:18-45 (fs.plan → FSZero repo) | no rooted DAG / decision-boundary marking in-repo |
| ZS-EXEC-002 | partial | interpreter.rs; host.rs:185,446,564; one_process.rs:92 | script execution, not DAG nodes; no trace-equivalence test |
| ZS-EXEC-003 | missing | interpreter.rs:281-288 | no DecisionRequired return |
| ZS-EXEC-004 | missing | (no policy type) | no contingent policy |
| ZS-EXEC-005 | partial | host.rs:60-63; connector.rs:127-153,1127-1130 | queue-level parallelism only; no critical path |
| ZS-EXEC-006 | partial | surface.rs:86-137; lower.rs:18-45; mcp_transport.rs:330-347; worker_adapter.rs:206 | build/test/package/profiler/external-service adapters not in-repo |
| ZS-EXEC-007 | missing | (no gate) | hidden semantic branches can be privately selected today |

## Priority recommendations for implementers

1. **ZS-ADAPTER-006 is the only implemented item** — keep as the reference pattern (shared CancellationSignal + deadline_unix_ms + journaled Indeterminate recovery).
2. **Decision vocabulary (ADAPTER-003) + decision gate (EXEC-003/004/007)** are the highest-risk cluster: the private interpreter currently *can* privately select hidden semantic branches with no return path. This is a correctness boundary, not a feature gap.
3. **Continuation layer (SESSION-001..005, ADAPTER-004)** is a greenfield build; reuse `zero-store` durable journals + `state_root` machinery; nothing to migrate.
4. **Conformance suite (ADAPTER-009/011)** can be built from existing component tests + `conformance/fixtures/*.json`; no golden semantic-equivalence harness exists.
