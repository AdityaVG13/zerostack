# Draft 5 Detailed Predecessor Pack

This is a convenience pack. The source paths and hashes in the corpus manifest remain authoritative.


---

## SOURCE: `archive/extracted/Draft5/implementation/ZEROSTACK_IMPLEMENTATION_REQUIREMENTS_DRAFT5.md`

# ZeroStack Draft 5 — Complete Implementation Requirements

**Author:** Aditya G  
**Status:** Draft 5 implementation contract  
**Date:** 13 August 2026

## Purpose

This document is the coding contract for turning the Draft 5 RACC papers into a real harness-side backend. It does not prescribe a replacement codebase, a particular Rust module layout, database, async runtime, or transport. It states the semantic objects, state transitions, checkers, receipts, authority boundaries, metrics, failure behavior, and acceptance tests that the existing ZeroStack/RACC-R implementation must expose. Existing code may satisfy a requirement under different names. A requirement is complete only when its behavior is instrumented, fault-tested, and mapped to a concrete implementation location.

The baseline is always the **same model, same harness, same reasoning allowance, same native tools, same initial project root, and same acceptance contract**. ZeroStack is an optional backend strategy callable through programmatic tool calling, Code Mode, a native harness extension, CLI/stdio, local RPC, or optional MCP. It must never remove the native path or silently decide a semantic issue that belongs to the model or user.

## Non-negotiable invariants

1. Models and planners propose; the trusted kernel authorizes.
2. Unknown never means Safe.
3. Provider-visible history is append-only under normal operation.
4. Project objects are immutable, content-rooted, causally keyed, and formation-receipted.
5. A provider-cache miss cannot become project amnesia while valid L2 state remains available.
6. A project change invalidates the proven causal effect region, not the entire repository and not nothing.
7. A candidate edit executes in a child sandbox and either commits an exact verified successor or leaves the authoritative root unchanged.
8. Every native tool path and baseline reasoning allowance remain available.
9. Interface-token savings, provider cache hits, causal reuse, physical residency, complete work, and quality are separate metrics.
10. No product claim outruns the strongest implemented certificate and paired evidence.

## Priority meanings

- **P0:** required before any no-degradation, exact-cache, transactional-edit, or public Pareto claim.
- **P1:** required for practical one-/two-call execution, Q99 service measurement, and durable capability accumulation.
- **P2:** scale, optimization, and cross-harness maturity after P0/P1 correctness.


## Trusted kernel

### ZS-KERNEL-001 — Canonical serialization (P0)

**Requirement.** Define exactly one versioned canonical byte representation for every rooted object, contract, receipt, event, view, delta, and authority object. Reject noncanonical encodings at the authority boundary.

**Depends on.** None beyond the global contract.

**Theorem/claim obligation.** Core: Canonical Rendering Identity; Q99: Rendering Fragmentation Penalty

**Acceptance test.** Property tests produce identical bytes under map-order, whitespace, path-alias, locale, and process-order perturbations; malformed/noncanonical inputs are rejected.

**Current status.** Not implemented / audit required

### ZS-KERNEL-002 — Content roots (P0)

**Requirement.** Compute collision-resistant roots over canonical bytes and bind the digest algorithm/version into every root. Never use display names, timestamps, or database row IDs as semantic identity.

**Depends on.** ZS-KERNEL-001

**Theorem/claim obligation.** Q99: Causal Cache Soundness

**Acceptance test.** Mutation of any canonical field changes the root; identical content in separate processes and branches yields the same root.

**Current status.** Not implemented / audit required

### ZS-KERNEL-003 — Formation receipts (P0)

**Requirement.** A stored payload receives cache authority only through a receipt that binds constructor identity, contract root, complete dependency roots, execution record, payload root, and formation time/epoch.

**Depends on.** ZS-KERNEL-001,ZS-KERNEL-002

**Theorem/claim obligation.** Q99: Causal Cache Soundness

**Acceptance test.** Relabeling an unrelated payload with a valid causal key fails verification; dependency or constructor mutation revokes reuse.

**Current status.** Not implemented / audit required

### ZS-KERNEL-004 — Trivalent epistemic verdict (P0)

**Requirement.** Every checker returns Safe, Unsafe, or Unknown. Unknown never aliases Safe and cannot issue execution or publication authority.

**Depends on.** None beyond the global contract.

**Theorem/claim obligation.** Core: Decision-View Certificate Soundness; Harness: Atomic No-Partial-Authority

**Acceptance test.** Fault injection that removes one required premise always yields Unknown or Unsafe, never Completed.

**Current status.** Not implemented / audit required

### ZS-KERNEL-005 — Typed authority separation (P0)

**Requirement.** Planner, model, retriever, cache optimizer, and executor code cannot construct authority objects. Only trusted checkers may issue short-lived scoped authority after validating rooted evidence.

**Depends on.** ZS-KERNEL-004

**Theorem/claim obligation.** Harness: Atomic No-Partial-Authority; Pareto: Same-Harness Capability Superset

**Acceptance test.** Static/module-boundary audit plus runtime forgery tests show that fabricated, replayed, expired, scope-mismatched, or stale authorities fail.

**Current status.** Not implemented / audit required

### ZS-KERNEL-006 — Append-only authoritative event log (P0)

**Requirement.** Record every state transition, evidence observation, cache decision, execution, verification, authority issuance, commit, rollback, and resource charge in an append-only sequence with parent roots.

**Depends on.** ZS-KERNEL-001,ZS-KERNEL-002

**Theorem/claim obligation.** Runtime refinement obligation

**Acceptance test.** A killed process can replay the log to the same authoritative state; missing or reordered events are detectable.

**Current status.** Not implemented / audit required

### ZS-KERNEL-007 — Versioned semantic ABI (P0)

**Requirement.** Root the ABI version for task contracts, semantic objects, receipts, events, and results. Schema migration must be explicit and never silently reinterpret old bytes.

**Depends on.** ZS-KERNEL-001

**Theorem/claim obligation.** Core: Cross-Harness Semantic Factorization

**Acceptance test.** Golden fixtures decode identically across supported releases; incompatible versions fail closed or migrate with a rooted receipt.

**Current status.** Not implemented / audit required

### ZS-KERNEL-008 — No partial authority (P0)

**Requirement.** All authoritative project mutations are atomic: either an exact verified successor root becomes current or the authoritative root is unchanged.

**Depends on.** ZS-KERNEL-005,ZS-KERNEL-006

**Theorem/claim obligation.** Harness: Atomic No-Partial-Authority

**Acceptance test.** Crash at every instruction boundary around verify/authorize/commit produces either old root or complete new root, never partial state.

**Current status.** Not implemented / audit required

## Contracts

### ZS-CONTRACT-001 — Task contract (P0)

**Requirement.** Bind raw request, acceptance criteria, protected dimensions, Unknown/subjective dimensions, side-effect policy, environment fixtures, initial project root, model/harness/tool contracts, budget, deadline, and fallback policy.

**Depends on.** ZS-KERNEL-001

**Theorem/claim obligation.** Core: Harness Decision-Boundary Compression

**Acceptance test.** Changing any contract field produces a different root and invalidates dependent certificates.

**Current status.** Not implemented / audit required

### ZS-CONTRACT-002 — Model invocation contract (P0)

**Requirement.** Bind provider/model/version, tokenizer or counting method, sampling, reasoning allowance, stopping policy, system prompt root, tool permission set, and routing fields observable to the harness.

**Depends on.** ZS-CONTRACT-001

**Theorem/claim obligation.** Core: Harness Capability Superset; Pareto: Same-Harness Capability Superset

**Acceptance test.** Paired baseline/treatment manifests reject mismatched invocation fields.

**Current status.** Not implemented / audit required

### ZS-CONTRACT-003 — Harness contract (P0)

**Requirement.** Bind harness name/version, tool serialization, message ordering, transcript policy, cancellation semantics, native tool set, and adapter renderer version.

**Depends on.** ZS-CONTRACT-001

**Theorem/claim obligation.** Harness: Harness-Transport Factorization

**Acceptance test.** Cross-harness comparisons identify renderer differences without invalidating semantic project objects.

**Current status.** Not implemented / audit required

### ZS-CONTRACT-004 — Protected scope (P0)

**Requirement.** Represent what equivalence/nonregression actually covers: tests, API, behavior, security, performance, file effects, user-visible output, and successor-state obligations.

**Depends on.** ZS-CONTRACT-001

**Theorem/claim obligation.** Core: Protected decision equivalence; Pareto: protected quality

**Acceptance test.** An uncovered property is represented as Unknown and cannot be advertised as equivalent.

**Current status.** Not implemented / audit required

## Baseline sovereignty

### ZS-BASE-001 — Native path preservation (P0)

**Requirement.** Keep every ordinary harness tool and the complete non-ZeroStack execution path available to the model. ZeroStack is an optional strategy, not a mandatory bottleneck.

**Depends on.** ZS-CONTRACT-002,ZS-CONTRACT-003

**Theorem/claim obligation.** Core/Harness/Pareto: Capability Superset

**Acceptance test.** A conformance task can disable ZeroStack mid-run and complete through the same native tool path.

**Current status.** Not implemented / audit required

### ZS-BASE-002 — Reasoning allowance preservation (P0)

**Requirement.** The treatment arm must not lower the baseline model reasoning ceiling, stop condition, evidence authority, or tool authority. Actual consumption is measured separately.

**Depends on.** ZS-CONTRACT-002

**Theorem/claim obligation.** Core: Harness Capability Superset; Pareto: Reasoning and Effect Floor

**Acceptance test.** Paired manifest checker fails any run with a smaller reasoning or tool contract.

**Current status.** Not implemented / audit required

### ZS-BASE-003 — Fallback reserve (P0)

**Requirement.** Before speculation, reserve sufficient time, token, compute, and external-effect budget to restore or execute the baseline path after reusable speculative work is credited.

**Depends on.** ZS-BASE-001

**Theorem/claim obligation.** Pareto: Baseline-Reserve Feasibility

**Acceptance test.** Injected late failure still permits baseline completion within the bound, or speculation is refused before it begins.

**Current status.** Not implemented / audit required

## Harness adapters

### ZS-ADAPTER-001 — Stable Zero Execute outer tool (P0)

**Requirement.** Expose a small fixed-schema outer operation (conceptually zero.execute) whose tool definition and field order remain stable across tasks. Dynamic project details belong in arguments/results, not in schema mutation.

**Depends on.** ZS-KERNEL-007,ZS-CONTRACT-003

**Theorem/claim obligation.** Core: Fixed Tool ABI Prefix Anchor; Q99: First-Emission Prefix Immunity

**Acceptance test.** Tool schema bytes remain identical across 10,000 randomized tasks; added capabilities are versioned rather than reordered in place.

**Current status.** Not implemented / audit required

### ZS-ADAPTER-002 — Structured task submission (P0)

**Requirement.** Translate harness calls into the canonical Task Contract without loss, implicit defaults, or unrooted side channels.

**Depends on.** ZS-ADAPTER-001,ZS-CONTRACT-001

**Theorem/claim obligation.** Harness: Harness-Transport Factorization

**Acceptance test.** Round-trip fixtures preserve every semantic field across CLI, RPC, native plugin, and optional MCP transports.

**Current status.** Not implemented / audit required

### ZS-ADAPTER-003 — Typed result envelope (P0)

**Requirement.** Return only typed outcomes: Completed, DecisionRequired, EvidenceExpansionRequired, VerificationUnknown, BaselineFallbackRequired, RejectedNoMutation, Cancelled, or FailedNoAuthority.

**Depends on.** ZS-KERNEL-004

**Theorem/claim obligation.** Core: Harness Decision-Boundary Compression

**Acceptance test.** Unknown, timeout, cancellation, and verifier disagreement cannot be rendered as Completed.

**Current status.** Not implemented / audit required

### ZS-ADAPTER-004 — Opaque continuation handles (P0)

**Requirement.** Map a short harness-visible handle to a rooted backend continuation. Handles are tenant/session scoped, revocable, non-authoritative, and resolve only through the backend.

**Depends on.** ZS-KERNEL-002,ZS-SESSION-001

**Theorem/claim obligation.** Core: Stable-Reference Recovery

**Acceptance test.** Forged/cross-tenant/stale handles fail; valid handles resume identical backend state without retransmitting prior project evidence.

**Current status.** Not implemented / audit required

### ZS-ADAPTER-005 — Native-tool coexistence (P0)

**Requirement.** Register ZeroStack alongside, not instead of, ordinary read/search/bash/edit tools. The adapter must not hide or shadow native tools unless the user explicitly configures it.

**Depends on.** ZS-BASE-001

**Theorem/claim obligation.** Core: Harness Capability Superset

**Acceptance test.** Model can select either path in the same session; disabling adapter leaves native functionality unchanged.

**Current status.** Not implemented / audit required

### ZS-ADAPTER-006 — Cancellation and deadline propagation (P1)

**Requirement.** Propagate cancellation, deadlines, and budget changes into backend execution while preserving no-partial-authority semantics.

**Depends on.** ZS-KERNEL-008

**Theorem/claim obligation.** Harness: Atomic No-Partial-Authority

**Acceptance test.** Cancel during read, build, test, verify, and commit; authoritative root remains valid and receipts record cancellation.

**Current status.** Not implemented / audit required

### ZS-ADAPTER-007 — Streaming without transcript flooding (P1)

**Requirement.** Permit progress streaming to the UI or logs while keeping model-visible results bounded and stable. Progress events must not become semantic prompt content unless deliberately promoted.

**Depends on.** ZS-ADAPTER-003

**Theorem/claim obligation.** Q99: CCNF; Core: Stable-Reference Recovery

**Acceptance test.** A long build produces large UI logs but a bounded model result plus exact expansion handle.

**Current status.** Not implemented / audit required

### ZS-ADAPTER-008 — Cross-harness semantic/render split (P1)

**Requirement.** Store harness-independent semantic decision views and derive canonical harness-specific renderings under renderer roots.

**Depends on.** ZS-KERNEL-007,ZS-CONTRACT-003

**Theorem/claim obligation.** Core/Harness: Cross-Harness Semantic Factorization

**Acceptance test.** Switching Pi/Codex/Claude/Cursor adapters reuses semantic roots; only render cache is invalidated.

**Current status.** Not implemented / audit required

### ZS-ADAPTER-009 — Adapter conformance suite (P2)

**Requirement.** Run the same rooted task through every adapter and compare protected outcome, project root, receipts, decision boundaries, and semantic resource projection.

**Depends on.** ZS-ADAPTER-002,ZS-ADAPTER-008

**Theorem/claim obligation.** Harness: Harness-Transport Factorization

**Acceptance test.** All adapters pass golden semantic equivalence; transport overhead differences are reported separately.

**Current status.** Not implemented / audit required

## Session/continuation

### ZS-SESSION-001 — Rooted continuation state (P0)

**Requirement.** Persist task root, project root, evidence roots, decision state, candidate state, verifier state, baseline reserve, and resource ledger under one continuation root.

**Depends on.** ZS-KERNEL-002,ZS-KERNEL-006

**Theorem/claim obligation.** Core: Stable-Reference Recovery

**Acceptance test.** Resume after process restart produces identical state and does not require replaying model-visible history.

**Current status.** Not implemented / audit required

### ZS-SESSION-002 — Continuation state machine (P0)

**Requirement.** Implement explicit states: Bound, Snapshotted, Resolved, DecisionRequired, Planned, Executing, DeltaSealed, Verifying, Authorized, Committed, Restored, Rejected, Unknown, Cancelled.

**Depends on.** ZS-KERNEL-006,ZS-SESSION-001

**Theorem/claim obligation.** Harness: Atomic No-Partial-Authority

**Acceptance test.** Every event has a total allowed-transition check; illegal transitions are rejected and audited.

**Current status.** Not implemented / audit required

### ZS-SESSION-003 — Branching continuations (P1)

**Requirement.** Create child continuations for alternative plans or model candidates without mutating the parent; permit convergence only through rooted equivalence/merge rules.

**Depends on.** ZS-SESSION-001

**Theorem/claim obligation.** Q99: Branch Convergence Reuse

**Acceptance test.** Parallel branches can execute and one verified child can commit; losing branches leave no authority.

**Current status.** Not implemented / audit required

### ZS-SESSION-004 — Continuation compaction without semantic loss (P1)

**Requirement.** Compact internal event logs only after sealing a snapshot/root from which all authoritative state and audit obligations remain recoverable. Never rewrite provider-visible history to achieve backend log compaction.

**Depends on.** ZS-KERNEL-006

**Theorem/claim obligation.** Q99: Retrospective Rewrite Cache-Break Characterization

**Acceptance test.** Replay before/after internal compaction yields identical authoritative state and audit roots.

**Current status.** Not implemented / audit required

## Object store / FSZero

### ZS-STORE-001 — Immutable content-addressed object store (P0)

**Requirement.** Store exact bytes, parsed artifacts, graph fragments, results, receipts, deltas, and views by root. Objects are immutable; updates create new roots.

**Depends on.** ZS-KERNEL-002

**Theorem/claim obligation.** Q99: Causal Cache Soundness

**Acceptance test.** Put/get round trips preserve exact bytes; attempted overwrite under same root is rejected.

**Current status.** Not implemented / audit required

### ZS-STORE-002 — Project snapshots (P0)

**Requirement.** Construct immutable project snapshots covering files, modes, symlinks, relevant metadata, dependency lockfiles, toolchain contracts, and declared external inputs.

**Depends on.** ZS-STORE-001,ZS-CONTRACT-001

**Theorem/claim obligation.** Core: Snap-to-Edit; Q99: causal keys

**Acceptance test.** Changing any covered project input changes snapshot root; excluded metadata is explicitly declared nonsemantic.

**Current status.** Not implemented / audit required

### ZS-STORE-003 — Exact reads and spans (P0)

**Requirement.** Resolve files and exact source spans against a snapshot root, with preimage roots and path normalization.

**Depends on.** ZS-STORE-002

**Theorem/claim obligation.** Core: Stable-Reference Recovery

**Acceptance test.** A span fails rather than silently drifting after source changes; line-ending/path alias variants canonicalize correctly.

**Current status.** Not implemented / audit required

### ZS-STORE-004 — Child sandbox snapshots (P0)

**Requirement.** Create writable child workspaces from an immutable parent without modifying the parent. Track every file, process, network, environment, and generated-output effect.

**Depends on.** ZS-STORE-002

**Theorem/claim obligation.** Harness: Atomic No-Partial-Authority

**Acceptance test.** Sandbox escape, symlink traversal, undeclared network/process access, and parent writes are blocked or produce Unsafe.

**Current status.** Not implemented / audit required

### ZS-STORE-005 — Exact delta sealing (P0)

**Requirement.** After execution, derive the complete canonical delta including generated files, modes, deletions, renames, formatter changes, and external-effect receipts.

**Depends on.** ZS-STORE-004

**Theorem/claim obligation.** Harness: Atomic No-Partial-Authority

**Acceptance test.** Independent rescan matches traced delta; hidden/untracked mutation causes verification failure.

**Current status.** Not implemented / audit required

### ZS-STORE-006 — Atomic compare-and-swap commit (P0)

**Requirement.** Commit only when parent root/epoch, authorized delta root, protected scope, nonce, and lease remain exact.

**Depends on.** ZS-KERNEL-005,ZS-STORE-005

**Theorem/claim obligation.** Harness: Atomic No-Partial-Authority

**Acceptance test.** Two concurrent commits yield one success and one stale-root failure; replayed lease cannot mutate state.

**Current status.** Not implemented / audit required

### ZS-STORE-007 — Integrity scrubbing and replication (P1)

**Requirement.** Periodically verify payload roots, repair from replicas, and distinguish logical L2 availability from physical L3 residency.

**Depends on.** ZS-STORE-001

**Theorem/claim obligation.** Q99: Provider-Controlled Residency Transposition

**Acceptance test.** Injected corruption is detected before reuse; replica recovery preserves the same root.

**Current status.** Not implemented / audit required

### ZS-STORE-008 — Retention and garbage collection (P1)

**Requirement.** Retain objects reachable from live snapshots, continuations, receipts, authorities, and audit policy. GC must be root-safe and lease-aware.

**Depends on.** ZS-STORE-001,ZS-SESSION-001

**Theorem/claim obligation.** Q99: logical versus physical warmness

**Acceptance test.** GC never deletes a reachable object; tombstone/reclamation races are fault-tested.

**Current status.** Not implemented / audit required

### ZS-STORE-009 — Tenant isolation and encryption (P1)

**Requirement.** Namespace, authorize, and encrypt objects so roots or handles cannot leak cross-tenant data. Avoid content-dedup side channels across trust domains unless policy permits.

**Depends on.** ZS-STORE-001

**Theorem/claim obligation.** Systems premise

**Acceptance test.** Cross-tenant handle/root guessing yields no data or timing oracle beyond declared policy.

**Current status.** Not implemented / audit required

## GraphZero / indexing

### ZS-GRAPH-001 — Incremental repository index (P0)

**Requirement.** Maintain language-aware files, syntax trees, symbols, references, imports, calls, tests, configuration, build, generated artifacts, and ownership edges under exact snapshot roots.

**Depends on.** ZS-STORE-002

**Theorem/claim obligation.** Core: Snap-to-Edit

**Acceptance test.** Known benchmark repos reproduce a full-rebuild index exactly after incremental updates.

**Current status.** Not implemented / audit required

### ZS-GRAPH-002 — Causal dependency graph (P0)

**Requirement.** Represent deterministic constructors and every declared dependency: files, environment, tool versions, network fixtures, clocks/randomness policy, generated files, and verifier inputs.

**Depends on.** ZS-GRAPH-001,ZS-KERNEL-003

**Theorem/claim obligation.** Q99: Dependency-Complete Invalidation

**Acceptance test.** Synthetic hidden dependency changes cause a miss/Unknown until edge is added; no object outside closure is invalidated unnecessarily in complete fixtures.

**Current status.** Not implemented / audit required

### ZS-GRAPH-003 — Dependency assurance lattice (P0)

**Requirement.** Grade graph regions as Proved, BoundedComplete, Observed, or Unknown. Only the first two may authorize omission/exact reuse inside their explicit scope.

**Depends on.** ZS-GRAPH-002,ZS-KERNEL-004

**Theorem/claim obligation.** Core: dynamic-dependency epistemic boundary

**Acceptance test.** Observed-only graph never authorizes exact omission; assurance upgrades require evidence and are revocable.

**Current status.** Not implemented / audit required

### ZS-GRAPH-004 — Task-relative causal lens (P0)

**Requirement.** Given task contract and candidate plan, return the demanded evidence/write closure, assurance grade, source roots, tests, interfaces, and unresolved ambiguities.

**Depends on.** ZS-GRAPH-002,ZS-CONTRACT-001

**Theorem/claim obligation.** Core: Snap-to-Edit; Q99: Executable closure

**Acceptance test.** Curated refactor tasks include every actual read/write/test dependency; omitted edge becomes a counterexample.

**Current status.** Not implemented / audit required

### ZS-GRAPH-005 — Executable closure checker (P0)

**Requirement.** Given known values, cached objects, constructors, and demanded outputs, deterministically verify whether all outputs lie in the derivable closure.

**Depends on.** ZS-GRAPH-002

**Theorem/claim obligation.** Q99: Executable Causal-Frontier Sufficiency

**Acceptance test.** Checker agrees with exhaustive evaluation on finite DAG fixtures and rejects missing constructors/inputs.

**Current status.** Not implemented / audit required

### ZS-GRAPH-006 — Dependency-complete invalidation (P0)

**Requirement.** On changed object/contract roots, invalidate the reflexive descendant cone for each authoritative certificate.

**Depends on.** ZS-GRAPH-002

**Theorem/claim obligation.** Q99: Dependency-Complete Invalidation

**Acceptance test.** Random DAG tests show no affected descendant is reused and no unrelated node loses key identity.

**Current status.** Not implemented / audit required

### ZS-GRAPH-007 — Equality-boundary early cutoff (P1)

**Requirement.** Recompute boundary artifacts and stop downstream propagation when all separating boundary roots equal their prior roots.

**Depends on.** ZS-GRAPH-006

**Theorem/claim obligation.** Q99: Equality-Boundary Early Cutoff

**Acceptance test.** Syntactic changes yielding equal boundary values preserve downstream cache; unequal value propagates.

**Current status.** Not implemented / audit required

### ZS-GRAPH-008 — Counterexample-guided graph refinement (P1)

**Requirement.** Capture actual undeclared influences from sandbox traces, add true missing edges, revoke dependent certificates, and retain counterexamples.

**Depends on.** ZS-GRAPH-003,ZS-STORE-004

**Theorem/claim obligation.** Q99: Finite Dependency-Refinement Convergence

**Acceptance test.** Finite fixture converges after at most number of missing true edges under fair exercise.

**Current status.** Not implemented / audit required

### ZS-GRAPH-009 — Cross-branch convergence (P1)

**Requirement.** Share objects across branches when complete causal key and payload root match; branch name alone is not identity.

**Depends on.** ZS-GRAPH-002,ZS-STORE-001

**Theorem/claim obligation.** Q99: Branch Convergence Reuse

**Acceptance test.** Independently built identical branch artifacts deduplicate; different dependencies do not.

**Current status.** Not implemented / audit required

### ZS-GRAPH-010 — Dynamic/runtime dependency adapters (P2)

**Requirement.** Instrument build systems, test runners, interpreters, compilers, package managers, and selected runtime probes to discover nonstatic dependencies.

**Depends on.** ZS-GRAPH-008

**Theorem/claim obligation.** Systems premise

**Acceptance test.** Declared bounded domains reach BoundedComplete under adversarial dynamic access tests.

**Current status.** Not implemented / audit required

## TokenZero / decision views

### ZS-VIEW-001 — Semantic decision view (P0)

**Requirement.** Construct a harness-independent rooted object containing exact relevant facts, protected uncertainty, candidate choices, evidence roots, and expansion authorities.

**Depends on.** ZS-GRAPH-004

**Theorem/claim obligation.** Core: Protected Decision-View Minimality

**Acceptance test.** Finite decision-family checker confirms merged states induce identical protected next decisions.

**Current status.** Not implemented / audit required

### ZS-VIEW-002 — Stable capsule normal form (P0)

**Requirement.** Represent large artifacts as causal key + formation receipt + payload root + stable capsule + expansion handle at first model exposure.

**Depends on.** ZS-KERNEL-003,ZS-STORE-001

**Theorem/claim obligation.** Q99: Causal Cache Normal Form

**Acceptance test.** Later expansion appends a new rooted result and never rewrites the previously emitted capsule.

**Current status.** Not implemented / audit required

### ZS-VIEW-003 — Exact expansion (P0)

**Requirement.** Resolve every omitted fact through a live rooted handle or return Unknown; never fabricate a summary-only recovery path.

**Depends on.** ZS-VIEW-002

**Theorem/claim obligation.** Core: Stable-Reference Recovery

**Acceptance test.** Expansion returns exact bytes/facts for original root; stale root is explicit.

**Current status.** Not implemented / audit required

### ZS-VIEW-004 — Canonical provider rendering (P0)

**Requirement.** Render stable system/tool/schema blocks, task arguments, views, references, and results deterministically for each adapter/model contract.

**Depends on.** ZS-KERNEL-001,ZS-CONTRACT-003,ZS-ADAPTER-003

**Theorem/claim obligation.** Core: Canonical Rendering Identity; Q99: Rendering Fragmentation Penalty

**Acceptance test.** Repeated rendering under randomized process/map order is byte-identical.

**Current status.** Not implemented / audit required

### ZS-VIEW-005 — Append-only model history policy (P0)

**Requirement.** Never retrospectively replace provider-visible prior messages or tool results. Evidence refinement appends a new message/reference under a new root.

**Depends on.** ZS-VIEW-002,ZS-VIEW-004

**Theorem/claim obligation.** Q99: First-Emission Prefix Immunity

**Acceptance test.** Longest common prefix of successive histories equals full previous history except provider/harness-required dynamic envelope fields explicitly excluded from the cache key.

**Current status.** Not implemented / audit required

### ZS-VIEW-006 — Stable capsule admission policy (P1)

**Requirement.** Estimate raw-vs-capsule horizon cost, expansion probability, handling cost, and complete resource vector before choosing indirection.

**Depends on.** ZS-METRIC-001

**Theorem/claim obligation.** Q99: Stable-Capsule Admission

**Acceptance test.** Replay data predicts which representation wins; policy can be disabled when expansion burden dominates.

**Current status.** Not implemented / audit required

### ZS-VIEW-007 — Volatility-aware commutative prefix ordering (P1)

**Requirement.** Inside semantically commutative blocks, order long/stable material before volatile material according to measured survival score; never reorder noncommutative messages.

**Depends on.** ZS-VIEW-004

**Theorem/claim obligation.** Q99: Expected Prefix-Survival Ordering

**Acceptance test.** Exhaustive small permutations confirm maximum expected prefix survival; semantic-order tests prevent illegal reordering.

**Current status.** Not implemented / audit required

### ZS-VIEW-008 — Per-model token accounting (P1)

**Requirement.** Count rendered tokens with the provider/model tokenizer when available and retain byte counts when not. Bind counting method/version in ledgers.

**Depends on.** ZS-CONTRACT-002

**Theorem/claim obligation.** Pareto: complete resource vector

**Acceptance test.** Golden prompt fixtures match provider usage within documented accounting boundaries.

**Current status.** Not implemented / audit required

### ZS-VIEW-009 — Decision-view sufficiency check (P1)

**Requirement.** For finite or verifier-covered domains, prove the complete compatibility set maps to one protected decision; otherwise return Unknown/expand.

**Depends on.** ZS-VIEW-001,ZS-KERNEL-004

**Theorem/claim obligation.** Core: Stable-View Sufficiency

**Acceptance test.** Two compatible hidden states with distinct protected decisions always invalidate the view.

**Current status.** Not implemented / audit required

## Private composition / Code Mode

### ZS-EXEC-001 — Mechanical operation graph (P0)

**Requirement.** Represent read/search/index/build/test/format/analyze operations and their dependencies as a rooted DAG. Mark any node requiring model semantics as a decision boundary.

**Depends on.** ZS-CONTRACT-001

**Theorem/claim obligation.** Core: Harness Decision-Boundary Compression

**Acceptance test.** Baseline traces can be annotated and operation graph never crosses a labeled semantic decision without a supplied contingent policy.

**Current status.** Not implemented / audit required

### ZS-EXEC-002 — Private executor (P0)

**Requirement.** Execute eligible operation DAG nodes without inserting intermediate results into model history; retain exact artifacts and receipts for expansion/audit.

**Depends on.** ZS-EXEC-001,ZS-STORE-004

**Theorem/claim obligation.** Harness: Decision-Preserving Private Composition

**Acceptance test.** Model receives same protected decision information as primitive trace on fixture tasks; all intermediates remain recoverable.

**Current status.** Not implemented / audit required

### ZS-EXEC-003 — Decision boundary return (P0)

**Requirement.** Stop and return DecisionRequired when an unresolved semantic branch appears; include exact alternatives/evidence and continuation handle.

**Depends on.** ZS-EXEC-001,ZS-ADAPTER-003

**Theorem/claim obligation.** Core: Adaptive Decision-Round Lower Bound

**Acceptance test.** Adversarial tasks requiring k adaptive decisions produce at least k returns unless model supplied a total contingent policy.

**Current status.** Not implemented / audit required

### ZS-EXEC-004 — Contingent policy execution (P1)

**Requirement.** Accept a model-supplied finite typed policy over reachable observation classes; verify every action before execution and return on unhandled class.

**Depends on.** ZS-EXEC-001

**Theorem/claim obligation.** Harness: Contingent-Policy Call Compression

**Acceptance test.** Policy-covered branches stay within one call; injected unhandled observation returns DecisionRequired.

**Current status.** Not implemented / audit required

### ZS-EXEC-005 — Critical-path scheduler (P1)

**Requirement.** Parallelize independent operations while preserving deterministic semantic outputs and accounting actual work versus model-visible rounds.

**Depends on.** ZS-EXEC-001

**Theorem/claim obligation.** Core: Mechanical Critical-Path Compression

**Acceptance test.** Parallel schedule matches sequential rooted outputs and reduces wall-clock only where dependencies permit.

**Current status.** Not implemented / audit required

### ZS-EXEC-006 — Tool adapter registry (P1)

**Requirement.** Wrap filesystem, search, AST, build, test, package, static-analysis, profiler, and external-service tools under rooted contracts and side-effect policies.

**Depends on.** ZS-CONTRACT-003,ZS-STORE-004

**Theorem/claim obligation.** Systems premise

**Acceptance test.** Version/tool change invalidates dependent artifacts; undeclared effect is Unsafe.

**Current status.** Not implemented / audit required

## Verification

### ZS-VERIFY-001 — Verifier registry (P0)

**Requirement.** Bind verifier identity/version, scope, input roots, result, evidence, runtime, and confidence/assurance grade.

**Depends on.** ZS-CONTRACT-004,ZS-KERNEL-003

**Theorem/claim obligation.** Core: Guarded publication

**Acceptance test.** Changing verifier version or scope invalidates authority.

**Current status.** Not implemented / audit required

### ZS-VERIFY-002 — Current-effect verification (P0)

**Requirement.** Check build/tests/specification/static/security/performance obligations against the exact candidate delta and sandbox root.

**Depends on.** ZS-STORE-005,ZS-VERIFY-001

**Theorem/claim obligation.** Harness: Atomic No-Partial-Authority

**Acceptance test.** Substituting a different delta after verification fails authority validation.

**Current status.** Not implemented / audit required

### ZS-VERIFY-003 — Successor-state verification (P0)

**Requirement.** Check that the committed state preserves required future interfaces/invariants or remains in the declared simulation relation, not merely that current tests pass.

**Depends on.** ZS-VERIFY-002

**Theorem/claim obligation.** Core: guarded nonregression

**Acceptance test.** A locally passing edit that breaks a registered future action is rejected in a fixture.

**Current status.** Not implemented / audit required

### ZS-VERIFY-004 — Equivalence/dominance verdict (P0)

**Requirement.** Return Equivalent, Dominates, Reject, or Unknown under protected scope. Unknown cannot be promoted by planner confidence.

**Depends on.** ZS-VERIFY-001,ZS-KERNEL-004

**Theorem/claim obligation.** Pareto: Same-Harness Capability Superset

**Acceptance test.** Verifier timeout/disagreement/uncovered dimension yields Unknown.

**Current status.** Not implemented / audit required

### ZS-VERIFY-005 — Authority lease issuance (P0)

**Requirement.** Issue execution/commit authority only after verifying roots, scope, epoch, delta, verifier receipts, resource reserve, expiry, nonce, and caller identity.

**Depends on.** ZS-KERNEL-005,ZS-VERIFY-004

**Theorem/claim obligation.** Q99: Lease-Revocation Safety; Harness: Atomic No-Partial-Authority

**Acceptance test.** Replay/expiry/root mismatch/scope mismatch all fail before mutation.

**Current status.** Not implemented / audit required

### ZS-VERIFY-006 — Human/user acceptance gate (P1)

**Requirement.** Represent subjective or policy decisions as explicit DecisionRequired/UserApproval rather than pretending automated equivalence.

**Depends on.** ZS-CONTRACT-004,ZS-ADAPTER-003

**Theorem/claim obligation.** Epistemic safety premise

**Acceptance test.** Greenfield/UX task cannot auto-claim subjective superiority without declared evaluator.

**Current status.** Not implemented / audit required

### ZS-VERIFY-007 — Proof/evidence cache (P1)

**Requirement.** Cache verifier subresults under complete causal keys; invalidate on code, test, tool, environment, verifier, or scope changes and use equality early cutoff.

**Depends on.** ZS-KERNEL-003,ZS-GRAPH-006

**Theorem/claim obligation.** Q99 causal reuse

**Acceptance test.** Changed unrelated file preserves proof object; changed premise invalidates it.

**Current status.** Not implemented / audit required

## Causal cache / Q99

### ZS-CACHE-001 — Three-layer cache accounting (P0)

**Requirement.** Measure L1 provider prefix cache, L2 logical causal-object reuse, and L3 physical residency separately.

**Depends on.** ZS-METRIC-001

**Theorem/claim obligation.** Q99: Three cache layers

**Acceptance test.** Forced L1 eviction with valid L2 produces provider miss but project logical hit; cold L3 is not mislabeled as L2 miss.

**Current status.** Not implemented / audit required

### ZS-CACHE-002 — Provider cache telemetry (P0)

**Requirement.** Capture cached-read, cache-write, uncached input, eligible prefix, route/model, TTL mode, cache key, and time-to-first-token when provider exposes them.

**Depends on.** ZS-CONTRACT-002

**Theorem/claim obligation.** Q99: Provider-Controlled Residency Transposition

**Acceptance test.** Recorded values reconcile with provider usage response; missing fields are Unknown, not zero.

**Current status.** Not implemented / audit required

### ZS-CACHE-003 — Causal reuse telemetry (P0)

**Requirement.** For every request, record demanded weighted object set, valid reused set, invalid set, recomputed set, early-cutoff set, and reason codes.

**Depends on.** ZS-GRAPH-004,ZS-GRAPH-006

**Theorem/claim obligation.** Q99: Weighted Causal Q99

**Acceptance test.** Mass accounting sums exactly and cannot count Unknown/invalid objects as hits.

**Current status.** Not implemented / audit required

### ZS-CACHE-004 — Windowed Q99 service metric (P0)

**Requirement.** Compute task-weighted reuse in declared sliding windows and report lower windows, post-change recovery, and impossibility events.

**Depends on.** ZS-CACHE-003

**Theorem/claim obligation.** Q99: Windowed Q99; Adversarial Q99 Impossibility

**Acceptance test.** Central change exceeding 1% mass is reported as Q99 unavailable until threshold work is resolved.

**Current status.** Not implemented / audit required

### ZS-CACHE-005 — Time/work to Q99 (P1)

**Requirement.** Track invalid mass resolved over time and compute exact remaining mass required to restore 99% valid reuse.

**Depends on.** ZS-CACHE-003

**Theorem/claim obligation.** Q99: Q99 Restoration Threshold

**Acceptance test.** Synthetic mutations match formula and never count unresolved mass as hit.

**Current status.** Not implemented / audit required

### ZS-CACHE-006 — Rewrite crossover estimator (P1)

**Requirement.** Estimate first-use write cost, cached-read cost, suffix size, compaction cost, and remaining reuse horizon before any retrospective representation change. Default remains first-emission stable capsules.

**Depends on.** ZS-CACHE-002

**Theorem/claim obligation.** Q99: Cache-Compaction Crossover; Horizon-Aware Rewrite Break-Even

**Acceptance test.** Replay experiment predicts observed cost sign across providers/TTLs; no historical rewrite occurs in normal CCNF mode.

**Current status.** Not implemented / audit required

### ZS-CACHE-007 — Cache frontier planner (P1)

**Requirement.** Propose resident/materialized objects under storage, latency, and invalidation budgets using exact object costs and demand forecasts.

**Depends on.** ZS-GRAPH-005,ZS-METRIC-001

**Theorem/claim obligation.** Q99: Tree-Optimal Materialization; Frontier Hardness

**Acceptance test.** Tree cases match DP optimum; general proposals pass independent capacity and closure checks even when heuristic.

**Current status.** Not implemented / audit required

### ZS-CACHE-008 — Residency policy and prefetch (P1)

**Requirement.** Separate logical validity from hot placement; prefetch high-hazard/high-demand closures without granting semantic authority based on prediction.

**Depends on.** ZS-CACHE-001,ZS-CACHE-007

**Theorem/claim obligation.** Q99: Causal Hazard Bound

**Acceptance test.** Prediction errors alter latency/cost only; invalid object is never served as valid.

**Current status.** Not implemented / audit required

### ZS-CACHE-009 — Q99 statistical certification (P2)

**Requirement.** Compute exact/cluster-aware confidence intervals for hit rate and zero-failure bounds; do not infer universal Q99 from a handful of warm traces.

**Depends on.** ZS-CACHE-004

**Theorem/claim obligation.** Q99: Zero-Failure Q99 Sample Bound

**Acceptance test.** 299 zero-miss independent trials are required for 99% at 95% one-sided support; clustered report uses stricter effective sample.

**Current status.** Not implemented / audit required

## Accounting / Pareto

### ZS-METRIC-001 — Complete resource ledger (P0)

**Requirement.** Record calls, tool args/returns, uncached input, cache reads/writes, reasoning, visible output, wire/disk bytes, CPU/GPU, verification, latency, storage, preparation, maintenance, failed speculation, and restoration.

**Depends on.** ZS-KERNEL-006

**Theorem/claim obligation.** Pareto: Complete Resource Vector

**Acceptance test.** Ledger reconciles to provider bills/system meters within declared tolerances; no hidden uncharged worker.

**Current status.** Not implemented / audit required

### ZS-METRIC-002 — Paired baseline manifest (P0)

**Requirement.** Run same model, harness, prompt, reasoning allowance, native tools, initial root, acceptance criteria, and meter scope with and without optional ZeroStack.

**Depends on.** ZS-CONTRACT-001,ZS-CONTRACT-002,ZS-CONTRACT-003

**Theorem/claim obligation.** Evaluation: paired design

**Acceptance test.** Manifest diff must be empty outside treatment availability and randomized run metadata.

**Current status.** Not implemented / audit required

### ZS-METRIC-003 — Separate cache metrics (P0)

**Requirement.** Report provider prefix ratio, L2 valid causal reuse, L3 hot residency, Boundary-Q99, complete-work ratio, quality regression, strict rescue, and gap closure independently.

**Depends on.** ZS-CACHE-001,ZS-METRIC-001

**Theorem/claim obligation.** Pareto: Q99 Non-Substitutability

**Acceptance test.** Dashboard/API prevents one metric from being labeled as another.

**Current status.** Not implemented / audit required

### ZS-METRIC-004 — Multi-resource feasibility solver (P0)

**Requirement.** Given baseline/hit/fallback/preparation intervals and per-coordinate targets, compute exact feasible hit-rate intersection or blocker.

**Depends on.** ZS-METRIC-001

**Theorem/claim obligation.** Pareto: Multi-Resource Pareto Feasibility; Robust Certificate

**Acceptance test.** Exact rational tests match exhaustive grid; reports coordinate causing impossibility.

**Current status.** Not implemented / audit required

### ZS-METRIC-005 — Index amortization (P1)

**Requirement.** Allocate indexing/object-formation cost over actual reuse campaign and compute strict/Q99 break-even horizons.

**Depends on.** ZS-METRIC-001

**Theorem/claim obligation.** Pareto: Campaign Break-Even

**Acceptance test.** No warm-run claim omits cold build; denominator nonpositive returns Impossible.

**Current status.** Not implemented / audit required

### ZS-METRIC-006 — Frontier Closure decomposition (P1)

**Requirement.** Compute normalized preparation, prepared-path, and novelty/fallback terms and identify the largest limiting burden.

**Depends on.** ZS-METRIC-001

**Theorem/claim obligation.** Pareto: Frontier Closure

**Acceptance test.** Terms are nonnegative and sum to complete optimized/baseline ratio.

**Current status.** Not implemented / audit required

### ZS-METRIC-007 — Certified lower-bound ledger (P1)

**Requirement.** Store disjoint charging maps for unavoidable request information, decisions, reasoning, verification, output, and effects.

**Depends on.** ZS-METRIC-001

**Theorem/claim obligation.** Pareto: Disjoint Lower-Bound Composition

**Acceptance test.** Overlap checker rejects double counting; closure Gamma never exceeds 1 under valid data.

**Current status.** Not implemented / audit required

### ZS-METRIC-008 — Compression-dividend budget (P1)

**Requirement.** Expose verified savings that may fund extra candidates/tests/proofs/reasoning without exceeding baseline resource vector.

**Depends on.** ZS-METRIC-004

**Theorem/claim obligation.** Pareto: Compression-Dividend Reinvestment

**Acceptance test.** Augmentation scheduler refuses componentwise budget violation; strict rescue remains separately certified.

**Current status.** Not implemented / audit required

### ZS-METRIC-009 — Capability asset lifetime ledger (P2)

**Requirement.** Track capture/proof cost, reuse count, saved work, maintenance, invalidation hazard, strict rescue, and retirement decision.

**Depends on.** ZS-METRIC-001

**Theorem/claim obligation.** Pareto: Capability-Asset Lifetime Value

**Acceptance test.** Assets with negative measured expected lifetime value are demoted/retired; invalid assets lose authority immediately.

**Current status.** Not implemented / audit required

## Verified capability

### ZS-CAP-001 — Episode capture (P1)

**Requirement.** After a successful task, propose exact evidence plans, causal lenses, operators, tests, failure syndromes, and decision capsules with source episode roots.

**Depends on.** ZS-KERNEL-003

**Theorem/claim obligation.** Harness: Verified backend learning

**Acceptance test.** Captured artifact is nonauthoritative until separately proved.

**Current status.** Not implemented / audit required

### ZS-CAP-002 — Scope and precondition proof (P1)

**Requirement.** Bind every capability to repository/contract roots, explicit preconditions, reads, writes, postconditions, verifier, rollback, and invalidation dependencies.

**Depends on.** ZS-CAP-001,ZS-GRAPH-002

**Theorem/claim obligation.** Core: Stable-View Sufficiency

**Acceptance test.** Similar but out-of-scope task does not match; changed dependency invalidates capability.

**Current status.** Not implemented / audit required

### ZS-CAP-003 — Shadow-mode promotion (P1)

**Requirement.** Evaluate proposed capabilities alongside baseline before granting execution authority; require no protected regression under declared release gate.

**Depends on.** ZS-CAP-002,ZS-METRIC-002

**Theorem/claim obligation.** Pareto: capability/nonregression

**Acceptance test.** Promotion report includes misses, regressions, strict rescues, and complete cost.

**Current status.** Not implemented / audit required

### ZS-CAP-004 — Failure syndrome store (P1)

**Requirement.** Retain proven-invalid approaches under exact domain/scope and use them only to prune branches covered by the proof.

**Depends on.** ZS-CAP-001

**Theorem/claim obligation.** Negative-transfer firewall principle

**Acceptance test.** Out-of-scope syndrome cannot suppress candidate; in-scope invalid candidate is rejected with evidence.

**Current status.** Not implemented / audit required

### ZS-CAP-005 — Revocation/revalidation (P2)

**Requirement.** Invalidate assets on dependency, contract, verifier, epoch, or policy changes; permit audit storage but remove execution authority until revalidated.

**Depends on.** ZS-CAP-002

**Theorem/claim obligation.** Q99: Lease-Revocation Safety

**Acceptance test.** Expired/revoked asset increases cost but cannot worsen protected result.

**Current status.** Not implemented / audit required

## Security

### ZS-SEC-001 — Sandbox confinement (P0)

**Requirement.** Enforce filesystem namespace, process, network, environment, secret, device, and resource policies. Root all allowed external fixtures.

**Depends on.** ZS-STORE-004

**Theorem/claim obligation.** Systems premise

**Acceptance test.** Adversarial symlink, path traversal, subprocess, socket, environment, and secret-access tests fail closed.

**Current status.** Not implemented / audit required

### ZS-SEC-002 — Receipt and object poisoning defense (P0)

**Requirement.** Verify roots, formation, producer, protected scope, and canonical encoding before cache reuse. When multi-tenant operation is enabled, additionally bind and verify tenant identity.

**Depends on.** ZS-KERNEL-003,ZS-STORE-001,ZS-CONTRACT-004

**Theorem/claim obligation.** Q99: Causal Cache Soundness

**Acceptance test.** Poisoned object/key and metadata tampering are rejected; cross-tenant substitution is additionally rejected when multi-tenant mode is enabled.

**Current status.** Not implemented / audit required

### ZS-SEC-003 — Authority replay protection (P0)

**Requirement.** Use nonces, expiry, root/epoch binding, single-use or monotone commit semantics, and audit trail.

**Depends on.** ZS-VERIFY-005

**Theorem/claim obligation.** Q99: Lease-Revocation Safety

**Acceptance test.** Captured authority cannot be replayed after commit, rollback, branch switch, or epoch change.

**Current status.** Not implemented / audit required

### ZS-SEC-004 — Secret-safe rendering and logs (P1)

**Requirement.** Redact or reference secrets before model-visible rendering and public traces while retaining verifiable private receipts under policy.

**Depends on.** ZS-VIEW-004,ZS-KERNEL-006

**Theorem/claim obligation.** Systems premise

**Acceptance test.** Secret fixtures never appear in provider prompt, UI export, benchmark trace, or error string.

**Current status.** Not implemented / audit required

## Operations

### ZS-OPS-001 — Crash recovery (P0)

**Requirement.** Replay authoritative log, reconcile in-flight sandboxes/leases, and either complete a previously committed transaction or restore no-authority state.

**Depends on.** ZS-KERNEL-006,ZS-KERNEL-008

**Theorem/claim obligation.** Runtime refinement

**Acceptance test.** Kill/restart at every lifecycle state passes deterministic recovery fixtures.

**Current status.** Not implemented / audit required

### ZS-OPS-002 — Concurrency control (P1)

**Requirement.** Support concurrent reads, branches, index updates, and candidates with snapshot isolation and root/epoch CAS at authority.

**Depends on.** ZS-STORE-006,ZS-SESSION-003

**Theorem/claim obligation.** Harness: Atomic No-Partial-Authority

**Acceptance test.** Race tests preserve serializable authoritative roots; stale readers are explicit.

**Current status.** Not implemented / audit required

### ZS-OPS-003 — Observability and trace export (P1)

**Requirement.** Export privacy-safe rooted traces, decision-boundary annotations, cache events, invalidation reasons, verification outcomes, and resource ledgers.

**Depends on.** ZS-KERNEL-006,ZS-METRIC-001

**Theorem/claim obligation.** Evaluation paper

**Acceptance test.** Every benchmark result is reproducible from sealed manifests/ledgers or marked nonreproducible.

**Current status.** Not implemented / audit required

### ZS-OPS-004 — Schema and object migration (P1)

**Requirement.** Migrate semantic objects through explicit deterministic transformations with old/new roots and validation receipts; retain compatibility or fail closed.

**Depends on.** ZS-KERNEL-007

**Theorem/claim obligation.** Core: semantic factorization

**Acceptance test.** Migration round trip/golden fixtures preserve meaning and invalidate only renderer/contract layers required.

**Current status.** Not implemented / audit required

### ZS-OPS-005 — Distributed worker trust (P2)

**Requirement.** Treat workers as untrusted producers: accept outputs only after root, formation, sandbox, and verifier checks.

**Depends on.** ZS-KERNEL-003,ZS-VERIFY-001

**Theorem/claim obligation.** Proof-carrying producer/checker pattern

**Acceptance test.** Malicious worker output cannot acquire cache or commit authority.

**Current status.** Not implemented / audit required

## Benchmarks

### ZS-BENCH-001 — Task manifest schema (P0)

**Requirement.** Publish machine-readable manifests for explanation, refactor, port, and greenfield tasks with protected criteria and meter scope.

**Depends on.** ZS-CONTRACT-001

**Theorem/claim obligation.** Evaluation paper

**Acceptance test.** Schema validation and root sealing pass for every released task.

**Current status.** Not implemented / audit required

### ZS-BENCH-002 — Decision-boundary annotation (P0)

**Requirement.** Double-annotate baseline traces into semantic decisions and mechanical segments using a blinded rubric.

**Depends on.** ZS-EXEC-001

**Theorem/claim obligation.** Core: Decision-Boundary Compression

**Acceptance test.** Inter-annotator agreement and adjudications are published; one/two-call claims match irreducible count.

**Current status.** Not implemented / audit required

### ZS-BENCH-003 — Prefix policy trial (P0)

**Requirement.** Compare raw-retained, retrospective-rewrite, and stable-capsule append-only histories under fixed provider prefix variables.

**Depends on.** ZS-VIEW-005,ZS-CACHE-002

**Theorem/claim obligation.** Q99: rewrite/crossover/prefix immunity

**Acceptance test.** Report LCP, cached/uncached/write tokens, cost, latency, quality, expansions.

**Current status.** Not implemented / audit required

### ZS-BENCH-004 — Causal invalidation trial (P0)

**Requirement.** Inject leaf, central, semantic-no-op, toolchain, environment, and branch changes; measure invalidated/recomputed mass and soundness. Record early-cutoff mass when the optional early-cutoff subsystem is enabled.

**Depends on.** ZS-GRAPH-006

**Theorem/claim obligation.** Q99 causal theorem family

**Acceptance test.** No stale reuse; blast radius matches complete dependency model.

**Current status.** Not implemented / audit required

### ZS-BENCH-005 — Provider-miss insulation trial (P0)

**Requirement.** Warm both caches, independently force provider TTL/eviction/routing/model-key misses while preserving L2, and measure rediscovery avoided.

**Depends on.** ZS-CACHE-001

**Theorem/claim obligation.** Q99: Provider-Miss Insulation

**Acceptance test.** Provider miss reprocesses compact view but does not relist/reread/reindex unchanged project.

**Current status.** Not implemented / audit required

### ZS-BENCH-006 — Transactional fault matrix (P0)

**Requirement.** Inject crashes, cancellation, stale roots, undeclared effects, verifier timeout/disagreement, storage corruption, and CAS races.

**Depends on.** ZS-KERNEL-008,ZS-SEC-001

**Theorem/claim obligation.** Harness: Atomic No-Partial-Authority

**Acceptance test.** Every case yields verified successor or exact no-mutation.

**Current status.** Not implemented / audit required

### ZS-BENCH-007 — Windowed Q99 and recovery (P1)

**Requirement.** Measure reuse across sliding windows and time/work-to-Q99 after controlled changes.

**Depends on.** ZS-CACHE-004,ZS-CACHE-005

**Theorem/claim obligation.** Q99: Q99 Restoration Threshold

**Acceptance test.** Reports impossibility intervals rather than averaging them away.

**Current status.** Not implemented / audit required

### ZS-BENCH-008 — Paired quality evaluation (P1)

**Requirement.** Blindly compare same-model/same-harness outputs for regression, strict rescue, factual support, test/build results, and subjective dimensions.

**Depends on.** ZS-METRIC-002

**Theorem/claim obligation.** Pareto: Same-Harness Capability Superset

**Acceptance test.** Treatment cannot claim no degradation outside verifier/human scope.

**Current status.** Not implemented / audit required

### ZS-BENCH-009 — Release claim gate (P1)

**Requirement.** Automate checks for paper theorem status, current-provider fact date, citation resolution, no unsupported Q99 substitution, benchmark evidence, checksums, and negative results.

**Depends on.** ZS-METRIC-003

**Theorem/claim obligation.** Draft 5 public-release gates

**Acceptance test.** Release fails when any required artifact or claim scope is absent.

**Current status.** Not implemented / audit required

## Required implementation phases

### Phase 0 — Audit and instrumentation

Map every existing module to this document; add canonical roots, event logging, task contracts, paired ledgers, trivalent verdicts, and native fallback observability. No optimization claim is enabled.

### Phase 1 — Exact read-only Zero Execute

Implement rooted snapshots, exact reads/spans, repository index, task-relative causal lenses, stable decision views, continuation handles, canonical rendering, and provider/L2/L3 metrics. Support program explanation and repository orientation without mutation.

### Phase 2 — Transactional project effects

Add child sandboxes, effect tracing, exact delta sealing, verifier registry, authority leases, successor checks, CAS commit, rollback, crash recovery, and the full fault matrix. Only then enable edit/refactor/port/build workflows.

### Phase 3 — Durable causal caching and Q99

Enable formation-receipted derived objects, dependency-complete invalidation, early cutoff, provider-miss insulation, stable capsule admission, windowed Q99, time-to-Q99, frontier planning, and robust Pareto accounting.

### Phase 4 — One-/two-call private composition

Compile primitive operations into mechanical DAGs, identify decision boundaries, execute private segments, support contingent policies, preserve exact expansions, and validate one-/two-call claims against double-annotated baseline traces.

### Phase 5 — Verified backend learning

Capture scoped operators, evidence plans, proof/test assets, and failure syndromes. Require shadow-mode promotion, lifetime-value accounting, invalidation, revocation, and native baseline retention.

### Phase 6 — Cross-harness and distributed maturity

Ship multiple faithful adapters, semantic/render cache factorization, distributed untrusted workers, replication, tenant isolation, schema migration, public trace/ledger tooling, and release-grade benchmarks.

## Definition of an implementation-complete theorem

A theorem is implementation-complete only when the repository contains: (1) rooted evidence constructors; (2) a deterministic total checker; (3) a machine-revalidatable certificate; (4) a narrow authority consequence; (5) explicit Unknown/fallback behavior; (6) a checker cost bound or measurement; (7) a direct falsifier; and (8) a conformance test proving the concrete transition refines the abstract theorem.


---

## SOURCE: `archive/extracted/Draft5/claims/CLAIM_LEDGER_DRAFT5.md`

# Draft 5 Theorem, Claim, and Falsifier Ledger

**Author:** Aditya G  
**Date:** 13 August 2026

Premise classes: **M** = mathematical; **S** = receipt-backed systems; **E** = empirical/statistical evidence. A mathematically valid conditional theorem does not establish that the real implementation satisfies its systems premises.

| ID | Document | Claim | Status | Premises | Implementation | Direct falsifier |
|---|---|---|---|---|---|---|
| D5-C01 | RACC Core | Protected Decision-View Minimality | Proved in finite stated model | M | ZS-VIEW-009 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-C02 | RACC Core | Harness Decision-Boundary Compression | Conditional theorem | M+S | ZS-EXEC-001,ZS-EXEC-002,ZS-EXEC-003 | A private segment changes the next protected decision or performs an unauthorized effect. |
| D5-C03 | RACC Core | Mechanical Critical-Path Compression | Conditional theorem | M+S | ZS-EXEC-005 | The claimed DAG omits a dependency/decision or observed span exceeds the certified operation graph. |
| D5-C04 | RACC Core | Adaptive Decision-Round Lower Bound | Proved in finite communication model | M | ZS-EXEC-003 | Backend commits an observation-contingent semantic choice without prior policy/verifier or model return. |
| D5-C05 | RACC Core | One-/Two-Call Normal Form | Proved in stated model | M | ZS-BENCH-002 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-C06 | RACC Core | Boundary-Q99 Threshold | Proved arithmetic | M | ZS-METRIC-003 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-C07 | RACC Core | Stable-Reference Recovery | Conditional theorem | M+S | ZS-VIEW-003,ZS-SESSION-001 | Expansion fails to reproduce the exact object bound by the handle. |
| D5-C08 | RACC Core | Harness Capability Superset | Conditional theorem | M+S | ZS-BASE-001,ZS-BASE-002 | A native tool, reasoning setting, or baseline path becomes unavailable. |
| D5-C09 | RACC Core | Finite Signature-Quotient Synthesis | Proved in finite stated model | M | ZS-VIEW-001 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-C10 | RACC Core | Decision-View Certificate Soundness | Conditional theorem | M+S | ZS-VIEW-009 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-C11 | RACC Core | Stable-View Sufficiency | Proved in stated model | M | ZS-VIEW-009 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-C12 | RACC Core | Monotonicity under exact refinement | Proved | M | See implementation contract | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-H01 | RACC-Harness | Harness-Transport Factorization | Conditional theorem | M+S | ZS-ADAPTER-009 | Faithful adapters produce different protected semantic transitions for the same canonical task. |
| D5-H02 | RACC-Harness | Cross-Harness Semantic Cache Reuse | Conditional theorem | M+S | ZS-ADAPTER-008,ZS-ADAPTER-009 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-H03 | RACC-Harness | Decision-Preserving Private Composition | Conditional theorem | M+S | ZS-EXEC-002 | An intermediate hidden result would change a protected model decision. |
| D5-H04 | RACC-Harness | Atomic No-Partial-Authority | Conditional theorem | M+S | ZS-KERNEL-008,ZS-STORE-006 | Crash/race leaves partial authoritative mutation. |
| D5-H05 | RACC-Harness | Contingent-Policy Call Compression | Conditional theorem | M+S | ZS-EXEC-004 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q01 | RACC-Q99 | Retrospective Rewrite Cache-Break Characterization | Proved | M | ZS-BENCH-003 | Measured longest-common-prefix or rewritten residual differs from the exact characterization. |
| D5-Q02 | RACC-Q99 | Cache-Compaction Crossover | Proved | M | ZS-CACHE-006 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q03 | RACC-Q99 | Horizon-Aware Rewrite Break-Even | Proved | M | ZS-CACHE-006 | Direct horizon costs disagree with the inequality or minimal threshold. |
| D5-Q04 | RACC-Q99 | First-Emission Prefix Immunity | Proved | M | ZS-VIEW-005 | Expansion modifies an earlier provider-visible capsule. |
| D5-Q05 | RACC-Q99 | Zero Self-Induced Historical Rewrite Cost | Proved under append-only premise | M | ZS-VIEW-005 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q06 | RACC-Q99 | Stable-Capsule Admission | Proved in cost model | M | ZS-VIEW-006 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q07 | RACC-Q99 | Two-Plane Cache Factorization | Proved in stated cost model | M | ZS-CACHE-001 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q08 | RACC-Q99 | Provider-Miss Insulation | Proved in stated cost model | M | ZS-BENCH-005 | L1 miss forces unchanged project rediscovery despite valid retained L2 objects. |
| D5-Q09 | RACC-Q99 | Provider-Controlled Residency Transposition | Conditional theorem | M+S | ZS-CACHE-001,ZS-STORE-007 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q10 | RACC-Q99 | Causal Cache Soundness | Conditional theorem | M+S | ZS-KERNEL-003 | Accepted identical causal key/receipt yields a different exact payload. |
| D5-Q11 | RACC-Q99 | Dependency-Complete Invalidation | Conditional theorem | M+S | ZS-GRAPH-006 | A reused object depends on a changed premise outside the invalidated closure. |
| D5-Q12 | RACC-Q99 | Equality-Boundary Early Cutoff | Conditional theorem | M+S | ZS-GRAPH-007 | Downstream exact value changes despite every separating boundary root remaining equal. |
| D5-Q13 | RACC-Q99 | Tree-Optimal Causal Materialization | Proved for finite trees | M | ZS-CACHE-007 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q14 | RACC-Q99 | Capacity-Constrained Frontier Hardness | Proved by knapsack reduction | M | ZS-CACHE-007 | A polynomial exact optimizer for all knapsack instances would refute the assumed complexity boundary. |
| D5-Q15 | RACC-Q99 | Optimization/authority separation | Proved consequence | M | ZS-CACHE-007 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q16 | RACC-Q99 | Weighted Causal Q99 | Proved identity | M | ZS-CACHE-003 | Reported hit denominator or invalid mass differs from sealed weighted demand. |
| D5-Q17 | RACC-Q99 | Adversarial Q99 Impossibility | Proved lower bound | M | ZS-CACHE-004 | System reports exact Q99 while unresolved invalid mass exceeds one percent. |
| D5-Q18 | RACC-Q99 | Q99 Restoration Threshold | Proved | M | ZS-CACHE-005 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q19 | RACC-Q99 | Causal Hazard Bound | Proved union bound | M | ZS-CACHE-008 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q20 | RACC-Q99 | Branch Convergence Reuse | Conditional theorem | M+S | ZS-GRAPH-009 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q21 | RACC-Q99 | Zero-Failure Q99 Sample Bound | Proved under independent Bernoulli model | M | ZS-CACHE-009 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q22 | RACC-Q99 | Expected Prefix-Survival Ordering | Proved under independent volatility and commutative blocks | M | ZS-VIEW-007 | A commutative-block permutation has greater expected surviving prefix than the score order. |
| D5-Q23 | RACC-Q99 | Rendering Fragmentation Penalty | Proved | M | ZS-VIEW-004 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q24 | RACC-Q99 | Uniform fragmentation | Proved | M | ZS-VIEW-004 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-Q25 | RACC-Q99 | Finite Dependency-Refinement Convergence | Conditional theorem | M+S | ZS-GRAPH-008 | A fair refinement adds no new true edge or fails to terminate in finite domain. |
| D5-Q26 | RACC-Q99 | Lease-Revocation Safety | Conditional theorem | M+S | ZS-CAP-005 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-P01 | Pareto Closure | Same-Harness Capability Superset | Conditional theorem | M+S | ZS-BASE-001,ZS-BASE-002 | Treatment removes baseline strategy or publishes an unverified worse result. |
| D5-P02 | Pareto Closure | Campaign Break-Even | Proved | M | ZS-METRIC-005 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-P03 | Pareto Closure | Orthogonal Compression Composition | Proved under nonoverlapping-factor premise | M | ZS-METRIC-003 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-P04 | Pareto Closure | Multi-Resource Pareto Feasibility | Proved | M | ZS-METRIC-004 | Claimed hit rate violates at least one resource coordinate. |
| D5-P05 | Pareto Closure | Robust Pareto Certificate | Conditional theorem | M+S | ZS-METRIC-004 | A parameter corner inside certified intervals violates the target. |
| D5-P06 | Pareto Closure | Frontier Closure | Proved | M | ZS-METRIC-006 | Savings approach one while a nonnegative normalized burden remains bounded away from zero. |
| D5-P07 | Pareto Closure | Reasoning and Effect Floor | Proved | M | ZS-METRIC-007 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-P08 | Pareto Closure | Disjoint Lower-Bound Composition | Proved under disjoint charging | M | ZS-METRIC-007 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-P09 | Pareto Closure | Certified Redundant-Gap Closure | Proved | M | ZS-METRIC-007 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-P10 | Pareto Closure | Baseline-Reserve Feasibility | Conditional theorem | M+S | ZS-BASE-003 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-P11 | Pareto Closure | Compression-Dividend Reinvestment | Conditional theorem | M+S | ZS-METRIC-008 | A finite counterexample violates the stated implication or a required receipt/system premise is absent. |
| D5-P12 | Pareto Closure | Capability-Asset Lifetime Value | Proved in geometric-hazard model | M | ZS-METRIC-009 | Direct geometric expected value disagrees with formula or omitted maintenance/invalidation changes the scope. |
| D5-R01 | Research Program | One-/two-call coverage on common coding tasks | Empirical | S+E | ZS-BENCH-002 | Observed irreducible decision count or expansion rate requires more calls. |
| D5-R02 | Research Program | Windowed indexed Q99 on mature repositories | Empirical | S+E | ZS-BENCH-007 | Any declared window falls below 99% valid weighted causal reuse outside an acknowledged impossibility event. |
| D5-R03 | Research Program | Provider-miss insulation | Empirical | S+E | ZS-BENCH-005 | Forced L1 miss repeats primitive project rediscovery despite valid L2. |
| D5-R04 | Research Program | Dependency completeness in registered domains | Conditional systems premise | S | ZS-GRAPH-003,ZS-GRAPH-010 | Undeclared influence changes a protected object after completeness was certified. |
| D5-R05 | Research Program | Zero protected regression in evaluated scope | Empirical/statistical | S+E | ZS-BENCH-008 | Any soundly adjudicated protected-worse treatment result. |
| D5-R06 | Research Program | Positive strict-rescue rate | Empirical/statistical | S+E | ZS-BENCH-008 | No positive-mass class shows certified better outcome. |
| D5-R07 | Research Program | Boundary-Q99 model-visible interface reduction | Empirical | S+E | ZS-METRIC-003 | Treatment tool-interface tokens exceed one percent of paired baseline. |
| D5-R08 | Research Program | Complete-work Q99 | Empirical | S+E | ZS-METRIC-004 | Any declared complete-work coordinate or aggregate exceeds one percent target. |
| D5-R09 | Research Program | Positive capability-asset lifetime value | Empirical | S+E | ZS-METRIC-009 | Capture/maintenance/invalidation cost exceeds verified reuse benefit. |
| D5-R10 | Research Program | Historical novelty of complete composition | Unresolved | E | Independent literature review | A prior source states the same scoped composition and theorem family. |


---

## SOURCE: `archive/extracted/Draft5/implementation/THEOREM_TO_PROGRAM_MAP_DRAFT5.md`

# Draft 5 Theorem-to-Program Map

**Author:** Aditya G  
**Date:** 13 August 2026

This file is the traceability bridge between the papers and the implementation backlog. A theorem entry is runtime-backed only after every listed requirement is mapped to concrete code, its acceptance test passes, and the relevant systems premises have receipts.

| Claim ID | Claim | Status | Required program obligations |
|---|---|---|---|
| D5-C01 | Protected Decision-View Minimality | Proved in finite stated model | ZS-VIEW-009 |
| D5-C02 | Harness Decision-Boundary Compression | Conditional theorem | ZS-EXEC-001,ZS-EXEC-002,ZS-EXEC-003 |
| D5-C03 | Mechanical Critical-Path Compression | Conditional theorem | ZS-EXEC-005 |
| D5-C04 | Adaptive Decision-Round Lower Bound | Proved in finite communication model | ZS-EXEC-003 |
| D5-C05 | One-/Two-Call Normal Form | Proved in stated model | ZS-BENCH-002 |
| D5-C06 | Boundary-Q99 Threshold | Proved arithmetic | ZS-METRIC-003 |
| D5-C07 | Stable-Reference Recovery | Conditional theorem | ZS-VIEW-003,ZS-SESSION-001 |
| D5-C08 | Harness Capability Superset | Conditional theorem | ZS-BASE-001,ZS-BASE-002 |
| D5-C09 | Finite Signature-Quotient Synthesis | Proved in finite stated model | ZS-VIEW-001 |
| D5-C10 | Decision-View Certificate Soundness | Conditional theorem | ZS-VIEW-009 |
| D5-C11 | Stable-View Sufficiency | Proved in stated model | ZS-VIEW-009 |
| D5-C12 | Monotonicity under exact refinement | Proved | See implementation contract |
| D5-H01 | Harness-Transport Factorization | Conditional theorem | ZS-ADAPTER-009 |
| D5-H02 | Cross-Harness Semantic Cache Reuse | Conditional theorem | ZS-ADAPTER-008,ZS-ADAPTER-009 |
| D5-H03 | Decision-Preserving Private Composition | Conditional theorem | ZS-EXEC-002 |
| D5-H04 | Atomic No-Partial-Authority | Conditional theorem | ZS-KERNEL-008,ZS-STORE-006 |
| D5-H05 | Contingent-Policy Call Compression | Conditional theorem | ZS-EXEC-004 |
| D5-Q01 | Retrospective Rewrite Cache-Break Characterization | Proved | ZS-BENCH-003 |
| D5-Q02 | Cache-Compaction Crossover | Proved | ZS-CACHE-006 |
| D5-Q03 | Horizon-Aware Rewrite Break-Even | Proved | ZS-CACHE-006 |
| D5-Q04 | First-Emission Prefix Immunity | Proved | ZS-VIEW-005 |
| D5-Q05 | Zero Self-Induced Historical Rewrite Cost | Proved under append-only premise | ZS-VIEW-005 |
| D5-Q06 | Stable-Capsule Admission | Proved in cost model | ZS-VIEW-006 |
| D5-Q07 | Two-Plane Cache Factorization | Proved in stated cost model | ZS-CACHE-001 |
| D5-Q08 | Provider-Miss Insulation | Proved in stated cost model | ZS-BENCH-005 |
| D5-Q09 | Provider-Controlled Residency Transposition | Conditional theorem | ZS-CACHE-001,ZS-STORE-007 |
| D5-Q10 | Causal Cache Soundness | Conditional theorem | ZS-KERNEL-003 |
| D5-Q11 | Dependency-Complete Invalidation | Conditional theorem | ZS-GRAPH-006 |
| D5-Q12 | Equality-Boundary Early Cutoff | Conditional theorem | ZS-GRAPH-007 |
| D5-Q13 | Tree-Optimal Causal Materialization | Proved for finite trees | ZS-CACHE-007 |
| D5-Q14 | Capacity-Constrained Frontier Hardness | Proved by knapsack reduction | ZS-CACHE-007 |
| D5-Q15 | Optimization/authority separation | Proved consequence | ZS-CACHE-007 |
| D5-Q16 | Weighted Causal Q99 | Proved identity | ZS-CACHE-003 |
| D5-Q17 | Adversarial Q99 Impossibility | Proved lower bound | ZS-CACHE-004 |
| D5-Q18 | Q99 Restoration Threshold | Proved | ZS-CACHE-005 |
| D5-Q19 | Causal Hazard Bound | Proved union bound | ZS-CACHE-008 |
| D5-Q20 | Branch Convergence Reuse | Conditional theorem | ZS-GRAPH-009 |
| D5-Q21 | Zero-Failure Q99 Sample Bound | Proved under independent Bernoulli model | ZS-CACHE-009 |
| D5-Q22 | Expected Prefix-Survival Ordering | Proved under independent volatility and commutative blocks | ZS-VIEW-007 |
| D5-Q23 | Rendering Fragmentation Penalty | Proved | ZS-VIEW-004 |
| D5-Q24 | Uniform fragmentation | Proved | ZS-VIEW-004 |
| D5-Q25 | Finite Dependency-Refinement Convergence | Conditional theorem | ZS-GRAPH-008 |
| D5-Q26 | Lease-Revocation Safety | Conditional theorem | ZS-CAP-005 |
| D5-P01 | Same-Harness Capability Superset | Conditional theorem | ZS-BASE-001,ZS-BASE-002 |
| D5-P02 | Campaign Break-Even | Proved | ZS-METRIC-005 |
| D5-P03 | Orthogonal Compression Composition | Proved under nonoverlapping-factor premise | ZS-METRIC-003 |
| D5-P04 | Multi-Resource Pareto Feasibility | Proved | ZS-METRIC-004 |
| D5-P05 | Robust Pareto Certificate | Conditional theorem | ZS-METRIC-004 |
| D5-P06 | Frontier Closure | Proved | ZS-METRIC-006 |
| D5-P07 | Reasoning and Effect Floor | Proved | ZS-METRIC-007 |
| D5-P08 | Disjoint Lower-Bound Composition | Proved under disjoint charging | ZS-METRIC-007 |
| D5-P09 | Certified Redundant-Gap Closure | Proved | ZS-METRIC-007 |
| D5-P10 | Baseline-Reserve Feasibility | Conditional theorem | ZS-BASE-003 |
| D5-P11 | Compression-Dividend Reinvestment | Conditional theorem | ZS-METRIC-008 |
| D5-P12 | Capability-Asset Lifetime Value | Proved in geometric-hazard model | ZS-METRIC-009 |
| D5-R01 | One-/two-call coverage on common coding tasks | Empirical | ZS-BENCH-002 |
| D5-R02 | Windowed indexed Q99 on mature repositories | Empirical | ZS-BENCH-007 |
| D5-R03 | Provider-miss insulation | Empirical | ZS-BENCH-005 |
| D5-R04 | Dependency completeness in registered domains | Conditional systems premise | ZS-GRAPH-003,ZS-GRAPH-010 |
| D5-R05 | Zero protected regression in evaluated scope | Empirical/statistical | ZS-BENCH-008 |
| D5-R06 | Positive strict-rescue rate | Empirical/statistical | ZS-BENCH-008 |
| D5-R07 | Boundary-Q99 model-visible interface reduction | Empirical | ZS-METRIC-003 |
| D5-R08 | Complete-work Q99 | Empirical | ZS-METRIC-004 |
| D5-R09 | Positive capability-asset lifetime value | Empirical | ZS-METRIC-009 |
| D5-R10 | Historical novelty of complete composition | Unresolved | Independent literature review |

---

## SOURCE: `archive/extracted/Draft5/research/RESEARCH_AGENDA_RESOLUTION_MATRIX_DRAFT5.md`

# Draft 5 Research Agenda Resolution Matrix

**Author:** Aditya G  
**Date:** 13 August 2026  
**Purpose:** Map every Draft 4 research-agenda item to a Draft 5 theorem, method, artifact, implementation requirement, and remaining evidence status.

## Status vocabulary

- **Mathematically resolved** - a proof is supplied in the stated finite/analytic model.
- **Constructively specified** - the objects, checker, certificate, and falsifier are defined, but the production program has not yet executed them.
- **Finite validated** - an executable validator attacks the algebra or finite model.
- **Evaluation protocol complete** - the paired experiment and release criterion are specified.
- **Empirical result pending** - requires the developing ZeroStack backend, harness adapters, provider calls, or real repositories.
- **Independent review pending** - requires external mathematical, systems, or novelty review.

No empirical item is marked complete merely because a theorem predicts its outcome.

---

## A. Public repository and claim discipline

| Draft 4 agenda item | Draft 5 resolution | Evidence/artifact | Remaining condition |
|---|---|---|---|
| Separate papers from evolving evidence | Completed package layout with `papers/`, `sources/`, `claims/`, `validators/`, `benchmarks/`, `formal/`, `adversarial/`, `implementation/`, and `research/` | Release ZIP and README | Preserve immutable version tags in the public repository |
| Machine-readable claim ledger | Constructively specified and included in CSV/JSON/Markdown | `claims/CLAIM_LEDGER_DRAFT5.*` | Independent proof/status audit |
| Three-axis status labels | Adopted: mathematical, systems, novelty | Claim ledger and paper research-status blurbs | External reviewers update labels when evidence changes |
| Stable papers without “next steps” conclusions | Applied to Draft 5 papers | Five research PDFs | Editorial review before public release |
| Signed/checksummed release manifests | Included | `SHA256SUMS.txt`, ZIP checksum | Optional cryptographic signing by project owner |

---

## B. RACC Core formalization program

### B1. Protected decision-view minimality

**Draft 5 result:** Protected Decision-View Minimality and Finite Signature-Quotient Synthesis.

**Resolution:** Mathematically resolved for finite rooted task families. A decision view is exact only when every hidden state compatible with that view belongs to one protected strategy-signature class. The quotient is minimal among exact finite views.

**Executable obligation:**

- enumerate or represent the finite/domain-specific hidden-state set;
- compute protected strategy signatures;
- emit a class table and decoder;
- verify each proposed merge;
- return `Unknown` outside the certified finite scope.

**Status:** Mathematical proof supplied; finite validation included; general open-world synthesis remains intentionally incomplete.

### B2. Harness Decision-Boundary Compression

**Draft 5 result:** Harness Decision-Boundary Compression and Mechanical Critical-Path Compression.

**Resolution:** Mathematically resolved under trace-projection premises. Mechanical segments may be privately composed when the backend starts from the same rooted state, preserves exact decision-sufficient observations, executes no unauthorized effect, and exposes expansion/fallback.

**Executable obligation:** trace segmentation, private-composability checker, effect trace, decision-view certificate, baseline escape.

**Status:** Theorem supplied; evaluation protocol and falsifiers complete; real one-/two-call coverage pending.

### B3. Adaptive decision-round lower bound

**Draft 5 result:** Adaptive Decision-Round Lower Bound and Contingent-Policy Call Compression.

**Resolution:** Mathematically resolved. A backend cannot safely collapse an observation-dependent semantic choice unless the user/model supplied a contingent policy or a sound verifier uniquely resolves it.

**Status:** Proof and benchmark annotation rubric supplied; empirical annotation study pending.

---

## C. RACC-Harness systems program

### C1. Transport equivalence

**Draft 5 result:** Harness-Transport Factorization and Cross-Harness Semantic Cache Reuse.

**Resolution:** Constructively specified. Harness-independent semantic objects are separated from adapter-specific rendering. A faithful adapter must preserve request, state, action, result, and authority semantics.

**Artifacts:** implementation requirements, adapter conformance schema, paired evaluation protocol.

**Pending:** Implement at least three adapters and compare protected roots/receipts.

### C2. Task-contract coverage

**Draft 5 result:** complete process models for program explanation, repository refactor, Python-to-C++ port, and greenfield game/application construction.

**Resolution:** Constructively specified with task-specific protected contracts and Unknown dimensions.

**Pending:** Real task corpus and paired execution.

### C3. One-/two-call study

**Draft 5 result:** One-/Two-Call Normal Form and decision-boundary annotation protocol.

**Resolution:** Exact eligibility condition supplied; evaluation rubric specifies blinded annotation and failure when a semantic decision is hidden.

**Pending:** Same-model/same-harness measurements on real tasks.

### C4. Transactional safety

**Draft 5 result:** Atomic No-Partial-Authority and Lease-Revocation Safety.

**Resolution:** The state-machine invariant is mathematically explicit and implementation requirements define parent-root/epoch binding, exact delta, verifier receipt, short-lived authority, compare-and-swap, and no-mutation failures.

**Pending:** Fault-injection execution against the actual runtime.

---

## D. RACC-Q99 cache program

### D1. Prefix-stability experiment

**Draft 5 result:** Retrospective Rewrite Cache-Break Characterization, First-Emission Prefix Immunity, Rendering Fragmentation Penalty.

**Resolution:** The exact sequence-level theorems are proved. The experiment now compares raw retention, retrospective replacement, and reference-first append-only history while freezing tool/schema/render variables.

**Finite validation:** prefix and canonical-render identities tested.

**Pending:** provider measurements across models/routes and harness adapters.

### D2. Cache-compaction crossover

**Draft 5 result:** Cache-Compaction Crossover with horizon, read/write prices, rewritten suffix, and compaction cost.

**Resolution:** Exact inequality and break-even threshold supplied; validator tests both sides of the boundary.

**Pending:** populate with current provider price/latency measurements.

### D3. Causal dependency completeness

**Draft 5 result:** Causal Cache Soundness, Dependency-Complete Invalidation, finite counterexample-guided refinement.

**Resolution:** Constructive certificate and failure semantics specified. Graph completeness is graded `Proved`, `BoundedComplete`, `Observed`, or `Unknown`; runtime-discovered undeclared influences revoke certificates.

**Pending:** implement static/dynamic adapters and measure omission rate.

### D4. Causal invalidation benchmark

**Draft 5 result:** exact descendant invalidation, Equality-Boundary Early Cutoff, Weighted Causal Q99, Causal Hazard Bound.

**Resolution:** Mathematical identities and benchmark mutation classes supplied.

**Pending:** run on real repositories and build systems.

### D5. Provider-miss insulation

**Draft 5 result:** Two-Plane Cache Factorization and Provider-Controlled Residency Transposition.

**Resolution:** The theorem proves that provider loss changes the cost of reprocessing a compact view but need not invalidate retained causal project objects. Forced-miss protocol is complete.

**Pending:** provider/harness experiments.

### D6. Windowed Q99

**Draft 5 result:** Weighted Q99, Adversarial Q99 Impossibility, Q99 Restoration Threshold, sliding-window service semantics.

**Resolution:** Exact formulas and honest impossibility state supplied. Campaign averages cannot hide post-change collapse.

**Pending:** select service windows and demand weights from production traces.

---

## E. Pareto Closure program

### E1. Same-model/same-harness pairing

**Draft 5 result:** Same-Harness Capability Superset and complete paired-trial protocol.

**Resolution:** The baseline/treatment object is frozen: same model, harness, reasoning allowance, native tools, initial root, and acceptance criteria. ZeroStack remains optional.

**Pending:** execute paired trials and publish sealed ledgers.

### E2. Multi-resource feasibility

**Draft 5 result:** Multi-Resource Pareto Feasibility and Robust Pareto Certificate.

**Resolution:** Mathematically resolved with exact feasible intervals and uncertainty-aware sufficient condition. Validators compare analytic intervals with direct checks.

**Pending:** real coordinate estimates.

### E3. Frontier Closure decomposition

**Draft 5 result:** Frontier Closure plus Campaign Break-Even.

**Resolution:** Mathematically resolved. Preparation, prepared-path work, and novelty/fallback work are independently normalized; the largest term identifies the limiting avenue.

**Pending:** campaign traces.

### E4. Certified lower bounds

**Draft 5 result:** Disjoint Lower-Bound Composition and Certified Redundant-Gap Closure.

**Resolution:** The paper defines request, decision, reasoning, verification, output, and external-effect lower-bound components and the honest 100-percent endpoint `Gamma = 1`.

**Pending:** stronger task-specific lower-bound certificates and independent proof review.

---

## F. Additional theorem and systems avenues

### F1. Causal early cutoff

**Resolved:** Equality-Boundary Early Cutoff theorem; implementation checker and benchmark specified.

### F2. Minimum-cost causal frontier

**Resolved for trees:** Tree-Optimal Causal Materialization dynamic program, validated against brute force.  
**General DAG status:** optimization remains combinatorial; any proposed frontier is independently verified by executable closure.

### F3. Adversarial Q99 impossibility

**Resolved:** exact lower bound prevents valid reuse claims over unresolved changed mass above the threshold.

### F4. Stable-view semantic margins

**Resolved in finite/domain model:** Stable-View Sufficiency and monotonic refinement; counterexamples force expansion or view refinement.

### F5. Cross-harness rendering factorization

**Resolved as architecture/theorem:** semantic objects persist while rendering changes.  
**Pending:** concrete adapter measurements and canonical byte conformance.

### F6. Capability lifetime and invalidation

**Constructively specified:** capability registry, freshness/invalid/unknown states, drift observation, revalidation, retirement, and lifetime value are in the implementation requirements.  
**Pending:** empirical capture, invalidation, and strict-rescue rates.

### F7. Avenues added in Draft 5

Draft 5 also resolves or specifies several smaller avenues that were implicit but not fully developed in Draft 4:

- canonical-render fragmentation and deterministic rendering;
- validity leases and revocation;
- finite dependency-refinement convergence;
- orthogonal compression composition;
- index campaign break-even;
- robust Pareto feasibility under confidence intervals;
- disjoint lower-bound composition;
- exact zero-failure statistical bounds;
- task-weighted causal hazard;
- branch convergence reuse;
- provider-prefix versus causal-cache factorization.

---

## G. Formal proof priorities

| Priority | Draft 5 status | Formal artifact required |
|---|---|---|
| Exact-prefix rewrite/append-only preservation | Proved and finite validated | Lean finite-list theorem |
| Causal DAG invalidation and early cutoff | Proved under deterministic DAG premise | Lean explicit DAG/reachability model |
| Decision quotient/minimality | Proved for finite family | Lean finite lists/classes and decoder |
| Harness trace projection/no mutation | Proved conditionally | Abstract transition-system refinement proof |
| Multi-resource interval algebra | Proved and validated | Rational arithmetic theorem |
| Frontier Closure/gap closure | Proved and validated | Sequence/nonnegative-sum theorem |
| Successor simulation | Conditional systems theorem | Coinductive or finite-trace relation |
| Capability lifecycle | Conditional/empirical | Finite Markov/lifetime model first |

The release includes a detailed formalization specification but does not claim proof-assistant completion.

---

## H. Independent review and novelty

**Completed internally:** primary-source literature map, theorem/prior-art separation, explicit epistemic labels.  
**Pending:** backward/forward citation review, adjacent-field experts, independent mathematical audit, systems security review, public priority assessment.

The intended novelty claim remains narrow: not that content addressing, incremental builds, append-only logs, prompt caching, Code Mode, runtime assurance, or fallback are individually new, but that their recovery-aware harness composition produces a precise theorem, authority, Q99, and Pareto-closure framework.

---

## I. Publication release gates

Draft 5 package status:

- Paper proofs in stated models: **included; independent audit pending**.
- Current provider facts: **refreshed and dated from official documentation**.
- Systems claims and falsifiers: **included**.
- Source, PDFs, build logs, checksums: **included**.
- Claim ledger: **included**.
- Paired benchmark protocol and schemas: **included**.
- Raw production ledgers: **pending implementation**.
- Formal proof artifacts: **specification included; proofs pending**.
- Negative empirical results: **none claimed because production experiments have not yet run**.

Therefore Draft 5 is a complete research and implementation package, not a completed empirical validation of the production moat.

---

## J. Kill criteria and stop rules

The following criteria remain active and are encoded in the evaluation/fault-injection program:

- hidden semantic decisions in a claimed one-call path;
- recurring causal dependency omissions;
- compact views that change protected decisions after expansion;
- weak fallback reserve;
- verification/index/repair cost erasing savings;
- central changes preventing the declared Q99 window;
- provider-prefix instability caused by unavoidable adapter rendering;
- model-visible gains that fail complete-work accounting;
- capability assets with harmful transfer or negative lifetime value.

A new top-level theorem enters the series only when it changes runtime authority, tightens a lower bound, decomposes a measured bottleneck, explains a reproducible failure, or replaces a heuristic premise with a constructive checker and converse.

---

## Bottom line

The Draft 4 agenda is not “finished” in the sense of having production measurements before the program exists. Draft 5 completes the work that can be responsibly completed at this stage:

1. the theorem family is sharpened;
2. the systems premises are converted to explicit certificates and failure semantics;
3. the paired evaluation and fault-injection program is complete;
4. finite algebraic/graph models are executable and validated;
5. every remaining empirical claim is bound to a concrete implementation obligation and release gate.

The next evidence must come from the developing runtime, not from adding decorative theorems.


---

## SOURCE: `current/model_ingest/text/pdf_text/archive/extracted/Draft5/papers/02_RACC_Q99_Causal_Caching_Draft5.txt`

                                         RACC-Q99 Draft 5
        Durable Causal Caching, Prefix Immunity, and Exact Reuse Service Guarantees


                                                   Aditya G
                                   ZeroStack / TokenZero Research Program


                                         Draft 5 — 13 August 2026


 Research status and contribution. This paper develops the cache-specific mathematical core of ZeroStack.
 It distinguishes provider prefix caching from durable indexed causal reuse and proves the conditions under which
 the two layers compose rather than undermine one another. The formal results include a retrospective-rewrite
 cache-break theorem, a cache-compaction crossover theorem, a first-emission prefix-immunity theorem, a
 two-plane cache factorization theorem, provider-miss insulation, exact causal invalidation, equality-boundary
 early cutoff, a tree-optimal materialization theorem, weighted and windowed Q99 criteria, a Q99 restoration
 lower bound, a causal hazard bound, and a finite-sample condition for statistically supporting a 99-percent
 hit claim. The paper does not claim that a provider never evicts a prefix or that every repository change
 preserves Q99. Provider policies are dated factual inputs; dependency completeness, durable retention, equality
 checking, and demand weights are systems premises. The candidate contribution is an exact recovery-aware
 composition that prevents self-inflicted prefix invalidation while moving authoritative project memory from
 provider-controlled sequence caches to a content-rooted dependency cache.


                                                    Abstract

         Tool-output compaction can reduce the nominal length of an agent transcript while simultaneously
     destroying the exact prompt prefix on which provider caching depends. ZeroStack avoids this conflict
     by capturing large tool results before repeated model exposure, storing them as immutable content-
     addressed objects, and emitting stable decision capsules with exact expansion handles from the first
     interaction. The provider sees a short append-only conversation; the backend retains durable project
     state in a causally invalidated object graph. We formalize this as a two-plane cache. The provider
     plane accelerates exact prompt prefixes but is subject to routing, retention, eviction, model, tool-
     schema, and prefix-match conditions. The ZeroStack plane treats exact project objects and derived
     artifacts as authoritative while their complete causal keys remain valid. We prove that retrospective
     rewriting invalidates every cached suffix after the first changed token, whereas first-emission capsules
     create no self-induced historical prefix break. Conditional on a valid indexed object, provider misses
     require only reprocessing the compact decision view rather than rediscovering the project. We then
     develop dependency-complete invalidation, early cutoff at equal recomputed roots, exact materialization
     optimization on dependency trees, weighted and sliding-window Q99, recovery time after high-impact
     changes, and statistical certification. The result is a precise moat: provider cache volatility can alter
     the price of rendering a compact view, but it need not erase the project’s reusable knowledge or force
     the model back through a million-token primitive-tool history.



Contents

1 Three cache layers and five different hit rates                                                                   3




                                                        1
ZeroStack RACC Causal Cache Research                                                  Draft 5 — 13 August 2026


2 Why retrospective compaction can break provider caching                                                       3
   2.1   Exact compaction crossover . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .       4
   2.2   Horizon-aware rewrite economics . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .        5

3 Causal Cache Normal Form                                                                                      5
   3.1 When a stable capsule should be admitted . . . . . . . . . . . . . . . . . . . . . . . . . . .           6

4 Two-plane cache composition                                                                                   7
   4.1   Provider-controlled residency transposition . . . . . . . . . . . . . . . . . . . . . . . . . .        7

5 Causal keys and exact object formation                                                                        8

6 Dependency-complete invalidation                                                                              8
   6.1   Causal early cutoff . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .    9

7 Minimum-cost exact materialization on trees                                                                   9

8 Weighted Q99 and its exact boundary                                                                          10
   8.1   Q99 restoration after a change . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 10
   8.2 Windowed Q99 . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 11

9 Causal hazard and sensitivity                                                                                11

10 Branching, convergence, and cross-harness reuse                                                             12

11 Statistical support for Q99 claims                                                                          12

12 Volatility-aware prefix ordering                                                                            12

13 Canonical rendering and exact-prefix fragmentation                                                          13

14 Counterexample-guided dependency refinement                                                                 14

15 Validity leases and revocable authority                                                                     14

16 A precise meaning of “never break the cache”                                                                14

17 Conclusion                                                                                                  15




                                                       2
ZeroStack RACC Causal Cache Research                                              Draft 5 — 13 August 2026


1      Three cache layers and five different hit rates

A ZeroStack deployment should distinguish at least three layers:

1. L1 provider prefix cache: model-provider acceleration for an exact request prefix;
2. L2 ZeroStack causal cache: durable exact project objects and derived artifacts keyed by complete
   causal identity;
3. L3 physical residency: in-memory, local-disk, remote-object, or replicated storage placement for L2
   objects.

These layers answer different questions. Provider documentation currently reflects heterogeneous policies.
OpenAI now distinguishes exact cache breakpoints on GPT-5.6-and-later models, whose documented time
to live is refreshed on reuse and is at least thirty minutes, from earlier-model best-effort prefix caching,
whose in-memory entries generally survive five to ten minutes of inactivity, at most one hour, with
optional extended retention up to twenty-four hours on supported models; exact matching, availability,
and routing still determine hits [10, 7]. Anthropic exposes five-minute and one-hour cache durations and
prices writes separately from reads [4, 3]. xAI states that entries may be evicted under load or restarts
and recommends conversation routing to improve retention [12, 13]. Google offers implicit prefix caching
and explicit cache objects with a configurable time to live [6].
A public Q99 claim must say which quantity it measures:

• provider-reported cached input tokens;
• valid L2 object reuse;
• hot L3 residency;
• model-visible project-context elimination;
• complete-work reduction.

No one percentage may substitute for another.


2      Why retrospective compaction can break provider caching

Let a cached token sequence be
                                               H = A∥X∥B,
where X is an old tool result and B is every later message already appended after it. A retrospective
router replaces X by a smaller summary S:

                                               H ′ = A∥S∥B.

Theorem 2.1 (Retrospective Rewrite Cache-Break Characterization). Let

                      r = lcp(X∥B, S∥B),        n = |X| ,      s = |S| ,    b = |B| .

Then
                             lcp(H, H ′ ) = |A| + r ≤ |A| + min(n + b, s + b).



                                                     3
ZeroStack RACC Causal Cache Research                                               Draft 5 — 13 August 2026


For the rewritten request H ′ , the exact residual beyond the reusable prefix is
                                            mbreak = s + b − r.
Thus a self-induced exact-prefix miss occurs after the common prefix exactly when
                                                 r < s + b,

equivalently when S∥B is not a prefix of X∥B. If X and S first differ before either block ends, then
r = lcp(X, S) and the break occurs inside the replacement block. If S∥B is a prefix of X∥B, the shorter
rewritten request may be fully prefix-reused despite the historical rewrite.

Proof. The common prefix A cancels, giving lcp(A∥Y, A∥Y ′ ) = |A| + lcp(Y, Y ′ ) for Y = X∥B and
Y ′ = S∥B. The common-prefix length cannot exceed the length of the shorter residual. The rewritten
request has length s + b after A, so precisely s + b − r of those tokens lie beyond the reusable prefix. This
quantity is positive exactly when Y ′ is not a prefix of Y . The interior-mismatch case follows because the
first disagreement occurs before either replacement block ends.

Thus a small rewrite near the beginning can invalidate a very large cached suffix. OpenAI’s prompt-caching
guidance explicitly recommends preserving exact prefixes, appending messages rather than modifying
earlier ones, and keeping tool schemas and ordering stable [10]. xAI similarly documents misses when
earlier messages are modified or reordered [12].


2.1    Exact compaction crossover

Let:

• n = |X| be the old result size;
• s = |S| the replacement size;
• b = |B| the later suffix size;
• r = lcp(X∥B, S∥B) the exact reusable prefix after A;
• ρ the provider price multiplier for cached reads;
• u the multiplier for uncached processing or new cache writes.

After removing the common cost of A, keeping the exact cached segment costs
                                             Ckeep = ρ(n + b).
The rewritten request reuses r tokens and processes the remaining s + b − r tokens at the uncached/write
price:
                                      Crewrite = ρr + u(s + b − r).
Theorem 2.2 (Cache-Compaction Crossover). Retrospective compaction is cheaper in the modeled
coordinate exactly when
                              ρr + u(s + b − r) < ρ(n + b).
Equivalently, when u > 0,
                                          ρ      ρ
                                                          
                                        s< n− 1−   (b − r).
                                          u      u
The earlier zero-alignment formula u(s + b) < ρ(n + b) is the special case r = 0.

                                                       4
ZeroStack RACC Causal Cache Research                                                  Draft 5 — 13 August 2026


Proof. The first statement directly compares the two exact costs. Expanding the left-hand side and
isolating us gives us < ρn − (u − ρ)(b − r), which yields the second expression after division by u > 0.

The theorem does not say retrospective compaction is always bad. It says that prompt length alone is
not the objective: the position of the rewrite, suffix length, cache-read price, write price, and future reuse
all matter.


2.2   Horizon-aware rewrite economics

The one-request crossover can reverse when a rewritten history will be reused many times. Let T ≥ 1 be
the number of future requests sharing the history, cr the unit cost of an exact cached read, cw the unit
cost of processing or writing the rewritten prefix on its first use, and cc the one-time compaction cost.
Assume the original segment is already cache-eligible.

Theorem 2.3 (Horizon-Aware Rewrite Break-Even). Keeping the original history for T requests costs

                                           Ckeep (T ) = T cr (n + b).

Rewriting once, reusing the first r tokens on that request, and then reusing the rewritten prefix costs

                         Crewrite (T ) = cc + cr r + cw (s + b − r) + (T − 1)cr (s + b)
                                     = cc + (cw − cr )(s + b − r) + T cr (s + b).

Therefore rewriting is cheaper exactly when

                                  cc + (cw − cr )(s + b − r) < T cr (n − s).

When n > s, cr > 0, and cw ≥ cr , the smallest admissible integer horizon is

                                                 cc + (cw − cr )(s + b − r)
                                                                             
                             Tmin = max 1,                                  +1 .
                                                         cr (n − s)

Under cc ≥ 0 and cw ≥ cr , if n ≤ s rewriting cannot obtain a positive long-run advantage in this
coordinate.

Proof. The second cost identity follows by collecting the first-request cached and uncached portions with
the T − 1 later cached reads. Subtracting T cr (s + b) from T cr (n + b) gives T cr (n − s). The strict integer
threshold follows by division by the positive denominator cr (n − s).

This result separates two questions often conflated in cache discussions: whether rewriting wins on the
next request, and whether it wins over the remaining reuse horizon. A cache-aware router must estimate
both the suffix destroyed now and the number of future exact-prefix reads that the replacement is likely
to receive.


3     Causal Cache Normal Form

ZeroStack replaces retrospective rewriting with first-emission indirection.




                                                       5
ZeroStack RACC Causal Cache Research                                              Draft 5 — 13 August 2026


Definition 3.1 (Causal Cache Normal Form). A large exact artifact X is represented as

                                  CCNF(X) = (KX , FX , PX , CX , EX ),

where:

• KX is the complete causal key;
• FX is a formation receipt binding the key to the payload;
• PX is the immutable exact payload or content root;
• CX is the stable model-visible decision capsule;
• EX is an exact expansion authority.

The capsule is emitted when the artifact first enters model-visible history. The exact payload is stored
once in L2. Later expansion appends new evidence or a new rooted capsule; it does not rewrite the
original history.

Theorem 3.2 (First-Emission Prefix Immunity). Let model-visible histories satisfy

                                              Ht+1 = Ht ∥∆t .

Then Ht is a prefix of every later Ht+k . Evidence expansion under CCNF creates no self-induced
invalidation of the historical prefix Ht .

Proof. Immediate by induction on append-only concatenation.

Corollary 3.3 (Zero Self-Induced Historical Rewrite Cost). Decompose prefix misses into self-rewrite,
provider residency/routing, contract change, and genuinely new suffix components. Under CCNF and
stable rendering, the self-rewrite component is zero.

This is the rigorous form of “never break the cache”: ZeroStack cannot prevent provider eviction, but it
can avoid invalidating its own old prefix by rewriting tool history.


3.1      When a stable capsule should be admitted

Indirection is not free. A capsule consumes tokens on every request that includes it, and later evidence
expansion can add new model-visible material. Let n be the raw artifact size, c < n the stable capsule size,
T the number of future requests whose prefix contains the representation, and let et xt be the expected
additional model-visible expansion burden at request t. Let hr and hc be any additional per-request
handling costs for the raw and capsule representations in the same declared coordinate.

Theorem 3.4 (Stable-Capsule Admission). The capsule has lower expected horizon cost than first-emission
raw exposure exactly when
                                                               T
                                                               X
                                      T (n + hr − c − hc ) >         e t xt .
                                                               t=1

If the left-hand side is nonpositive, the capsule cannot dominate in that coordinate. If expansions are
uniformly bounded by et xt ≤ η, a sufficient condition is

                                           n + hr − c − hc > η.

                                                     6
ZeroStack RACC Causal Cache Research                                            Draft 5 — 13 August 2026


                                                                                  P
Proof. The raw representation costs T (n + hr ). The capsule costs T (c + hc ) + t et xt . Compare and
rearrange. The bounded sufficient condition follows by replacing the sum with T η.

The admission decision must use a complete resource vector. A capsule may win in provider-input tokens
while losing in latency because expansions are frequent, or win in model-visible tokens while losing in
backend storage. CCNF supplies the safe representation; Theorem 3.4 supplies a measurable policy
boundary.


4      Two-plane cache composition

Let D be the model-visible project transcript required by ordinary rediscovery, and C < D the compact
decision view generated from valid L2 state. Let p be the L1 provider-hit probability, ρ the cached-read
multiplier, and u the uncached/write multiplier.
Conditional on valid L2 state, native project retransmission has expected provider-processing cost

                                       E[JB ] = pρD + (1 − p)uD,

whereas ZeroStack has
                                       E[JZ ] = pρC + (1 − p)uC.

Theorem 4.1 (Two-Plane Cache Factorization). Under the same provider hit process and price multipliers,

                                                E[JZ ]  C
                                                       = .
                                                E[JB ]  D

Provider volatility scales both renderings but does not erase the L2 compression ratio.

Proof. Factor the common multiplier pρ + (1 − p)u from numerator and denominator.

Corollary 4.2 (Provider-Miss Insulation). On an L1 miss, the model-visible savings conditional on a
valid L2 object are
                                                C
                                            1− .
                                                D
The backend re-renders the compact view rather than repeating primitive project rediscovery.

This is the central moat. L1 caching lowers the price of a stable compact interaction; L2 caching
determines whether the project itself remains known.


4.1    Provider-controlled residency transposition

Let:

• I be causal validity of the project object;
• M be model, prefix, tool, and routing compatibility;
• RP be provider residency;
• RZ be ZeroStack durable retention.


                                                    7
ZeroStack RACC Causal Cache Research                                              Draft 5 — 13 August 2026


The provider hit event is
                                            HP = I ∩ M ∩ RP ,
and the indexed logical hit is
                                              HZ = I ∩ RZ .
Theorem 4.3 (Provider-Controlled Residency Transposition). If RZ is guaranteed within the declared
storage contract, then
                   P(HZ ) − P(HP ) = P I ∩ RZ ∩ (M ∩ RP )c − P I ∩ M ∩ RP ∩ RZ
                                                                             c
                                                                                      
                                                                               .
When RZ holds for every valid retained object, the second term is zero and the indexed logical hit rate is
no lower than the provider hit rate.

The theorem is about logical project availability, not hot-memory latency. An L2 object may reside on
disk or remote storage and still be a logical hit.


5    Causal keys and exact object formation

Let a derived object v be produced by deterministic constructor gv from dependency values xp1 , . . . , xpk
under contract γv . Define
                                 Kv = H(gv , γv , H(xp1 ), . . . , H(xpk )).
A formation receipt binds Kv to the exact payload root H(xv ) and the producer execution record.
Theorem 5.1 (Causal Cache Soundness). Assume collision-free identity within the declared digest premise,
deterministic construction under γv , and a valid formation receipt. Reusing the object associated with Kv
yields the same exact value that recomputation would produce.

Proof. The key fixes the constructor, contract, and exact dependency values. Determinism fixes the
output. The formation receipt proves that the stored payload was produced under that key.

A matching key without a formation receipt is insufficient: the store must prove that the payload was
actually formed from the bound inputs rather than merely labeled with them.


6    Dependency-complete invalidation

Let G = (V, E) be a finite DAG whose edges u → v mean that v’s constructor depends on u. Let ∆ ⊆ V
be nodes whose exact values or bound contracts changed. Define the reflexive descendant closure
                                                Desc∗ (∆).
Theorem 6.1 (Dependency-Complete Invalidation). Under a complete dependency DAG and deterministic
constructors, every node outside Desc∗ (∆) retains the same causal key and exact value. Initial invalidation
need not include any such node.

Proof. Proceed in topological order. A node outside the descendant closure is unchanged and has no
changed ancestor; therefore its constructor, contract, and dependency roots are unchanged, so its key and
value remain unchanged.

The completeness premise is the hard systems obligation. Actual runtime reads, environment values, tool
versions, network responses, generated files, clocks, randomness, and platform state must either appear in
the graph or be prohibited by the scope.

                                                     8
ZeroStack RACC Causal Cache Research                                            Draft 5 — 13 August 2026


6.1   Causal early cutoff

Initial descendant invalidation may still overestimate physical recomputation. Let B be a boundary
separating changed sources from downstream nodes.

Theorem 6.2 (Equality-Boundary Early Cutoff). Suppose every path from ∆ to demanded downstream
outputs intersects B. If recomputation produces the same exact boundary values as before,

                                       H(x′b ) = H(xb )      ∀b ∈ B,

then every downstream deterministic node has the same exact value as before, and change propagation
may stop at B.

Proof. Each downstream node depends only on unchanged nonaffected inputs and boundary values. Since
all such inputs are equal, induction in topological order gives equality of every downstream value.

This principle appears in incremental build systems as early cutoff and in self-adjusting computation as
change propagation with reuse [8, 1, 2]. RACC applies it to model-facing project facts, decision capsules,
verification artifacts, and executable operators.


7     Minimum-cost exact materialization on trees

The cache should retain a computational frontier, not merely recently used text. Consider a dependency
tree oriented from children to a demanded root. For node v, let:

• cv be the cost to retrieve a valid cached value of v;
• rv be local recomputation cost;
• av be the cost to obtain a leaf source value.

Let M (v) be the minimum exact cost to materialize v.

Theorem 7.1 (Tree-Optimal Causal Materialization). For a leaf v,

                                            M (v) = min(cv , av ).

For an internal node with children ch(v),
                                                                         
                                                            X
                                 M (v) = min cv , rv +              M (u) .
                                                           u∈ch(v)


The recurrence yields the globally minimum exact materialization cost for the demanded root.

Proof. Induct on tree height. Any exact strategy for v either retrieves v directly or computes it from
every child. Tree subproblems are disjoint, so the cheapest compute strategy uses the independently
optimal child strategies. Taking the minimum of the two exhaustive cases is optimal.




                                                      9
ZeroStack RACC Causal Cache Research                                               Draft 5 — 13 August 2026


For general DAGs, shared dependencies couple the subproblems; a naive tree recurrence may double-count.
An optimizer may use dynamic programming on restricted graph families, integer programming, or
heuristics. Regardless of optimization method, sufficiency of a proposed frontier remains independently
checkable by executable closure. Build systems already separate action metadata from a content-
addressable store and reuse exact outputs under action keys [9, 8, 2]; ZeroStack extends the object domain
to project-semantic views and harness decisions.
Theorem 7.2 (Capacity-Constrained Frontier Hardness). The general problem of choosing cached objects
under a capacity budget to maximize independent reuse value is NP-hard, even before dependency sharing
or invalidation is introduced.

Proof. Reduce 0–1 knapsack. For each knapsack item i create an independently sufficient cache object
with resident size equal to the item weight and saved work equal to the item value. A cache subset within
capacity with saved work at least V exists exactly when the corresponding knapsack subset exists.

Corollary 7.3 (Optimization/authority separation). A globally optimal causal-cache plan may be com-
putationally difficult to find, while exact capacity, identity, validity, and executable-closure checks for a
proposed finite plan remain polynomial in the explicit graph and object set. Therefore heuristics, solvers,
or models may propose plans without becoming cache-validity authority.


8     Weighted Q99 and its exact boundary

Let D(q) be the task-relevant demanded object set for request q, with nonnegative weight measure w.
Let I(q, ∆) be the subset invalidated after dependency closure and any proven early cutoff.
Define exact weighted reuse
                                                          w(I(q, ∆))
                                       Rw (q, ∆) = 1 −               .
                                                           w(D(q))
Theorem 8.1 (Weighted Causal Q99).

                          Rw (q, ∆) ≥ 0.99    ⇐⇒      w(I(q, ∆)) ≤ 0.01w(D(q)).

Proof. Algebraic rearrangement of the definition.

Weights must be declared. Valid choices include exact bytes, object count, retrieval work, model-visible
tokens, latency, or another resource coordinate. Changing weights changes the claim.
Theorem 8.2 (Adversarial Q99 Impossibility). If more than one percent of demanded weight is genuinely
invalid and no independent equality or sufficiency proof covers that mass, an exact system cannot claim a
valid 99% hit for that task before recomputation or verification resolves enough of the affected mass.

The result prevents stale-cache marketing under central changes.


8.1   Q99 restoration after a change

Let total demanded mass be W , initial invalid mass I0 , and V (t) the invalid mass that has been recomputed
or independently verified by time t. Then
                                                        I0 − V (t)
                                           R(t) = 1 −              .
                                                           W

                                                     10
ZeroStack RACC Causal Cache Research                                            Draft 5 — 13 August 2026


Theorem 8.3 (Q99 Restoration Threshold). Q99 is restored exactly when

                                       V (t) ≥ max(0, I0 − 0.01W ).

Any exact recovery process must resolve at least this much affected mass unless a stronger proof removes
it from I0 .

Proof. Set R(t) ≥ 0.99 and rearrange. Unresolved invalid mass cannot be counted as a valid hit.

This theorem turns post-change coldness into a measurable service objective: time-to-Q99 and work-to-
Q99.


8.2   Windowed Q99

Campaign averages can hide severe failure after edits. For a family of windows W, define

                                          valid reused demand mass in W ′
                             Rmin = inf                                   .
                                     ′
                                     W ∈W     total demand mass in W ′
Windowed Q99 requires Rmin ≥ 0.99. High-impact changes that make the bound impossible are reported
as lower-bound events rather than averaged away.


9     Causal hazard and sensitivity

For node v, define task-relative causal sensitivity

                                                 w(D(q) ∩ Desc∗ (v))
                                      aq (v) =                       .
                                                     w(D(q))

If exactly one changed node is drawn from distribution p(v), expected reuse before early cutoff is
                                                       X
                                        E[Rw ] = 1 −            p(v)aq (v).
                                                        v

Theorem 9.1 (Causal Hazard Bound). For an arbitrary random changed set ∆, let pv = P(v ∈ ∆). Then
                                                        X
                                         E[1 − Rw ] ≤            pv aq (v),
                                                            v

with equality when exactly one node changes and the descendant sets are measured under that single-change
distribution.

Proof. The invalidated set is the union of task-relative descendant cones. Weighted union mass is at most
the sum of cone masses. Taking expectations yields the bound.

The bound identifies high-risk central nodes and suggests where stronger dependency capture, replication,
preverification, or compact equality certificates have the greatest value.




                                                      11
ZeroStack RACC Causal Cache Research                                                Draft 5 — 13 August 2026


10     Branching, convergence, and cross-harness reuse

Content addressing allows project knowledge to survive branch operations.

Proposition 10.1 (Branch Convergence Reuse). If two branches independently produce an object with
the same complete causal key and payload root, the object is identical within the digest premise and may
be shared after convergence. Branch identity alone is not an invalidator.

Similarly, harness-specific renderings should be derived from harness-independent semantic objects. A
switch from one faithful harness adapter to another invalidates the renderer cache, not the project-semantic
object cache.


11     Statistical support for Q99 claims

An observed hit rate is not automatically a reliable service-level probability. Suppose n independent
Bernoulli trials produce zero misses. To reject the null p < q at one-sided significance α, it is sufficient
that
                                                 q n ≤ α.

Proposition 11.1 (Zero-Failure Q99 Sample Bound). With zero observed misses, a one-sided exact test
supports p ≥ q at significance α only when

                                                    ln α
                                                               
                                                 n≥      .
                                                    ln q

For q = 0.99 and α = 0.05, at least 299 consecutive valid hits are required. For q = 0.999, at least 2995
are required.

Proof. Under p = q, the probability of n consecutive hits is q n . Require this tail probability to be at
most α and rearrange.

The proposition is a special zero-failure case of exact binomial confidence analysis [5, 11]. Real evaluations
should account for temporal dependence, project clustering, and sliding-window claims rather than
treating every task as independent.


12     Volatility-aware prefix ordering

Some provider-visible blocks are semantically commutable: for example, independent tool schemas, static
capability descriptions, or canonically sorted reference tables. Their order can change the expected length
of the surviving exact prefix.
Let block i have token length ℓi > 0 and independent probability pi of changing before the next request.
For an ordering π, the expected reusable prefix mass contributed by the ordered blocks is
                                                 m
                                                 X            j
                                                              Y
                                        Φ(π) =         ℓπ j       (1 − pπk ).
                                                 j=1          k=1




                                                        12
ZeroStack RACC Causal Cache Research                                                      Draft 5 — 13 August 2026


Theorem 12.1 (Expected Prefix-Survival Ordering). Among blocks that may be reordered without
changing semantics, Φ is maximized by sorting in nonincreasing order of
                                                               1 − pi
                                                    θ i = ℓi          ,
                                                                 pi

with pi = 0 placed first and pi = 1 placed last.

Proof. Consider adjacent blocks a, b after a prefix that survives with probability Q. Ordering a, b is at
least as good as b, a exactly when
                 Q [(1 − pa )ℓa + (1 − pa )(1 − pb )ℓb ] ≥ Q [(1 − pb )ℓb + (1 − pb )(1 − pa )ℓa ] .
After cancellation this is equivalent to
                                                   1 − pa      1 − pb
                                              ℓa          ≥ ℓb        .
                                                     pa          pb
Repeated adjacent exchanges yield the stated order.

The theorem is not permission to reorder conversation semantics. It applies only inside a declared
commutative region. It gives a precise reason to place long, stable material before short, volatile material
when an adapter controls the canonical prefix layout.


13     Canonical rendering and exact-prefix fragmentation

Durable semantic reuse does not guarantee provider-prefix reuse when the same semantic object is
rendered into different bytes. Variation may arise from field order, timestamps, randomized tool ordering,
path aliases, whitespace, nondeterministic serialization, or model-specific prose.
Let one semantic view admit exact byte renderings r1 , . . . , rm . Suppose independent repeated renders
select ri with probability pi .
Theorem 13.1 (Rendering Fragmentation Penalty). Conditional on provider residency and all other
prefix fields matching, the probability that two independently rendered copies match exactly is
                                                        m
                                                        X
                                                               p2i .
                                                        i=1

A deterministic canonical renderer makes the conditional match probability one. The gain is strict
whenever at least two variants have positive probability.

Proof. The renders match exactly when both draws select the same variant. Summing p2i gives the
probability. Deterministic canonicalization has a single variant of probability one.

Corollary 13.2 (Uniform fragmentation). If m variants are equally likely, exact-match probability is
1/m. Canonicalization removes a factor-m loss without changing project semantics.

Canonical rendering should cover every provider-visible prefix component: stable system instructions,
tool schemas and order, structured-output schemas, rooted references, and decision-view serialization.
Dynamic identifiers used only for logging should remain out of the prompt when the harness permits
it. This prescription agrees with official provider guidance that cache hits require identical prefixes and
stable tool/schema structure [10, 4, 12].

                                                          13
ZeroStack RACC Causal Cache Research                                                        Draft 5 — 13 August 2026


14     Counterexample-guided dependency refinement

A declared dependency graph may be incomplete. An undeclared runtime influence is therefore treated
as a counterexample, not as an ordinary miss.
Let E ∗ be the finite true dependency-edge set inside a bounded sandbox and Et ⊆ E ∗ the currently
certified graph. Whenever execution of object v observes an undeclared predecessor u, the edge (u, v) is
added and every certificate that relied on the older completeness claim is revoked.
Theorem 14.1 (Finite Dependency-Refinement Convergence). Assume a finite dependency universe,
exact observation of all file, process, environment, network, clock, randomness, and tool influences in the
bounded domain, and fairness: every missing edge capable of affecting a registered execution is eventually
exercised. If each counterexample adds at least one previously missing true edge and no false edge is
certified, then after at most
                                                  |E ∗ \ E0 |
proper refinements the certified graph equals E ∗ on the registered domain.

Proof. Each refinement strictly decreases the finite number of missing true edges and never increases it.
Fairness prevents termination while a reachable missing edge remains.

Observed edges improve ranking before convergence, but observation alone does not prove the absence
of unseen edges. Until completeness is certified, omitted regions remain Unknown and cannot receive
exact-cache authority.


15     Validity leases and revocable authority

Exact payload identity and permission to reuse the payload are different. A stored object can remain
byte-exact after its verifier version, protected scope, project epoch, or policy lease expires.
Definition 15.1 (Cache authority lease). A lease binds an object root to a protected scope, dependency
epoch, verifier contract, and validity interval or revocation set. Expired objects may remain readable for
audit, but cannot authorize execution or publication without revalidation.
Theorem 15.2 (Lease-Revocation Safety). If lease expiry removes an optimized cache object from the
authoritative strategy set while exact expansion and the baseline path remain available, expiry cannot
create a protected regression. It can only reduce reuse and increase cost.

Proof. The expired optimized strategy is no longer publishable. Every remaining authoritative strategy,
including the baseline, was already available. Protected quality therefore remains at least baseline; only a
cheaper opportunity may disappear.

This separates semantic validity from residency and policy. A wall-clock lease may be operationally useful,
but time alone should not be confused with causal freshness.


16     A precise meaning of “never break the cache”

Let total cache failure be attributed to:
                           Itotal = Iself + Iprovider + Iproject + Icontract + Istorage ,

                                                        14
ZeroStack RACC Causal Cache Research                                                Draft 5 — 13 August 2026


where the terms denote self-induced historical rewrite, provider eviction/routing, real project change,
model/tool/policy change, and loss of retained storage.
CCNF can make
                                                  Iself = 0.
Durable indexed retention can prevent Iprovider from becoming project rediscovery. Causal invalidation
minimizes the safe blast radius of Iproject . Exact keys handle Icontract . Replication and integrity checking
address Istorage . No honest system can force all five to zero for every open-world workload.


  Novelty and epistemic status.
  Append-only logs, prompt-prefix caching, content-addressed stores, dependency graphs, incremental
  builds, early cutoff, and exact binomial confidence are established ideas. The proposed RACC-Q99
  contribution is their protected two-plane composition around a harness backend, including the explicit
  separation of self-induced prefix failure from provider eviction, the provider-miss insulation ratio,
  weighted and windowed causal Q99, and the exact work required to restore Q99 after a high-impact
  change. Historical novelty remains unresolved pending independent review.


17     Conclusion

Q99 is not achieved by calling every old token a cache hit. It is achieved by changing what the system
retains and what it retransmits. Provider caches are valuable but temporary accelerators over exact
prompt prefixes. ZeroStack stores authoritative project knowledge as exact causal objects, emits stable
capsules before model exposure, and uses append-only interaction so that its own compaction does not
destroy the old prefix. When the provider cache misses, the compact view is reprocessed; the project
is not rediscovered. When the project changes, only the proven causal effect region is invalidated, and
equality boundaries stop propagation. Weighted, windowed, and statistically supported Q99 then become
falsifiable service properties rather than slogans. The moat is durable causal knowledge plus cache-stable
harness interaction.


References

 [1] Umut A. Acar. “Self-Adjusting Computation: An Overview”. In: Proceedings of the ACM SIGPLAN
     Workshop on Partial Evaluation and Program Manipulation. 2009.
 [2] Daniel Anderson, Guy E. Blelloch, Anubhav Baweja, and Umut A. Acar. “Efficient Parallel Self-
     Adjusting Computation”. In: Proceedings of the ACM on Programming Languages 5.PLDI (2021).
     doi: 10.1145/3453483.3454078.
 [3] Anthropic. Claude Platform Pricing: Prompt Caching. 2026. url: https://docs.anthropic.com/
     en/docs/about-claude/pricing (visited on 08/13/2026).
 [4] Anthropic. Prompt Caching. 2026. url: https://docs.anthropic.com/en/docs/build-with-
     claude/prompt-caching (visited on 08/12/2026).
 [5] C. J. Clopper and E. S. Pearson. “The Use of Confidence or Fiducial Limits Illustrated in the Case
     of the Binomial”. In: Biometrika 26.4 (1934), pp. 404–413. doi: 10.1093/biomet/26.4.404.
 [6] Google. Context Caching: Gemini API. 2026. url: https://ai.google.dev/gemini-api/docs/
     generate-content/caching (visited on 08/12/2026).


                                                     15
ZeroStack RACC Causal Cache Research                                           Draft 5 — 13 August 2026


 [7] Erika Kettleson and OpenAI. Prompt Caching 201. Feb. 18, 2026. url: https://developers.
     openai.com/cookbook/examples/prompt_caching_201 (visited on 08/13/2026).
 [8] Neil Mitchell. “Shake Before Building: Replacing Make with Haskell”. In: Proceedings of the 17th
     ACM SIGPLAN International Conference on Functional Programming. 2012, pp. 55–66. doi:
     10.1145/2364527.2364538.
 [9] Andrey Mokhov, Neil Mitchell, and Simon Peyton Jones. “Build Systems á la Carte”. In: Proceedings
     of the ACM on Programming Languages 2.ICFP (2018), 79:1–79:29. doi: 10.1145/3236774.
[10] OpenAI. Prompt Caching in the API. 2026. url: https://developers.openai.com/api/docs/
     guides/prompt-caching (visited on 08/12/2026).
[11] Edwin B. Wilson. “Probable Inference, the Law of Succession, and Statistical Inference”. In: Journal
     of the American Statistical Association 22.158 (1927), pp. 209–212. doi: 10.1080/01621459.1927.
     10502953.
[12] xAI. Prompt Caching: Best Practices and FAQ. 2026. url: https://docs.x.ai/developers/
     advanced-api-usage/prompt-caching/best-practices (visited on 08/12/2026).
[13] xAI. Prompt Caching: Usage and Pricing. May 10, 2026. url: https://docs.x.ai/developers/
     advanced-api-usage/prompt-caching/usage-and-pricing (visited on 08/13/2026).




                                                  16


---

## SOURCE: `current/model_ingest/text/pdf_text/archive/extracted/Draft5/papers/05_ZeroStack_Draft5_Implementation_Requirements.txt`

                ZeroStack Draft 5 Implementation Requirements
   Complete Program Contract for Harness Execution, RACC-R, Q99, and Causal Caching


                                                   Aditya G
                                  ZeroStack / TokenZero Research Program


                                        Draft 5 — 13 August 2026


 Research status and contribution. This document is an implementation contract, not a claim that the
 described program already exists or that the current repository uses these names. It enumerates the semantic
 objects, constructors, functions, state machines, authority boundaries, instrumentation, tests, and release
 criteria required for an existing RACC-R/ZeroStack codebase to realize the Draft 5 papers. It intentionally
 avoids fabricating a complete Rust implementation. The names are descriptive and may be mapped onto
 current modules. What is mandatory is behavior: exact rooted state, stable first-emission capsules, persistent
 causal objects, dependency-complete invalidation, decision-boundary returns, isolated candidate execution,
 proof-carried authority, native fallback, and complete accounting. Each requirement is tied to a theorem or
 an empirical claim so implementation work can begin incrementally and falsify the research program as it
 develops.


                                                   Abstract

        ZeroStack is a model-agnostic backend invoked by an agent harness through programmatic tool
    calling. To test the paper claims, the program must do more than compress text. It must bind tasks
    and runtime contracts, snapshot project state, maintain a content-addressed object store, build and
    validate a causal graph, construct decision-sufficient views, preserve append-only provider prefixes,
    execute mechanical operation graphs privately, isolate candidate mutations, capture exact deltas and
    dependencies, verify current and successor states, issue short-lived commit authority, fall back to native
    tools, and seal a complete resource ledger. This document provides the build contract. It defines
    required subsystems; canonical records and roots; constructor and checker semantics; the Zero Execute
    state machine; causal cache lookup, invalidation, early cutoff, and materialization planning; provider
    and harness adapters; transactional edit and commit behavior; verification and epistemic outcomes;
    Q99 and Pareto instrumentation; capability lifecycle; fault-injection tests; and a staged implementation
    sequence beginning with an observable read-only vertical slice. The document is intended to be mapped
    1:1 onto the developing program’s existing architecture rather than copied as a replacement codebase.



Contents

1 System scope and non-goals                                                                                      3

2 Required subsystem map                                                                                          3

3 Canonical records that must exist semantically                                                                  5
  3.1   Identity and contract records . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .         5
  3.2   Project and object records . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .        5

                                                        1
ZeroStack RACC Causal Cache Research                                                  Draft 5 — 13 August 2026


   3.3   Model-visible and continuation records . . . . . . . . . . . . . . . . . . . . . . . . . . . . .       5
   3.4   Execution and authority records . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .        6
   3.5 Accounting and research records . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .          6

4 Required constructors and pure checkers                                                                       6

5 Zero Execute authoritative state machine                                                                      8

6 Causal cache behavior to implement                                                                            9
   6.1   First-emission stable capsules . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .     9
   6.2   Logical versus physical cache state . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .      9
   6.3   Lookup path . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .      9
   6.4   Invalidation and early cutoff . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .    9
   6.5   Materialization planning . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 10

7 Decision-view and one-/two-call requirements                                                                 10

8 Harness adapter requirements                                                                                 11

9 Provider cache instrumentation                                                                               11

10 Verification and epistemic authority                                                                        12

11 Complete resource instrumentation                                                                           12

12 Q99 and Pareto reports to implement                                                                         13

13 Capability registry requirements                                                                            13

14 Mandatory tests and fault injection                                                                         14
   14.1 Canonical and object integrity . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 14
   14.2 Prefix and capsule behavior . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 14
   14.3 Causal graph and cache . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 14
   14.4 Decision boundaries . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 14
   14.5 Transactions . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 14
   14.6 Accounting . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 14
   14.7 Cross-harness conformance        . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 15

15 Normative backlog coverage                                                                                  15



                                                       2
ZeroStack RACC Causal Cache Research                                                 Draft 5 — 13 August 2026


16 Implementation sequence for immediate testing                                                            15
    16.1 Stage 0: baseline observability . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 15
    16.2 Stage 1: immutable snapshots and exact object store . . . . . . . . . . . . . . . . . . . . . 15
    16.3 Stage 2: stable read-only capsules . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 16
    16.4 Stage 3: causal index and invalidation . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 16
    16.5 Stage 4: Zero Execute private composition . . . . . . . . . . . . . . . . . . . . . . . . . . . 16
    16.6 Stage 5: transactional edits . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . 16
    16.7 Stage 6: Q99 service and provider-miss experiments . . . . . . . . . . . . . . . . . . . . . 16
    16.8 Stage 7: Pareto and capability reinvestment . . . . . . . . . . . . . . . . . . . . . . . . . . 16

17 Minimum validating vertical slice                                                                        16

18 Acceptance criteria for the first public implementation                                                  17

19 Conclusion                                                                                               17


1    System scope and non-goals

The program is a backend used by harnesses. It may be called through an in-process API, local RPC,
CLI/stdio, native plugin, Code Mode, or optional MCP adapter. It is not required to modify model
weights, tokenizer, hidden states, or inference kernels.
The implementation must support the paired baseline

                                 same model + same harness + native tools

and retain that path at all times.
Non-goals for the first validating implementation:

• universal semantic equivalence for arbitrary programs;
• forced one-call execution for tasks with unresolved decisions;
• provider-cache control beyond documented APIs;
• a single global dependency graph claimed complete for every task;
• automatic publication from heuristic confidence;
• hiding backend work behind token-only metrics;
• replacing existing project modules solely to match names in this document.


2    Required subsystem map



                                                       3
ZeroStack RACC Causal Cache Research                                             Draft 5 — 13 August 2026



   Subsystem                   Required responsibility
   Harness adapter             Register ZeroStack tools, bind session/workspace identity, translate
                               structured calls and results, preserve continuation handles, expose
                               native fallback.
   Task-contract binder        Canonicalize request, model/harness/tool/reasoning contract, project
                               root, criteria, side-effect policy, verifier scope, and budget.
   Canonical root service      Deterministic serialization, hashing, root verification, versioning, and
                               domain separation.
   Project snapshot service    Immutable project roots, exact object reads, branch/child snapshots,
                               epochs, and atomic parent transition.
   Content-addressed store     Store immutable payloads, manifests, formation receipts, retention
                               state, integrity checks, tier placement, and garbage-collection reacha-
                               bility.
   Causal graph/index          Track project entities, constructors, declared and observed dependen-
                               cies, task-relative demanded closures, graph versions, and counterex-
                               amples.
   Causal cache engine         Key formation, exact lookup, validity checks, invalidation, early cutoff,
                               materialization planning, reuse accounting, and Q99 state.
   Decision-view builder       Construct protected-sufficient capsules, exact references, unresolved
                               alternatives, expansion handles, and size/accounting projections.
   Zero Execute orchestrator   Run the task state machine, build mechanical operation DAGs, sched-
                               ule private work, return at decision boundaries, and preserve continu-
                               ation state.
   Sandbox/execution service   Isolate     candidate      effects,     restrict    permissions,   trace
                               reads/writes/processes/network/environment, and capture all
                               generated effects.
   Delta service               Compute canonical complete parent-to-child delta, verify preimages,
                               bind actual effects, and reject incomplete capture.
   Verification service        Run task-specific checks, compare against baseline/specification, re-
                               turn Safe/Unsafe/Unknown, and check successor-state obligations.
   Authority service           Issue narrow short-lived execution/commit leases bound to roots,
                               delta, scope, verifier result, nonce, epoch, and expiry.
   Fallback/reserve service    Maintain native-tool and baseline execution availability within the
                               declared budget.
   Provider cache observer     Record provider cached/uncached tokens, cache writes/reads, reten-
                               tion settings, routing keys, and prefix fingerprints.
   Resource ledger             Measure every declared token, call, byte, operation, compute, latency,
                               storage, verification, preparation, miss, and fallback coordinate.
   Capability registry         Store optional verified evidence plans/operators, scope, dependencies,
                               benefit, invalidation, revalidation, and retirement.
   Audit/replay service        Event-source authoritative transitions, reproduce certificate checks,
                               and project concrete traces into the abstract state machine.
   Benchmark exporter          Emit paired manifests, anonymized traces, Q99 windows, Pareto
                               intervals, and release artifacts.




                                                   4
ZeroStack RACC Causal Cache Research                                          Draft 5 — 13 August 2026


3     Canonical records that must exist semantically

Names may differ, but the program needs equivalents of the following.


3.1   Identity and contract records

• CanonicalRoot: algorithm, domain tag, schema version, digest bytes.
• ModelContract: provider/model/version, tokenizer, reasoning settings, sampling, tool rendering,
  endpoint/backend identity.
• HarnessContract: harness/version, system/developer prompts, tool schemas and order, ap-
  proval/sandbox settings, transcript policy.
• ToolchainContract: executable paths/roots, versions, libraries, containers, platform, environment
  allowlist.
• ProtectedScope: acceptance criteria, observable dimensions, verifier versions, Unknown dimensions,
  publication policy.
• ResourceBudget: per-coordinate limits, baseline reserve, timeout, cancellation, and failure policy.
• TaskContract: request bytes/root, initial project root, all contracts, side-effect policy, protected
  scope, and budget.


3.2   Project and object records

• ProjectSnapshot: project root, parent root, epoch, workspace identity, object manifest, branch
  identity, creation receipt.
• ExactObjectRef : payload root, object type, size, encoding, store location, retention state, access
  policy.
• CausalKey: constructor identity, contract root, ordered dependency roots, declared external-input
  roots.
• FormationReceipt: causal key, payload root, producer execution root, canonical encoding root,
  timestamp outside semantic identity, signature/MAC where used.
• CausalNode: node identity, object type, current key/value root, dependencies, reverse dependencies,
  validity and epistemic state.
• GraphVersion: graph root, schema/plugin versions, completeness scope, observed-counterexample
  set.
• DemandClosure: request/task root, demanded nodes, weights by resource coordinate, derivation
  receipt.


3.3   Model-visible and continuation records

• DecisionCapsule: stable model-visible content, supported claims, exact reference handles, unresolved
  choices, verifier state, size/token projections.
• ExpansionHandle: opaque public ID mapped to exact semantic root, allowed queries, expiry/policy,
  and audit identity.

                                                  5
ZeroStack RACC Causal Cache Research                                            Draft 5 — 13 August 2026


• ContinuationHandle: task root, project root, evidence root, candidate root, verification root, ledger
  root, epoch, and scope.
• DecisionRequest: choice schema, alternatives, decision-relevant evidence, consequences, and contin-
  uation handle.
• ZeroResult: Completed, DecisionRequired, EvidenceExpansionRequired, VerificationUnknown, Base-
  lineFallbackRequired, or RejectedNoMutation.


3.4   Execution and authority records

• OperationNode: exact operation type, inputs, dependency edges, policy branch, resource estimate,
  and effect class.
• OperationDAG: nodes, edges, critical path metadata, declared semantic boundaries, and scheduler
  receipt.
• Sandbox: child project root, isolation policy, allowed effects, trace sink, and cleanup state.
• ExecutionTrace: actual reads, writes, processes, network, environment, outputs, exit status, times-
  tamps, resources, and trace root.
• ExactDelta: parent root, child root, created/modified/deleted/renamed objects, metadata, generated
  artifacts, and canonical delta root.
• VerificationReceipt: verifier contract, exact candidate/delta/successor roots, observations, verdict,
  uncovered dimensions, proof/test artifacts.
• AuthorityLease: action class, parent root, epoch, exact delta/successor root, scope, verifier receipt,
  nonce, issued/expiry, single-use state.
• CommitReceipt: previous/new root, consumed authority, linearization point, durable-write status,
  and audit root.


3.5   Accounting and research records

• ResourceLedger: raw counts for every resource coordinate plus attribution to preparation, hit, miss,
  fallback, verification, and maintenance.
• ProviderCacheReceipt: request prefix root, provider/model, routing/cache key, retention setting,
  cached/uncached/write tokens, price and latency.
• CausalReuseReceipt: demand closure, valid reused set, invalidated set, early-cutoff set, recomputed
  set, weights, and Q99 result.
• ParetoReport: paired baseline/optimized ledgers, confidence intervals, feasible hit-rate intersection,
  protected quality result, and gap closure.
• ClaimRecord: claim ID, statement, mathematical/systems/statistical/novelty statuses, evidence,
  falsifier, and release version.


4     Required constructors and pure checkers

Every authority-bearing object must be produced by a deterministic checker rather than caller-supplied
booleans.

                                                   6
ZeroStack RACC Causal Cache Research                                         Draft 5 — 13 August 2026



   Semantic operation           Required behavior
   Canonicalize                 Serialize supported values deterministically, reject ambigu-
                                ous/noncanonical forms, include schema/domain version.
   Root                         Produce domain-separated digest over canonical bytes; never hash
                                lossy display text as semantic identity.
   Verify root                  Recompute and compare; distinguish missing, malformed, mis-
                                match, and unsupported algorithm.
   Bind task                    Construct        TaskContract      from     request,       project,
                                model/harness/tool/reasoning contracts, criteria, side effects, and
                                budget.
   Snapshot project             Produce immutable project root and manifest; reject unresolved
                                filesystem errors or nondeterministic enumeration.
   Resolve exact object         Return byte-identical payload matching root or a typed
                                miss/corruption result.
   Form causal key              Bind constructor, complete contract, ordered dependencies, and
                                external inputs.
   Verify formation             Prove stored payload was formed under the key; key equality alone
                                is insufficient.
   Validate graph scope         Check graph/plugin versions, project root, observed dependency
                                coverage, and task scope; return Unknown when incomplete.
   Derive demand closure        Identify task-relevant demanded nodes and declared weights; retain
                                derivation receipt.
   Compute invalidation         Calculate dependency-complete descendant closure after changed
                                keys/contracts.
   Apply early cutoff           Stop propagation only after exact boundary-value equality under
                                compatible constructors/contracts.
   Plan materialization         Propose cached/recomputed frontier minimizing declared cost;
                                proposal may be heuristic.
   Verify materialization       Independently prove all demanded outputs are in executable closure
                                of known/cached sources.
   Build decision capsule       Include protected-sufficient facts, stable handles, unresolved
                                choices, and exact support roots.
   Check capsule sufficiency    Domain-specific exact or conservative check; otherwise return Un-
                                known and allow expansion.
   Resolve expansion            Map opaque handle to exact authorized evidence without changing
                                old model-visible history.
   Build operation DAG          Separate mechanical operations from unresolved decisions; bind
                                policies and side effects.
   Schedule DAG                 Respect dependencies, cancellation, budget, isolation, and deter-
                                ministic aggregation.
   Create sandbox               Produce        isolated    child    state    with      enforceable
                                read/write/network/process policy.
   Trace effects                Capture every actual dependency and effect relevant to causal keys
                                and exact delta.
   Derive exact delta           Compare parent and child roots after all genera-
                                tors/formatters/tools; include undeclared effects.


                                                 7
ZeroStack RACC Causal Cache Research                                             Draft 5 — 13 August 2026



    Semantic operation            Required behavior
    Run verifier                  Bind exact candidate/delta/successor and return Safe, Unsafe, or
                                  Unknown with evidence.
    Check successor               Establish protected future-state obligation or require base-
                                  line/Unknown.
    Issue authority               Only trusted verifier boundary may issue; bind all roots, scope,
                                  nonce, epoch, expiry, and action.
    Commit CAS                    Atomically consume a live authority only if parent root/epoch and
                                  exact delta still match.
    Fallback                      Invoke same-harness native path under reserved resources; preserve
                                  task and accounting scope.
    Seal ledger                   Prevent later mutation; link every cost to task, path, and evidence
                                  roots.
    Compute Q99                   Calculate named provider/indexed/hot/interface/complete-work
                                  metrics; never conflate them.
    Compute Pareto                Use paired ledgers and uncertainty intervals; return feasible, infea-
                                  sible, or insufficient-data.



5    Zero Execute authoritative state machine

The implementation must make states and permitted transitions explicit.

 1. Idle: no bound task.
 2. Bound: TaskContract validated.
 3. Snapshotted: exact initial project root available.
 4. Resolved: continuation/index/cache state resolved or typed miss known.
 5. Viewed: decision capsule or expansion request constructed.
 6. WaitingDecision: model/user semantic choice required.
 7. Planned: mechanical OperationDAG and effect scope bound.
 8. Executing: isolated child state active.
 9. DeltaSealed: actual complete delta rooted.
10. Verifying: current and successor obligations running.
11. Authorized: short-lived authority issued.
12. Committed: atomic parent transition completed.
13. Recovered: optimized path abandoned and baseline/native path restored.
14. Rejected: no authoritative mutation.
15. Closed: ledger and audit roots sealed.




                                                   8
ZeroStack RACC Causal Cache Research                                                Draft 5 — 13 August 2026


Forbidden transitions include WaitingDecision to Executing without an applicable policy, Executing
directly to Committed, Unknown to Authorized, stale Authorized to Committed, and any failed state
that mutates the parent.


6     Causal cache behavior to implement

6.1   First-emission stable capsules

Large tool results must be captured before repeated model exposure. The first model-visible representation
is the stable capsule and handle. Subsequent evidence is appended. The implementation should detect
and record any adapter behavior that rewrites earlier messages, tool schemas, or ordering because it may
break provider prefixes.


6.2   Logical versus physical cache state

Each object needs independent state for:

• causal validity;
• durable logical retention;
• local-disk residency;
• memory residency;
• remote replica availability;
• provider-prefix residency where observable.

A logical hit may still require physical bytes. Provider hits are never used as authority for project validity.


6.3   Lookup path

A causal lookup must:

1. canonicalize constructor and contract;
2. resolve every dependency root;
3. construct the complete causal key;
4. locate the action/object entry;
5. verify formation and payload integrity;
6. verify current validity and policy;
7. return exact object plus reuse receipt, or a typed miss/Unknown.


6.4   Invalidation and early cutoff

On changed object, constructor, tool, environment, or policy roots:


                                                      9
ZeroStack RACC Causal Cache Research                                          Draft 5 — 13 August 2026


1. mark direct key mismatches invalid;
2. traverse reverse dependencies within the complete graph;
3. schedule recomputation/verification in task-weight priority;
4. compare recomputed boundary roots with old roots;
5. stop propagation at exact-equality boundaries;
6. update Q99 restoration state and all affected capsules;
7. preserve unaffected objects and cross-branch identical roots.


6.5   Materialization planning

The planner must support at least:

• direct cache retrieval;
• recomputation from dependencies;
• source reread;
• mixed frontiers;
• cost vectors rather than one scalar;
• exact tree dynamic programming for validation fixtures;
• independent executable-closure verification for every proposed plan.

A cheaper plan that fails closure is not eligible.


7     Decision-view and one-/two-call requirements

The capsule builder must distinguish:

• exact supported facts;
• inference labeled as inference;
• unresolved semantic choices;
• verifier conclusions;
• evidence omitted because it is mechanical/recoverable;
• evidence omitted because it is unavailable or Unknown.

The model-visible result must contain enough information to choose among the protected decision classes
or provide exact expansion. A one-call completion is permitted only when no unresolved semantic choice
remains. A two-call path returns exactly one decision object and then continues from the continuation
handle.
The program should record why every model return occurred:



                                                     10
ZeroStack RACC Causal Cache Research                                           Draft 5 — 13 August 2026


• semantic decision;
• uncovered observation branch;
• evidence expansion requested by model;
• verifier Unknown;
• user approval or subjective judgment;
• fallback.

This record is required to test the decision-round theorem.


8    Harness adapter requirements

Every adapter must:

• register a stable minimal ZeroStack tool surface;
• preserve opaque continuation/expansion handles exactly;
• avoid adding volatile timestamps or identifiers to stable cached prefixes when possible;
• maintain deterministic tool schema and ordering;
• map cancellation and timeout to nonauthority states;
• preserve session/workspace/model identities;
• expose native tools alongside ZeroStack;
• report argument/result tokens or enough raw text for offline counting;
• never derive commit authority from transport success alone;
• support semantic conformance tests across adapters.

At least one low-friction adapter should be implemented first, but the backend semantic API must remain
transport-neutral.


9    Provider cache instrumentation

Provider instrumentation must record dated policy inputs and per-request observations:

• provider, endpoint, model, organization/project;
• cache retention/TTL setting;
• prompt/cache key or conversation routing key;
• exact prefix fingerprint and token count;
• tool schema/order fingerprint;
• reasoning configuration fingerprint;
• cached, uncached, and cache-write tokens when exposed;

                                                  11
ZeroStack RACC Causal Cache Research                                              Draft 5 — 13 August 2026


• request price and latency;
• warmup timing and inactivity gap;
• miss classification: expected first use, prefix mismatch, routing, expiry/eviction, model/tool change,
  or unknown.

The instrumentation should support the three paired history modes: full raw outputs, retrospective
rewrite, and reference-first append-only capsules.


10     Verification and epistemic authority

Every protected claim must produce one of:


                                    Safe,     Unsafe,      Unknown.

Unknown may trigger more evidence, a stronger verifier, user/model decision, or baseline fallback. It may
not issue optimized publication authority.
Verifier plugins should declare:

• input and output root types;
• protected dimensions covered;
• soundness model and known blind spots;
• dependencies and version roots;
• deterministic/nondeterministic behavior;
• required resources;
• exact evidence/proof artifact;
• whether they prove current-result, successor-state, or both.

For coding tasks, verifier combinations may include parse/build, tests, type/static analysis, differential
execution, property tests, fuzzing, sanitizers, API compatibility, performance budgets, security checks,
and project-specific invariants.


11     Complete resource instrumentation

The ledger must record at least:

• model-visible tool calls, arguments, results, expansions, and control tokens;
• provider uncached, cached-read, and cache-write tokens;
• model reasoning allowance and observed reasoning tokens where exposed;
• visible output tokens;
• backend reads/writes/searches/traversals/processes/network requests;


                                                   12
ZeroStack RACC Causal Cache Research                                              Draft 5 — 13 August 2026


• logical and physical cache hits/misses;
• bytes by harness, provider, local storage, remote storage, and network;
• CPU/GPU time, wall latency, critical path, queueing, and concurrency;
• verification work;
• preparation/indexing/maintenance;
• failed speculation, rollback, and fallback;
• storage occupancy, replication, eviction, and garbage collection.

All costs need path attribution: baseline, prepared hit, provider miss with L2 hit, causal miss, novel path,
failed optimized path, or maintenance.


12     Q99 and Pareto reports to implement

The program must emit separate reports for:

 1. provider prefix hit ratio;
 2. indexed causal reuse ratio by declared weight;
 3. hot physical residency ratio;
 4. Boundary-Q99 model-visible token ratio;
 5. windowed Q99 and immediate post-change reuse;
 6. time/work to restore Q99;
 7. multi-resource feasible hit-rate interval;
 8. Frontier Closure terms;
 9. unavoidable lower-bound components;
10. certified redundant-gap closure Γ;
11. protected regression and strict-rescue rates.

Each report must include denominator, scope, task family, project root range, time window, statistical
interval, missing data, and direct falsifiers.


13     Capability registry requirements

A verified reusable capability/evidence plan needs:

• exact origin episode and proof artifacts;
• preconditions and project/task scope;
• dependency and contract roots;
• declared reads, writes, effects, and verifier plan;

                                                     13
ZeroStack RACC Causal Cache Research                                             Draft 5 — 13 August 2026


• fallback/rollback;
• capture and maintenance cost;
• reuse count, savings, strict rescues, rejections, and failures;
• invalidation/revalidation state;
• transfer policy across branches, projects, harnesses, and models;
• retirement threshold and reason.

Capabilities are optional candidates. They cannot remove the native baseline and cannot certify themselves.


14     Mandatory tests and fault injection

14.1    Canonical and object integrity

Test deterministic serialization, schema migration, domain separation, corrupted payloads, mismatched
formation receipts, missing dependencies, ordering changes, and cross-platform canonicalization.


14.2    Prefix and capsule behavior

Test first-emission capsule stability, append-only expansion, adapter attempts to rewrite old history,
tool-schema reorder, volatile metadata placement, and exact prefix fingerprints.


14.3    Causal graph and cache

Test leaf/central/config/toolchain/environment changes, omitted dependency injection, early cutoff,
branch convergence, invalid object reuse rejection, tree-DP optimality, general-DAG closure, and Q99
restoration.


14.4    Decision boundaries

Test zero, one, and multiple unresolved decisions; unhandled policy branches; hidden evidence that
changes the model’s choice; exact expansion; and native-tool escape.


14.5    Transactions

Test crashes at every state, stale roots, expired/replayed authority, concurrent commits, incomplete delta,
sandbox escape, verifier disagreement, and rollback failure.


14.6    Accounting

Test token/byte double counting, missing provider fields, hidden preparation, background maintenance,
failed speculation, cache-write versus read cost, and false Q99 denominator selection.




                                                    14
ZeroStack RACC Causal Cache Research                                              Draft 5 — 13 August 2026


14.7    Cross-harness conformance

Run identical semantic tasks through at least three adapters and compare project roots, semantic results,
authority receipts, and ledgers modulo transport coordinates.


15     Normative backlog coverage

The release accompanies this paper with a detailed implementation contract and machine-readable backlog.
The backlog currently contains 110 independently auditable requirements: 65 P0 correctness/authority
requirements, 39 P1 operational/Q99 requirements, and 6 P2 scale/maturity requirements. They cover
the trusted kernel, task and model/harness contracts, baseline sovereignty, harness adapters, continuation
state, FSZero storage and snapshots, GraphZero indexing, TokenZero decision views, private Code Mode
composition, verification, causal caching, Pareto accounting, verified capability, security, operations, and
benchmarks.
Every backlog entry names:

1. the exact semantic behavior that must exist;
2. its theorem or systems-premise dependency;
3. the requirements it depends on;
4. a direct acceptance or fault-injection test;
5. a status field to be mapped onto the existing repository.

The normative source is implementation/ZEROSTACK_IMPLEMENTATION_REQUIREMENTS_DRAFT5.md; the
sortable execution backlog is implementation/IMPLEMENTATION_BACKLOG_DRAFT5.csv. A separate audit
template records the concrete crate/module/file, actual function/type/event, test evidence, and missing
semantics for each requirement. This prevents the paper from inventing a new codebase while still making
the paper-to-program mapping exhaustive.


16     Implementation sequence for immediate testing

16.1    Stage 0: baseline observability

Before optimization, instrument the ordinary harness path. Capture calls, transcript tokens, project
reads/writes, model/provider usage, latency, and task outcome. Without a sealed baseline ledger, no
savings claim is testable.


16.2    Stage 1: immutable snapshots and exact object store

Implement canonical roots, project snapshots, exact object reads, payload integrity, formation receipts,
retention, and audit events. No causal reuse claim should precede this layer.




                                                    15
ZeroStack RACC Causal Cache Research                                              Draft 5 — 13 August 2026


16.3    Stage 2: stable read-only capsules

Implement capture-before-model-exposure, stable capsules, expansion handles, append-only adapter
behavior, and provider-prefix instrumentation. First test “what does this program do?” and indexed
read/search tasks without mutation.


16.4    Stage 3: causal index and invalidation

Add symbol/file/test/build/config graph nodes, reverse dependencies, observed runtime dependencies,
task-relative demand closure, descendant invalidation, equality cutoff, and reuse receipts. Start with one
language/toolchain whose dependency semantics can be audited.


16.5    Stage 4: Zero Execute private composition

Build operation DAGs for read/search/graph/build/test workflows, schedule privately, return at decision
boundaries, preserve continuation handles, and expose native fallback. Measure one-/two-call coverage.


16.6    Stage 5: transactional edits

Add child sandboxes, full effect tracing, exact delta, verifier plugins, successor checks, authority leases,
compare-and-swap commit, and crash/race tests. Do not permit authoritative edits before this stage
passes.


16.7    Stage 6: Q99 service and provider-miss experiments

Add sliding-window reuse, time-to-Q99, provider expiration/routing tests, crossover analysis, tree materi-
alization validation, and complete L1/L2/L3 metrics.


16.8    Stage 7: Pareto and capability reinvestment

Compute robust feasible hit-rate intervals, Frontier Closure, lower-bound closure, and baseline reserve.
Only then test additional candidates or verified capabilities funded by measured savings.


17     Minimum validating vertical slice

The smallest useful end-to-end implementation should support:

1. one harness adapter;
2. one project language;
3. exact immutable project snapshot;
4. file/symbol/reference/test index;
5. stable capsule and exact expansion;
6. one high-level read-only Zero Execute task;

                                                    16
ZeroStack RACC Causal Cache Research                                             Draft 5 — 13 August 2026


 7. provider cache instrumentation;
 8. same-model native baseline trace;
 9. sealed resource ledger;
10. controlled file changes and causal invalidation;
11. provider-miss insulation test;
12. weighted/windowed reuse report.

This slice can test the core moat before editing authority is implemented: Does persistent indexed state
replace repeated ls/grep/read interaction, remain available after provider cache loss, and invalidate only
the affected project region without changing the model’s supported explanation?


18     Acceptance criteria for the first public implementation

A first release should not claim full RACC-Sovereign or complete-work Q99. It should require:

• no silent Unknown-to-Safe transitions;
• deterministic roots and validated formation receipts;
• exact expansion of every model-visible reference;
• no self-induced retrospective history rewrite in the supported adapter;
• provider and indexed hits reported separately;
• dependency omissions surfaced by the supported observation scope;
• no stale causal reuse in adversarial fixtures;
• same-model/same-harness paired measurements;
• complete interface, backend, and storage ledgers;
• a documented native fallback path;
• published negative results and unsupported scopes.


19     Conclusion

The implementation target is not a magic compressor. It is an exact, stateful harness backend with a small
trusted authority boundary. The program must make project knowledge durable, causal, addressable, and
cheap to render; preserve the model’s genuine decisions and native strategies; execute mechanical work
privately; and measure every cost. The requirements above turn Draft 5 from a paper architecture into
a testable build contract. They are intentionally modular: baseline observability and read-only causal
caching can begin immediately, while transactional edits, strong verification, Q99 service guarantees, and
capability compounding are added only after their prerequisites are instrumented and attacked.




                                                       17
