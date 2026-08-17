# Authority boundaries V6 → V7–V10

**Status:** audit only. No runtime authority. `docs/internal/zerostack-handoff-2026-08-16.md` is discussion-only; V6 remains canonical. Waves 7–10 and the 16 Aug ZeroKernel essay start in audit/shadow; Wave 10 is official ZeroKernel pack and overrides the essay (no daemon, no idle pool). **This document grants no production authority, no mutation path, and no new write gate.**

**Scope:** ZeroStack only. No FSZero/GraphZero/TokenZero edits. No cross-repo dependency edges (see S5).

**Audit bead:** `zerostack-vpq2` (this doc). Program: `zerostack-vcqk`. Handoff: `docs/internal/zerostack-handoff-2026-08-16.md:1-30`.

---

## 1 V6 checked matrix (live authority)

| # | Surface | Live anchor (path:symbol) | Authority | Falsifier / stop |
|---|---|---|---|---|
| V6-01 | Lowering | `crates/zsx-core/src/lower.rs:lower_fs_plan:402`, `lower_token_expand:353`, `lower_compound_search_scope:703`, `lower_fs_world:427` | Canonical surface->domain lowering; typed dispatch. `lower.rs:1` no process-backed copy | Any un-lowered op reaching adapter => FAIL |
| V6-02 | Dispatch | `crates/zsx-core/src/connector.rs:prepare_mutation_journal:408`, `cross_mutation_journal:506`, `mark_dispatch_crossed:507`; `lib.rs:4` aggregate connector; `session.rs:474` dispatch_idle | One dispatch per mutation attempt; journal crossed before adapter | Dispatch without cross => journal unrecoverable -> stop |
| V6-03 | Approval grants | `crates/zero-abi/src/raw_worker.rs:ApprovalGrant:253`, `validate_approval_grant:288`, `ApprovalMetadata:381` | Additive grant consumed before action; absent valid for non-approval | Forged hex digest => InvalidHex deny |
| V6-04 | Machine permits | `crates/zerostack-machine-permit/src/lib.rs:MachinePermit:371`, `MachinePermitHeartbeat:84`, `try_create:583` | Machine-local liveness under tmp; heartbeat reclaims stale | Busy/Fatal => deny; Dead reclaimable else deny |
| V6-05 | Mutation journaling | `crates/zsx-core/src/connector.rs:MutationJournal:269`, `prepare_mutation_journal_unique:408`, `succeed:527`, `indeterminate:551`, `reconcile:629`; `zero-store/src/durable_journal.rs:ContinuationCartridgeRecord:729` | Durable prepare->cross->succeed/indeterminate; post-cross never redispatches | Missing dispatch_entry_digest => never crossed error |
| V6-06 | Two-phase gate | `crates/zero-gate/src/two_phase.rs`, `lib.rs:DecisionGate:407`, `GateInput:415`, `GateState:429` | Proof-carrying pre/post; aggregate requires all engines | ConflictingProofs/IrreversibleSpeculation/TerminalState => gate error |
| V6-07 | Decision escape | `crates/zero-gate/src/lib.rs:DecisionGate::RawFallback`, `session.rs:execute_envelope` escape | Unknown/Unsafe never aliases Safe; ambiguous => escape | False-safe => kill |
| V6-08 | execute result envelopes | `crates/zero-abi/src/zero_execute.rs:ZeroExecuteResult`, `session.rs:dispatch_metrics:538` | Typed result with receipt/ledger/residency/Q99 | ZeroDigest mismatch => rollback |
| V6-09 | Cancellation | `crates/zsx-core/src/session.rs:cancel_request`, `CancellationSignal:362` | Checked before dispatch; flag shared | Not observed => audit fail |
| V6-10 | Continuation scope | `crates/zero-abi/src/continuation.rs:ContinuationHandle:130`, `ContinuationRoots:90`, `validate_against:229`; `durable_journal.rs:ContinuationCartridgeRecord:729` | 8 roots + state Bound->Committed | Forged/CrossProject/Revoked => reject |
| V6-11 | Resource/verdict metering | `crates/zero-ledger/src/lib.rs:TokenLedger`, `charging_maps.rs:200`, `zsx-core/src/verdict.rs:reserve_dispatch:165`, `zero-gauge/src/bounds.rs:237` | Finite budgets; ledger complete | BudgetOverflow => fail-closed |
| V6-12 | Residency / Q99 | `crates/zero-gate/src/residency.rs:ResidencyPlan:398`, `q99.rs:Q99_CACHE_SCHEMA:19`, `CausalCacheDecision:472` | Residency validates root; Q99 needs 9 coords | IncompleteCoordinateSet => not certified |
| V6-13 | Explicit external read | `crates/zsx-core/src/fszero.rs` + `fszero_tests.rs:285`, FSZERO_SCRATCH_DIR | Explicit path O(file) without indexing | Absolute without explicit path => invalid_path |
| V6-14 | Process topology | `crates/zero-process/src/lib.rs:VerifiedChild`, `zero-codemode/src/worker.rs:12` | No daemon; idle wait before shutdown | Orphan child => NoOrphanProcess fail |
| V6-15 | Native fallback | `crates/zsx-core/src/help.rs:6`, handoff Direct-vs-Kernel, bead `zerostack-pvwg` | C_Z = min(C_direct,C_kernel) | Removing native while zero rejects => bug |

---

## 2 Proposed surfaces — classification (audit/shadow, no authority)

Each row: Exists? No. Falsifier revokes claim. Bead = future work; no cross-repo edges.

### V7 certificate / checkers

| Surface | Bead | Paths/symbols (proposed) | Falsifier |
|---|---|---|---|
| V7 certificate + trivalent verdict | `zerostack-fhcj` | `zero-cert/src/trace_export.rs:EventClass`, `zero-abi/src/identity.rs:equivalent_claim_permitted:384` | Unsafe issuing authority => W7-T01 fail |
| ProofIR checker | `zerostack-7inx` | `proofir/ -> Checker::check(): Safe/Unsafe/Unknown` | Checker != denotation => stop |
| Savings-provenance | `zerostack-nld8` | `zero-ledger/src/charging_maps.rs` + `zero-gate/src/quality.rs:683` | Missing segment label => no saving claim |

### W8 manifest / Q99

| Surface | Bead | Paths/symbols | Falsifier |
|---|---|---|---|
| ProjectImage(r) manifest | `zerostack-zksb` | `zero-store/src/cas.rs:is_full_lower_hex`, `residency.rs:ResidencyPlan` | Nonce mismatch W8-T3 => fail |
| Q99 action guard | `zerostack-zksb`/`zerostack-4lfp` | `q99.rs:validate:174`, `ResidencyThresholdChecker:486` | L1 hit as L2 truth => fail |
| Precommit warm-swap | `zerostack-4lfp` | `session.rs:prepare_image->publish` | Warm not before publish => fail |

### W9-E handles / demand / Snap-to-File

| Surface | Bead | Paths/symbols | Falsifier |
|---|---|---|---|
| Demand plan + completeness | `zerostack-3cdn` | hub SafeExpandHandle unforgeable | False-safe handle => no edit authority |
| SafeExpandHandle + exact expansion | `zerostack-3cdn` | `cas.rs:object_path`, `lower.rs:lower_compound_search_scope:703`, `z.expand` | Hidden retry => Unsafe |
| Snap-to-File S0-S4 | `zerostack-pfvi` | `z.snap(task)`, `z.resolve(demand)` | Multi-file sold as one-file => blocker |

### W9-D effects (shadow)

| Surface | Bead | Paths/symbols | Falsifier |
|---|---|---|---|
| Residual frontier | `zerostack-e7dz` | `zero-gate/src/lib.rs:DecisionGate` | False exclusion => kill |
| Effect programs | `zerostack-qg2a` | `zero-abi/src/effect.rs:EffectClass` | Preimage mismatch => Unknown |
| Q99 repair queue | `zerostack-rybb` | `reinvestment.rs:q99_error:1506` | g < gmin with Q99 claim => fail |

### W10 zero.execute / supervisor / preflight / guest / CAS / release

| Surface | Bead | Paths/symbols | Falsifier |
|---|---|---|---|
| zero.execute zerokernel | `zerostack-0n55` | `zero_execute.rs:ZeroExecuteResult` + `ContinuationCartridgeRecord:729` | Unknown field not fail-closed => fail |
| Supervisor embedded/one-shot | `zerostack-s0lx` | `zero-codemode/src/worker.rs`, `zero-process:VerifiedChild` | Executor !=0 after terminal => W10-T11 fail |
| Preflight broker | `zerostack-pvwg` | `z.context.{projectRoot,workspaceRoot,requestRoot,sessionRoot}` | Semantic auto-repair => fail |
| Guest z surface | `zerostack-fhcj`/`zerostack-xbg3` | `z.resolve`/`z.expand`/`z.snap`/`z.transaction` | Broken W9 chain with authority => W10-T12 fail |
| Session/CAS + release | `zerostack-xbg3` | `cas.rs:compare_and_swap`, `durable_journal.rs:1584`, `release.rs:PUBLIC_CLAIM_GATES` | CAS without roots => W10-T9 fail |

### Daemon topology (forbidden)

Wave 10 forbids: listener, per-session resident worker, global poller, background event loop, detached task (handoff: Two production profiles). Allowed: harness `zsx mcp` child, FSZero roots, short-lived one-shot workers. Falsifier: ps shows listener or executor>0 between calls; idle CPU>0.1%% or GPU>0 => release failure. Bead `zerostack-s0lx` asserts engine count zero.

### K4 effects + K5 promotion

| Phase | Bead | Falsifier |
|---|---|---|
| K4 transactional effects | `zerostack-qg2a`+`zerostack-rybb` | Not via z.transaction or commits without Q99 gate => fail |
| K5 promoted operator W10-T14 | `zerostack-xbg3` P2 | No stable verified history => fail; removes baseline => fail |

---

## 3 No cross-repo edges

ZeroStack only for this audit and W7-W10 shadow/K0 until per-repo bead. FSZero, GraphZero, TokenZero receive no edits; missing support becomes new bead in that repo later (`zerostack-vcqk` tracker). All children `repo_scope: ZeroStack only` (`zerostack-vpq2`, `zerostack-0n55`, `zerostack-rzpn`, `zerostack-s0lx`, `zerostack-pvwg`, `zerostack-fhcj`, `zerostack-7inx`, `zerostack-zksb`, `zerostack-nld8`, `zerostack-4lfp`, `zerostack-3cdn`, `zerostack-pfvi`, `zerostack-e7dz`, `zerostack-qg2a`, `zerostack-rybb`, `zerostack-xbg3`).

---

## 4 Anchor index (concise)

- Handoff: `docs/internal/zerostack-handoff-2026-08-16.md:1-30, Wave 7 ETNF, Wave 8 ProjectImage, Wave 9 E/D, Wave 10 daemonless + K0-K5`
- Lowering: `crates/zsx-core/src/lower.rs:402,353,703`
- Dispatch/journal: `crates/zsx-core/src/connector.rs:269,408,506,527,551,629`
- Approval: `crates/zero-abi/src/raw_worker.rs:253,288`
- Permits: `crates/zerostack-machine-permit/src/lib.rs:371,84,583`
- Gate: `crates/zero-gate/src/lib.rs:407,429`, `two_phase.rs`, `aggregate.rs:106`
- Results: `crates/zero-abi/src/zero_execute.rs:ZeroExecuteResult`, `session.rs:538`
- Cancel: `crates/zsx-core/src/session.rs:cancel_request`, `CancellationSignal:362`
- Continuation: `crates/zero-abi/src/continuation.rs:130,90,229`, `durable_journal.rs:729`
- Metering: `crates/zero-ledger/src/lib.rs:TokenLedger`, `charging_maps.rs:200`, `verdict.rs:165`, `zero-gauge/src/bounds.rs:237`
- Residency/Q99: `crates/zero-gate/src/residency.rs:398`, `q99.rs:19,472`
- External read: `crates/zsx-core/src/fszero.rs` + `fszero_tests.rs:285`
- Process: `crates/zero-process/src/lib.rs:VerifiedChild`, `zero-codemode/src/worker.rs:12`
- Fallback: `crates/zsx-core/src/help.rs:6`, `zerostack-pvwg`
- W10 supervisor: `zerostack-s0lx` + `zerostack-0n55`, `zerostack-pvwg`, `zerostack-rzpn`

---

## 5 How to use this audit

1. Every later bead must reference one permitted authority class above.
2. Shadow work may add checkers/receipts but never new ApprovalGrant or direct commit without cross + Gate Terminal + CAS.
3. Native fallback stays; C_Z = min(C_direct,C_kernel) (W10-T13).
4. Re-audit after each K-phase; close `zerostack-vpq2` only when matrix matches live code.

*End. Audit grants no runtime authority.*
