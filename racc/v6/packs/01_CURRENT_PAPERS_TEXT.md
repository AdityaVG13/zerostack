# Current Draft 6 Papers - Extracted Text

This is a convenience pack. The source paths and hashes in the corpus manifest remain authoritative.


---

## SOURCE: `current/model_ingest/text/pdf_text/current/papers/00_ZeroStack_RACC_Cumulative_Core_Draft6.txt`

                  ZeroStack RACC Cumulative Core Draft 6
Harness-Relative Compression, Protected Decision Boundaries, and Certified Pareto Closure

                                                Aditya G

                                      Draft 6 – 14 August 2026


  Research status
  This paper is the current mathematical core of the cumulative ZeroStack RACC program.
  It consolidates the harness correction introduced in Draft 3, the cache-stability and causal-
  invalidation results developed in Drafts 4 and 5, and the no-degradation and Pareto-closure
  results inherited from earlier RACC and RACC-R work. The primary object is not a model-
  internal compressed state. It is the model-visible interaction trace produced by an unchanged
  model operating through an ordinary agent harness. Mathematical results are proved in
  finite or explicitly stated models. Production claims remain conditional on rooted project
  identity, protected decision-view sufficiency, exact expansion, baseline strategy inclusion,
  verifier soundness, future-safe successor publication, and complete accounting. Historical
  novelty is unresolved; the proposed contribution is the exact composition of these elements
  into a persistent harness-side backend.

                                                 Abstract
         A coding model can spend hundreds of thousands of model-visible tokens repeatedly listing,
     searching, reading, editing, building, testing, and inspecting a project even when only a small
     number of genuinely semantic decisions occur. Recovery-Aware Context Compression (RACC)
     reframes this inefficiency as a harness-level trace problem. ZeroStack is a persistent backend
     called through a stable programmatic tool interface. It retains exact project state, indexes causal
     structure, privately composes mechanical operations, and returns protected-sufficient decision
     views with exact expansion and native-tool escape. We prove a protected decision-view minimality
     theorem, a harness decision-boundary compression theorem, an adaptive decision-round lower
     bound, and a one-/two-call normal form. We then establish stable-reference recovery, same-
     harness capability inclusion, guarded nonregression, and a complete-work frontier decomposition.
     The central result is not that arbitrary information disappears into one token. It is that repeated
     project discovery and tool plumbing can be relocated behind a durable recovery boundary while
     the model retains every baseline reasoning strategy. A defensible 100-percent endpoint is defined
     as closure of the certified redundant-work gap rather than elimination of unavoidable reasoning,
     novelty, verification, or external effects.


Contents
1 Architecture and comparison object                                                                       2

2 Protected decision views                                                                                 2

3 Harness decision-boundary compression                                                                    3

                                                     1
ZeroStack RACC Draft 6                                                                        Aditya G


4 Adaptive decisions and the one-/two-call normal form                                                 4

5 Snap-to-edit and exact recovery                                                                      4

6 Capability preservation and guarded publication                                                      5

7 Reasoning sovereignty                                                                                5

8 Frontier closure                                                                                     6

9 Forty-token illustration                                                                             6

10 Conclusion                                                                                          7


1     Architecture and comparison object
RACC is an umbrella program for reducing repeated context and interaction cost while retaining
exact recovery and protected capability. In Draft 6, the canonical comparison is

                      B = same model + same harness + ordinary native tools,

and
                                Z = B + optional ZeroStack backend.
The backend may be called through Code Mode, a native harness package, local RPC, CLI/stdio,
or an optional MCP adapter. The transport is not the theorem. The semantic backend is.
    The model is unchanged. It retains responsibility for intent interpretation, architecture, tradeoffs,
and any decision not uniquely determined by the user contract or a sound verifier. ZeroStack may
recover exact state, search indexes, traverse dependencies, run tools, stage candidate effects, verify,
restore, and commit. It may not silently choose between protected-distinct semantic outcomes and
call that compression.
Definition 1.1 (Protected task order). For task x, let Yx be publishable outcomes and let ⪯x
be the declared protected “no worse than” relation. The relation may bind exact behavior, tests,
API compatibility, security, performance, formatting, factual support, human approval, or another
explicitly scoped criterion.
Definition 1.2 (Baseline strategy set). Let ΠB (x) be the strategies available to the same model in
the same harness with ordinary native tools, evidence, reasoning settings, and stopping policy. Let
ΠZ (x) be the strategies available after ZeroStack is added.
    The no-degradation architecture requires a trace-capable injection

                                           ΠB (x) ,→ ΠZ (x).

ZeroStack may add strategies; it cannot remove the baseline path.


2     Protected decision views
Let Oi be the complete observation available immediately before semantic decision i. Two observa-
tions are protected-equivalent when every baseline decision rule relevant to the declared task either
chooses the same protected action or can request exact expansion before choosing.

                                                   2
ZeroStack RACC Draft 6                                                                      Aditya G


Definition 2.1 (Protected decision equivalence). For decision point i, define
                                                  o ∼i o′
when the complete protected strategy set available from o is reconstructible from o′ and conversely,
including exact evidence expansion before authority is exercised.
Definition 2.2 (Decision view). A decision view is a map vi : Oi → Vi together with exact expansion
authority. It is protected-sufficient when
                                     vi (o) = vi (o′ ) =⇒ o ∼i o′ .
Theorem 2.3 (Protected Decision-View Minimality). Let Oi be finite. The quotient Oi / ∼i is the
smallest exact protected decision-view alphabet up to relabeling. Any exact view must assign different
labels to different quotient classes, and the quotient itself is sufficient.
Proof. If an exact view merged two observations in different quotient classes, then some protected
strategy or expansion requirement would differ, contradicting exactness. Therefore every exact view
refines the quotient and has at least as many labels. Mapping each observation to its quotient class
is sufficient by definition of ∼i .

    The theorem identifies the object that should be compressed: not arbitrary source bytes, but
the information required at the next semantic boundary. Exact source remains recoverable behind
rooted references.


3    Harness decision-boundary compression
A baseline tool trace can be written
                                       τB = d0 σ1 d1 σ2 · · · σm dm ,
where di are model-semantic decisions and σi are infrastructure segments such as listing, indexed
search, exact reads, deterministic graph traversal, formatting, build, test, and result collection.
Definition 3.1 (Privately composable segment). A segment σi is privately composable when:
1. it begins from the same rooted state as the baseline segment;
2. every internal branch is mechanical, independently verifiable, or covered by a contingent policy
   already supplied by the model or user;
3. it performs no unauthorized authoritative effect;
4. its final view is protected-sufficient for the next decision;
5. exact expansion and baseline escape remain available;
6. failure returns Unknown, a counterexample, or the baseline path.
Theorem 3.2 (Harness Decision-Boundary Compression). Replacing each privately composable σi
by one ZeroStack macro-transition preserves the protected decision opportunities and final protected
outcome of every baseline strategy represented in the treatment.
Proof. Proceed by induction over decision boundaries. At the first boundary both executions begin
from the same rooted state. Private composability guarantees that the replacement segment reaches
a view in the same protected decision class, or returns to exact expansion/baseline before a decision.
Therefore every baseline choice remains available. The chosen action induces the next rooted
segment under the same contract. Repeating the argument preserves each boundary and the final
protected outcome. Unauthorized effects are excluded by premise.

                                                     3
ZeroStack RACC Draft 6                                                                        Aditya G


   If the baseline exposes K = i |σi | primitive model-visible operations and ZeroStack exposes
                                  P

one macro-call per segment, the call reduction is
                                                         m
                                                 1−        .
                                                         K
The hidden backend work remains real and is charged separately.


4    Adaptive decisions and the one-/two-call normal form
Definition 4.1 (Unresolved adaptive decision). An adaptive decision is unresolved when a future
observation can require two protected-distinct actions and neither the initial request, a supplied
contingent policy, nor a sound verifier uniquely selects the action.

Theorem 4.2 (Adaptive Decision-Round Lower Bound). A protocol preserving all baseline strategies
must return to the model at least once for every unresolved adaptive decision encountered before
completion.

Proof. Suppose the backend does not return at such a decision. It must select one action without a
pre-specified policy or unique verifier conclusion. Because at least two protected-distinct actions
remain possible, one baseline strategy is removed. This contradicts strategy inclusion.

Corollary 4.3 (One-/Two-Call Normal Form). Let DZ (x) be the number of unresolved adaptive
decisions after the initial request and any supplied contingent policy. When all other work is privately
composable,
                                          NZ (x) = DZ (x) + 1.
Thus DZ (x) = 0 gives one-call completion and DZ (x) = 1 gives two-call completion.

   The theorem explains both the moat and its boundary. Repository size, file count, and primitive
operation count need not determine model-visible calls. Genuine decision depth does.


5    Snap-to-edit and exact recovery
Let C(q, s, π) be the task-relative causal lens for request q, project root s, and candidate plan π. Let
E ∗ (q, s, π) be evidence required to judge the plan and W ∗ (q, s, π) possible immediate write targets.

Theorem 5.1 (Snap-to-Edit Sufficiency). If the indexed lens is completeness-certified and contains

                                       E ∗ (q, s, π) ∪ W ∗ (q, s, π),

then no primitive discovery call is required before the model can propose a concrete edit for π.

Proof. Every evidence item and write target required by the stated plan is already represented in
the exact rooted lens, with expansion authority. A preliminary list, grep, or read would add no
required information. If π itself is unresolved, the first view may instead present the protected
architecture decision.

Definition 5.2 (Stable reference). A stable reference binds an opaque harness-visible handle to an
exact project object, contract, root, and expansion authority. The handle is not commit authority.



                                                     4
ZeroStack RACC Draft 6                                                                       Aditya G


Theorem 5.3 (Stable-Reference Recovery). If a continuation or expansion handle resolves under
the same rooted contract to the exact bound object, replacing repeated object transmission with the
handle preserves exact recoverability.
    This is the harness-side meaning of recovery-aware compression: repeated project state becomes
a stable reference plus an incremental decision, not a lossy summary that must silently stand in for
the original.


6    Capability preservation and guarded publication
Theorem 6.1 (Same-Harness Capability Superset). If native tools, the full baseline invocation, the
baseline reasoning allowance, and sufficient fallback reserve remain available, then

                                          ΠB (x) ,→ ΠZ (x).

Consequently, for any protected utility u,

                                      sup u(π) ≥         sup      u(π).
                                    π∈ΠZ (x)           π∈ΠB (x)

Proof. The treatment contains a trace-equivalent copy of each baseline strategy. Taking a supremum
over a superset cannot reduce the optimum.

   This is a capability theorem, not a controller theorem. The system must still prevent a controller
from selecting a worse added strategy.
Theorem 6.2 (Guarded Protected Nonregression). Let bx be the exact baseline result. Suppose an
optimized candidate zx publishes only with a sound certificate that

                                                bx ⪯x zx

and, for stateful work, that the successor remains in the protected future-simulation relation. Other-
wise the system publishes/restores bx . Then the deployed result D(x) satisfies

                                               bx ⪯x D(x).

Proof. If the candidate is certified, the relation holds by verifier soundness. Otherwise the output is
the baseline. Future safety follows from the successor premise.

   The theorem is scope-bound. Unsupported semantic dimensions remain Unknown and cannot
acquire publication authority.


7    Reasoning sovereignty
Definition 7.1 (Reasoning sovereignty). A deployment preserves reasoning sovereignty when the
treatment’s maximum reasoning allowance, tool access, evidence expansion, and stopping freedom
are no weaker than the baseline’s.
    ZeroStack may reduce repeated project-context tokens and tool transcripts while keeping the
reasoning budget unchanged. If actual hidden reasoning consumption is positive and included in
total token work, it creates a nonzero all-token floor. Preserving reasoning tokens and simultaneously
claiming those same tokens were eliminated is inconsistent.

                                                   5
ZeroStack RACC Draft 6                                                                      Aditya G


8    Frontier closure
Let, over a campaign of n tasks:
• Bn > 0 be same-scope baseline work per task;
• Pn /n amortized preparation;
• Hn prepared-path work;
• Fn novel or fallback work;
• νn the novel/fallback fraction.
Then
                                       Pn
                                  Cn =    + (1 − νn )Hn + νn Fn
                                       n
and
                                                   Cn
                                         Sn = 1 −     .
                                                   Bn
Theorem 8.1 (Frontier Closure). Under nonnegative work terms,
                                               Sn → 1
if and only if
                         Pn            (1 − νn )Hn           νn Fn
                             → 0,                  → 0,            → 0.
                        nBn                Bn                 Bn
Proof. The normalized optimized cost is the sum of the three nonnegative normalized burdens. A
sum of nonnegative sequences converges to zero exactly when each term converges to zero.
    The theorem identifies the engineering frontier: preparation amortization, prepared-path cost,
and novelty/fallback mass. New mathematics is useful when it tightens one of these terms or proves
a lower bound.
Definition 8.2 (Certified redundant-work closure). Let L be a sound lower bound on unavoidable
same-scope work, with L ≤ Z ≤ B and B > L. Define
                                                 B−Z
                                            Γ=       .
                                                 B−L
Theorem 8.3 (Certified 100-percent endpoint).
                                        Γ = 1 ⇐⇒ Z = L.
    This is the defensible 100-percent claim: the system removed all work the current certificate
proves avoidable. Necessary request information, genuine semantic decisions, protected reasoning,
verification, novel output, and external effects belong in L.


9    Forty-token illustration
If the same-harness baseline inserts JB = 1,000,000 model-visible tool-interface tokens and a prepared
ZeroStack exchange inserts JZ = 40, the exact interface-coordinate reduction is
                                              40
                                     1−             = 99.996%.
                                          1,000,000
Two forty-token exchanges give 99.992%. These figures do not include hidden backend work unless
that work is explicitly added to the same resource coordinate. Their scientific significance is that
the model-visible transcript can approach the irreducible decision boundary while exact project
knowledge persists elsewhere.

                                                  6
ZeroStack RACC Draft 6                                                                    Aditya G


10    Conclusion
The cumulative core is a conservation result rather than a magic compression claim. Information
needed by the model remains visible or exactly expandable. Mechanical work remains executed and
accounted. The frontier moves because stable indexed project state and private composition prevent
the same information and operations from being repeatedly transported through the model. The
unchanged model keeps its reasoning and native-tool strategies, while the backend can add verified
evidence, execution, and reusable capability. One-/two-call operation is therefore a statement about
decision depth and recoverable state, not about hiding the world from the model.




                                                 7


---

## SOURCE: `current/model_ingest/text/pdf_text/current/papers/01_Zero_Execute_Harness_Runtime_Draft6.txt`

                     Zero Execute Harness Runtime Draft 6
                    A Model-Agnostic Backend for Persistent Project Work

                                                Aditya G

                                      Draft 6 – 14 August 2026


  Research status
  This paper specifies ZeroStack as a harness-side runtime rather than a model-integrated
  mechanism. It refines the architectural correction introduced in Draft 3 and consolidates the
  process models developed in Drafts 4 and 5. The claims concern transport-neutral semantics,
  private composition, decision boundaries, transactional effects, and exact continuation. They
  do not require modified weights, tokenizer changes, key–value state installation, or a special
  inference kernel. MCP is one possible adapter; it is not the architecture. Production
  equivalence across adapters remains conditional on canonical task contracts, exact project
  roots, faithful rendering, native-tool escape, and a conforming authority boundary.

                                                Abstract
        Agent harnesses expose models to repositories through repeated primitive calls. The resulting
    interaction may be dominated by mechanical listing, searching, reading, build, test, and edit
    plumbing rather than semantic reasoning. This paper defines Zero Execute, a transport-neutral
    operation by which a harness invokes a persistent ZeroStack backend. The backend resolves
    exact project state, constructs a task-relative causal lens, privately composes eligible operations,
    and returns a compact decision view or a verified result. We define request and result semantics,
    opaque continuation handles, adapter fidelity, task processes for explanation, refactoring, cross-
    language porting, and greenfield construction, and an atomic effect protocol. We prove transport
    factorization, continuation sufficiency, decision-preserving private composition, no-mutation
    failure, and cross-harness semantic reuse under rooted contracts. The model retains full reasoning
    authority and can request exact expansion or native tools at every semantic boundary. The
    runtime learns through scoped verified assets rather than by silently altering model behavior.


Contents
1 Zero Execute as the harness boundary                                                                     2

2 Transport factorization                                                                                  2

3 Continuation handles                                                                                     3

4 Private composition                                                                                      3

5 Project explanation                                                                                      4

6 Repository refactoring                                                                                   4


                                                     1
ZeroStack RACC Draft 6                                                                  Aditya G


7 Cross-language porting                                                                        5

8 Greenfield construction                                                                       5

9 Transactional execution                                                                       5

10 Verified capability accumulation                                                             6

11 Operational architecture                                                                     6

12 Conclusion                                                                                   6


1    Zero Execute as the harness boundary
The model-facing surface should be small and stable. A harness may expose one operation named
zero.execute, zero, or another equivalent symbol. The name and wire format are adapter details.
The semantic request binds:
• task and protected criteria;
• exact project/workspace root;
• optional continuation handle;
• operation class and user objective;
• model decision or contingent policy when present;
• side-effect policy;
• model, harness, tool, reasoning, and verification contracts;
• private-composition and resource budgets;
• baseline reserve and fallback policy.
   The result is one of:
• Completed;
• DecisionRequired;
• EvidenceExpansionRequired;
• VerificationUnknown;
• BaselineFallbackRequired;
• RejectedNoMutation.
   The result taxonomy makes epistemic limits operational. An incomplete verifier or missing
causal edge does not become a vague success response.


2    Transport factorization
Let S be the harness-independent semantic state and let Ra be the rendering/transport function for
adapter a. Let Da parse a rendered request back into canonical semantics.

Definition 2.1 (Faithful adapter). An adapter a is faithful when, for every in-scope semantic
request s,
                                      Da (Ra (s)) = s,
and the adapter preserves cancellation, timeout, Unknown, baseline escape, result ordering, and
resource-accounting semantics.




                                                2
ZeroStack RACC Draft 6                                                                          Aditya G


Theorem 2.2 (Harness-Transport Factorization). If adapters a and b are faithful and invoke the
same rooted backend transition T , then their protected backend results are equivalent up to declared
rendering differences:
                                     Da−1 (T (s)) ∼P Db−1 (T (s)).

Proof. Both adapters decode to the same canonical semantic request s. The backend transition
is determined by the same rooted state and contract. Faithful rendering preserves the protected
result fields and authority semantics. Therefore any differences are confined to declared adapter
rendering.

   This permits Pi, Codex CLI, Claude CLI, Cursor, Grok Build, RPC, CLI/stdio, or MCP
wrappers to share the same L2 project objects and state machine.


3    Continuation handles
A continuation handle resolves to a rooted backend state

          κt = H(task, project, evidence, candidate, verification, ledger, contracts, epoch).

The harness may display a short opaque identifier. The backend retains the full root and authorization
scope. A handle is not execution or commit authority.

Theorem 3.1 (Continuation Sufficiency). Suppose κt resolves exactly to the complete in-scope
backend state χt under the same task, project, ABI, security, and semantic contracts. Then a
subsequent request need carry only κt and the new decision/request information to reproduce every
protected backend transition available from χt .

Proof. Exact resolution reconstructs χt . The new request supplies the only additional information.
The backend transition therefore has the same inputs as a full retransmission. Compatibility checks
prevent use under a different contract.

   Handles may survive process restart and compatible harness changes. They fail closed on stale
roots, incompatible ABI, cross-project scope, forged identity, or revoked epoch.


4    Private composition
Code Mode and programmatic tool calling can execute dependent operations inside a single outer
call rather than returning every intermediate object to the model. Cloudflare’s Code Mode is one
public example of this general interaction pattern [1]. ZeroStack adds durable project-specific state
and authority semantics.

Definition 4.1 (Private composition eligibility). A sequence is eligible when all internal choices are
deterministic, verifier-resolved, or covered by a contingent policy; no intermediate observation is
needed for an uncovered model decision; all effects are sandboxed or separately authorized; and the
final view is protected-sufficient.

Theorem 4.2 (Decision-Preserving Private Composition). Replacing an eligible primitive trace by
one private backend execution preserves the set of protected model decisions and outcomes.




                                                  3
ZeroStack RACC Draft 6                                                                        Aditya G


Proof. By eligibility, no hidden intermediate observation requires an uncovered semantic choice.
Mechanical results are exactly incorporated into the final view, and any failure becomes Unknown
or returns to the model. Sandboxing prevents hidden authoritative mutation. Thus the model loses
no protected strategy.

  The backend must return DecisionRequired when eligibility ceases. This is the runtime
manifestation of the adaptive decision-round lower bound.


5    Project explanation
For “what does this program do?”, the backend:
1. binds the request and factual-support criteria;
2. constructs an exact project snapshot;
3. resolves entry points, module relationships, configuration, data/control paths, tests, and runtime
   evidence supported by adapters;
4. builds a task-relative evidence graph;
5. returns a compact explanation view with exact rooted references;
6. expands any disputed factual claim before publication.

Theorem 5.1 (Explanation Evidence Preservation). If every factual claim in a compact explanation
is supported by an exact rooted source/runtime artifact or explicitly labeled inference, and all omitted
evidence remains expandable before a protected factual decision, then the compact interface does not
reduce the baseline factual strategy set.

   The theorem does not guarantee that the model writes the best explanation. It guarantees that
compression does not remove the evidence authority needed to match the baseline.


6    Repository refactoring
A refactor task binds:
• objective and architectural constraints;
• public API/behavior preservation;
• build, test, security, and performance criteria;
• allowed side effects;
• subjective or unverifiable dimensions;
• baseline fallback.
   The backend resolves the affected closure and returns only genuine architecture choices. After
a model decision, it stages typed or preimage-bound effects in a child sandbox, runs formatters/-
generators/build/tests, derives the complete delta, verifies the result and successor, and atomically
commits or restores.

Theorem 6.1 (Decision-Delimited Refactor). If a refactor contains d unresolved adaptive semantic
decisions and all other operations are privately composable and verifiable, then the prepared model-
visible interaction requires exactly d + 1 Zero Execute calls.

    A task such as a fully specified symbol rename may have d = 0. A refactor with one public-API
design choice may have d = 1. A complex architecture task may require more.




                                                   4
ZeroStack RACC Draft 6                                                                        Aditya G


7    Cross-language porting
A port cannot be protected merely by compiling the target language. The task contract should
enumerate observable obligations:
• inputs, outputs, and errors;
• state transitions and side effects;
• serialization and persistent formats;
• numerical behavior and tolerances;
• ordering, concurrency, and timing obligations;
• API/CLI compatibility;
• platform, build, package, performance, and memory requirements.
   Let B be declared source-behavior obligations and V ⊆ B the obligations currently verified.
Theorem 7.1 (Port Nonregression under Complete Observational Coverage). If V = B, the verifier
is sound, the target satisfies every obligation in V , and the source baseline remains available for
uncovered environment cases, then the published target is protected-equivalent within the declared
observational contract.
   When V =  ̸ B, the unverified obligations remain Unknown. The backend may still deliver a
milestone, but it may not claim complete equivalence.


8    Greenfield construction
For a new game or application, there is no preexisting complete implementation to replay. The
paired baseline is the same model/harness using ordinary tools and the full current project state.
ZeroStack preserves:
• milestone roots and accepted design decisions;
• scene/entity/module structure;
• build and platform state;
• deterministic tests and performance budgets;
• exact rollback and branch history;
• human authority over subjective quality.
Theorem 8.1 (Greenfield Strategy Preservation). If every ZeroStack suggestion, capability, or plan
is optional, exact project evidence remains expandable, native tools remain available, and subjective
decisions remain with the model/user, then adding the backend cannot remove a baseline construction
strategy.
   Quality improvement remains empirical and must be measured by strict rescues, not assumed
from lower token counts.


9    Transactional execution
Candidate work is isolated from authority. Let s be the parent root, ŝ a child sandbox, ∆ the exact
derived delta, and a a short-lived authority lease.
Theorem 9.1 (Atomic No-Mutation Failure). Suppose all candidate effects occur in ŝ; commit
requires a valid lease binding s, the current epoch, ∆, verifier and successor receipts; and publication
uses expected-parent compare-and-swap. Then every failed, stale, raced, canceled, or rejected attempt
leaves the authoritative root equal to s.

                                                   5
ZeroStack RACC Draft 6                                                                       Aditya G


Proof. No candidate effect touches authority before commit. A failed lease or comparison cannot
perform the single authoritative root transition. Therefore the parent remains unchanged.

   A successful transition publishes the complete verified successor root. Crash recovery replays
the append-only event log to either the old or new root, never a partial mixture.


10     Verified capability accumulation
The runtime may capture accepted episodes as proposals for reusable assets. Each asset records
scope, preconditions, reads, writes, effects, postconditions, verifier, successor relation, rollback,
dependencies, freshness, and maintenance cost.

Theorem 10.1 (Verified Capability Compounding). Let An be the set of fresh verified optional
capabilities after stage n, with An ⊆ An+1 . If the baseline remains in every candidate set and the
selector publishes only protected-safe candidates, then optimal protected utility is nondecreasing with
n. If the minimum complete cost among safe strategies is used, that minimum is nonincreasing
while capabilities remain fresh.

Proof. Each later candidate set is a superset of the earlier set. The maximum utility over a superset
cannot decrease and the minimum cost over a superset cannot increase. Freshness and guarded
publication preserve admissibility.

   Invalidation can remove an asset from the fresh set, so maintenance and drift must be measured.
The theorem does not justify uncontrolled self-modification.


11     Operational architecture
The persistent backend should avoid spawn-per-call overhead and mixed worker generations. Stable
payloads and volatile receipts are separated. Workers are pinned by exact digests and matched
manifests. Default idle operation targets negligible overhead; heavier indexing is scheduled, measured,
and interruptible. Errors are typed and actionable. No silent fallback is permitted: fallback is an
explicit result with a complete ledger.


12     Conclusion
Zero Execute is the missing product boundary between the theory of recoverable context and
practical agent work. The model does not need to be modified. It needs a harness tool whose
backend already knows the project, can execute mechanical work without transcript flooding, and
returns at the exact points where intelligence is required. The moat is therefore persistent project
cognition at the harness boundary: exact enough to recover, narrow enough to communicate, and
guarded enough to preserve every baseline strategy.


References
[1] Cloudflare. Code Mode: the better way to use MCP. 2025. url: https://blog.cloudflare.
    com/code-mode-mcp/ (visited on 08/14/2026).




                                                  6


---

## SOURCE: `current/model_ingest/text/pdf_text/current/papers/02_RACC_Q99_Causal_Residency_Draft6.txt`

                     RACC-Q99 Causal Residency Draft 6
   Prefix-Stable Interaction, Durable Logical Memory, and Capacity-Certified Caching

                                              Aditya G

                                     Draft 6 – 14 August 2026


  Research status
  This paper is the current cache theory for the ZeroStack harness backend. It preserves Draft
  5’s exact longest-common-prefix correction, Causal Cache Normal Form, dependency-complete
  invalidation, equality-boundary cutoff, weighted Q99, and provider-miss insulation. Draft 6
  adds an exact physical-residency feasibility theorem, an eviction-slack theorem, and a bounded
  provider-miss amplification measure. Mathematical identities are proved in finite sequence,
  graph, and capacity models. Runtime claims remain conditional on canonical identity,
  complete causal keys, formation receipts, dependency completeness, storage integrity, correct
  demand denominators, and independent checker authority. Q99 is never a universal claim:
  every result names its cache layer, resource coordinate, workload, weights, and measurement
  window.

                                               Abstract
        Provider prompt caches accelerate exact or breakpoint-compatible request prefixes, but
    residency, routing, policy, and historical rewriting can cause misses. A project backend needs
    a different authority model. RACC-Q99 separates provider prefix reuse (L1), durable logical
    causal reuse (L2), and physical materialization (L3). Large tool results are captured before
    model exposure and represented by immutable decision capsules with exact expansion handles,
    producing append-only model-visible histories. Derived project objects are keyed by constructors,
    contracts, dependency roots, and formation receipts. Project changes invalidate only the
    dependency-complete affected cone, with exact equality boundaries stopping further propagation.
    We restate the exact retrospective-rewrite and horizon crossover theorems, then introduce a
    Causal Residency Budget Theorem: L3 Q99 is feasible exactly when a capacity-bounded resident
    set retains at least 99 percent of valid demand mass. We derive an eviction-slack guard and
    quantify provider-miss insulation. These results define the moat precisely: provider eviction may
    make a compact view cold, but it need not erase durable project knowledge or force repeated
    repository discovery.


Contents
1 Three cache layers                                                                                     2

2 Retrospective rewriting                                                                                2
  2.1 Horizon-aware economics        . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .   3

3 Causal Cache Normal Form                                                                               3

4 Logical causal identity                                                                                4

                                                   1
ZeroStack RACC Draft 6                                                                     Aditya G


5 Dependency-complete invalidation                                                                 4
  5.1 Equality-boundary cutoff . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .   5

6 Weighted causal Q99                                                                              5

7 Draft 6 causal residency budget                                                                  5

8 Q99 eviction slack                                                                               6

9 Provider-controlled residency transposition                                                      6

10 Cross-session, branch, and harness reuse                                                        7

11 Reporting discipline                                                                            7

12 Conclusion                                                                                      7


1     Three cache layers
A ZeroStack deployment distinguishes:
1. L1 provider prefix cache: acceleration over provider-recognized request prefixes;
2. L2 logical causal cache: durable exact project objects whose causal identity remains valid;
3. L3 physical residency: RAM, local disk, remote storage, or replica placement of L2 objects.
    These layers have different miss semantics. An L1 miss means a compact prompt prefix may be
processed again. An L3 miss means a valid L2 object must be fetched or rematerialized. An L2
miss or invalidity means project knowledge must be recomputed or reverified.
    Official provider documentation illustrates the volatility and heterogeneity of L1. OpenAI
documents exact prompt-prefix reuse and recommends append-only histories and stable tool ordering
for cache preservation [3, 4]. Anthropic offers five-minute and one-hour prompt-cache durations [1].
xAI documents misses after eviction and reports cached-token observations [5]. Google supports
implicit and explicit context caching with explicit-cache lifetime controls [2]. These services are
useful accelerators; none should be treated as the sole authority for durable project state.


2     Retrospective rewriting
Let a previously cached model-visible history be

                                           H = A∥X∥B,

where X is an old tool result and B is the later suffix. A router replaces X by summary S:

                                           H ′ = A∥S∥B.

Let
                       r = lcp(X∥B, S∥B),      n = |X|,    s = |S|,   b = |B|.

Theorem 2.1 (Exact Retrospective Rewrite Characterization).

                                       lcp(H, H ′ ) = |A| + r.


                                                   2
ZeroStack RACC Draft 6                                                                     Aditya G


The exact rewritten residual beyond the reusable prefix is

                                               s + b − r.

A self-induced prefix break occurs exactly when r < s + b.
Proof. The common prefix A cancels, leaving the longest common prefix of the two residuals. The
rewritten residual has length s + b, so the portion beyond the common prefix has length s + b − r.

   The theorem corrects the overly broad claim that every historical shortening necessarily destroys
the entire later suffix. It frequently does, but the exact common-prefix length controls the result.
   Let ρ be the relative cached-read price and u the uncached or cache-write price. Ignoring the
common cost of A,
                                            Ckeep = ρ(n + b)
and
                                     Crewrite = ρr + u(s + b − r).
Theorem 2.2 (Cache-Compaction Crossover). Retrospective compaction is cheaper in the modeled
coordinate exactly when
                             ρr + u(s + b − r) < ρ(n + b).

    The prompt can become dramatically shorter and still become more expensive when a small
early rewrite converts a large cached residual into uncached processing.

2.1     Horizon-aware economics
Let T future requests share the history, cr be cached-read unit cost, cw first-use rewritten process-
ing/write cost, and cc one-time compaction cost. Then

                                        Ckeep (T ) = T cr (n + b),

while
                        Crewrite (T ) = cc + (cw − cr )(s + b − r) + T cr (s + b).
Theorem 2.3 (Horizon-Aware Rewrite Break-Even). Rewriting wins exactly when

                               cc + (cw − cr )(s + b − r) < T cr (n − s).

    A router must therefore estimate not only current prompt length, but also destroyed suffix, price
ratios, compaction work, and expected reuse horizon.


3     Causal Cache Normal Form
Definition 3.1 (Causal Cache Normal Form). A large artifact X is represented as

                                CCNF(X) = (KX , FX , PX , CX , EX ),

where KX is the complete causal key, FX a formation receipt, PX the immutable payload/root, CX
the stable model-visible capsule, and EX exact expansion authority.
   The capsule is emitted before raw tool output enters the model-visible history. Later expansion
appends evidence rather than rewriting the capsule.

                                                    3
ZeroStack RACC Draft 6                                                                   Aditya G


Theorem 3.2 (First-Emission Prefix Immunity). If

                                          Ht+1 = Ht ∥∆t ,

then Ht remains an exact prefix of every later history. CCNF creates no self-induced historical
prefix rewrite.

Corollary 3.3 (Zero Self-Rewrite Component). When prefix misses are decomposed into self-rewrite,
provider residency/routing, contract changes, and new suffix work, the self-rewrite component is
zero under append-only CCNF and stable rendering.

    This is the correct meaning of “never break the cache”: ZeroStack cannot prevent provider
eviction, but it can avoid invalidating its own history through retrospective replacement.


4     Logical causal identity
For derived object v, define

    Kv = H(constructor, contract, dependency roots, canonical parameters, relevant environment).

A formation receipt binds Kv to the execution record and exact payload root.

Theorem 4.1 (Causal Cache Soundness). Assume canonical serialization, collision-resistant roots
in the systems model, complete causal keys, deterministic or receipt-bound constructors, and valid
formation receipts. If two accepted objects have the same causal key and receipt-verified payload
identity, they are interchangeable for the declared protected use.

   A bare key lookup is insufficient. The receipt proves the payload was actually formed under
that key.


5     Dependency-complete invalidation
Let G = (V, E) be a finite directed dependency graph, with edges from premises to derived objects.
For changed nodes ∆, let Desc∗ (∆) be the reflexive descendant closure.

Theorem 5.1 (Dependency-Complete Invalidation). Under complete deterministic dependency
semantics, every derived object outside Desc∗ (∆) retains the same causal key and exact value after
changes confined to ∆.

Proof. Every path by which a changed premise could influence a derived object follows declared
edges. An object outside the descendant closure has no such path. Its dependencies and constructor
contract are unchanged, so its key and value remain unchanged.

    The hard premise is graph completeness. A discovered undeclared influence revokes dependent
certificates and becomes a refinement counterexample.




                                                 4
ZeroStack RACC Draft 6                                                                     Aditya G


5.1    Equality-boundary cutoff
Let boundary B separate the changed region from downstream outputs. Suppose recomputation
yields
                                 H(x′b ) = H(xb ) ∀b ∈ B.

Theorem 5.2 (Equality-Boundary Early Cutoff). If B is a complete separating boundary and every
boundary root is unchanged, all deterministic downstream objects retain their previous exact values.
Invalidation propagation may stop at B.

Proof. Every downstream dependency path from the changed region crosses B. Downstream
constructors receive the same boundary values and unchanged other dependencies, so deterministic
outputs remain equal by induction over a topological order.

   This preserves cache state across formatting-only changes, behavior-preserving refactors, and
other modifications whose externally relevant boundaries remain identical.


6     Weighted causal Q99
For request q, let D(q) be the demanded causal closure, wi > 0 demand weights, and I the
invalid/Unknown demanded objects. Define
                                                    P
                                                           i∈D(q)∩I wi
                                    RL2 (q) = 1 − P                      .
                                                            i∈D(q) wi

Theorem 6.1 (Weighted Causal Q99).

                                           RL2 (q) ≥ 0.99

if and only if
                                               wi ≤ 0.01
                                       X                      X
                                                                      wi .
                                    i∈D(q)∩I                 i∈D(q)

Corollary 6.2 (High-Impact Novelty Impossibility). If unresolved invalid demanded mass exceeds
one percent and no independent equality or replacement proof exists, exact L2 Q99 cannot be claimed
until recomputation or verification reduces that mass.

   A public service metric should use sliding windows to avoid hiding post-change collapse inside a
long average.


7     Draft 6 causal residency budget
Logical validity does not imply local residency. Let valid demanded objects be indexed by i, with
size si > 0, demand weight wi > 0, and resident decision ri ∈ {0, 1}. Let physical-tier capacity be C
and total valid demand mass
                                            W =
                                                 X
                                                     wi .
                                                       i

Theorem 7.1 (Causal Residency Budget). The tier can satisfy physical Q99 exactly when there
exists r such that                   X
                                        si ri ≤ C
                                               i


                                                   5
ZeroStack RACC Draft 6                                                                    Aditya G


and
                                                     wi ri ≥ 0.99W.
                                              X

                                                i

Equivalently, the optimum of

                               max
                                         X                         X
                                             wi ri    subject to          si ri ≤ C
                             ri ∈{0,1}
                                         i                            i

is at least 0.99W .

Proof. A resident plan is physically feasible exactly when its sizes fit capacity. It satisfies Q99
exactly when its retained demand weight reaches the threshold. Maximizing retained weight over
feasible plans gives the stated equivalence.

   The optimization is a knapsack problem in general. A heuristic may propose a plan, but a simple
independent checker verifies the capacity and threshold inequalities. Optimization and authority
remain separate.

Corollary 7.2 (Uniform-Size Residency). If all objects have equal size and capacity holds k objects,
the maximum resident demand mass is obtained by retaining the k largest weights. The smallest
Q99 cardinality is the shortest descending-weight prefix reaching 0.99W .


8     Q99 eviction slack
Let current resident valid demand mass be WR and define

                                              σ = WR − 0.99W.

For proposed eviction set E, let w(E ∩ R) be resident demand mass removed.

Theorem 8.1 (Q99 Eviction Slack). The eviction is guaranteed to preserve the current Q99
certificate when
                                   w(E ∩ R) ≤ σ.
If removed mass exceeds σ, Q99 is not certified without compensating admissions or a new demand
certificate.

Proof. Post-eviction resident mass is WR − w(E ∩ R). It remains at least 0.99W exactly when the
removed mass is at most WR − 0.99W = σ.

   This theorem turns Q99 from a retrospective dashboard into a pre-eviction guard. Recency may
propose eviction; current demand mass authorizes it.


9     Provider-controlled residency transposition
Let I denote L2 causal validity, M provider/model/prefix matching, and RP provider residency. A
provider hit requires
                                       HP = I ∩ M ∩ RP
when the prompt depends on the same valid project information. A retained ZeroStack logical hit
requires I and storage/authorization, not provider prefix residency.


                                                         6
ZeroStack RACC Draft 6                                                                      Aditya G


Theorem 9.1 (Provider-Miss Insulation). Conditional on valid retained L2 state, an L1 miss may
require processing the compact decision view again, but does not require rediscovering unchanged
project state.

   Let B be baseline model-visible project replay, C the compact view, and L continuation-control
overhead.

Theorem 9.2 (Provider-Miss Bounded Amplification). Conditional on valid L2 state, the model-
visible burden after an L1 miss is bounded by C + L rather than B. The coordinate-specific reduction
is
                                                 C +L
                                             1−
                                                   B
for B > 0.

  Backend fetch, rematerialization, verification, and storage are not included in this ratio and
must be reported separately.


10     Cross-session, branch, and harness reuse
Content roots are semantic identity; branch labels, session IDs, and display paths are not. Exact
objects may be shared across sessions and branches when their content, constructor, contract, and
dependency roots match. Harness-specific rendering is a derived object keyed by its rendering
contract. Authorization remains project/tenant scoped even when physical content deduplicates.

Proposition 10.1 (Branch Convergence Reuse). If two branches produce identical exact roots at a
complete dependency boundary, downstream L2 objects keyed only by that boundary and unchanged
contracts can be shared after convergence.


11     Reporting discipline
A Q99 report must include:
• named layer and resource coordinate;
• demand set, weights, and measurement window;
• validity and Unknown rules;
• physical tier and capacity for residency claims;
• provider observations for L1 claims;
• invalidated and recomputed mass;
• time/work to restore Q99 after change;
• complete backend cost;
• confidence or exactness status.
   A statement such as “99 percent cached” without these coordinates is scientifically underdeter-
mined.


12     Conclusion
The causal-cache moat is not merely longer retention. It is a change in representation and authority.
Provider caches reuse exact sequences for a limited and externally controlled period. ZeroStack retains
project-semantic objects under exact causal identity, invalidates them according to dependency

                                                  7
ZeroStack RACC Draft 6                                                                    Aditya G


topology, and presents a short append-only interface to the provider. Draft 6 adds the missing
physical-residency theorem: Q99 in memory or local storage is a capacity-certified demand-mass
property, and eviction can be guarded before it breaks. Together, L1, L2, and L3 turn cache behavior
from an opaque transient optimization into a layered, auditable system.


References
[1] Anthropic. Prompt caching. 2026. url: https://docs.anthropic.com/en/docs/build-with-
    claude/prompt-caching (visited on 08/14/2026).
[2] Google. Context caching. 2026. url: https://ai.google.dev/gemini-api/docs/generate-
    content/caching (visited on 08/14/2026).
[3] OpenAI. Prompt caching. 2026. url: https://developers.openai.com/api/docs/guides/
    prompt-caching (visited on 08/14/2026).
[4] OpenAI. Prompt Caching 201. 2026. url: https://developers.openai.com/cookbook/
    examples/prompt_caching_201 (visited on 08/14/2026).
[5] xAI. Prompt caching: usage and pricing. 2026. url: https : / / docs . x . ai / developers /
    advanced-api-usage/prompt-caching/usage-and-pricing (visited on 08/14/2026).




                                                 8


---

## SOURCE: `current/model_ingest/text/pdf_text/current/papers/03_ZeroStack_Implementation_Conformance_Draft6.txt`

            ZeroStack Implementation and Conformance Draft 6
              From Recovery-Aware Theorems to an Auditable Harness Backend

                                                  Aditya G

                                        Draft 6 – 14 August 2026


   Research status
   This paper is an implementation contract, not a replacement codebase. It translates the current
   theorem stack into rooted evidence, deterministic checkers, certificates, authority transitions,
   failure semantics, observability, tests, and release gates. The existing ZeroStack/RACC-R
   implementation is assumed to contain substantial work; every obligation must first be mapped
   to actual files, symbols, data structures, and tests. The paper deliberately avoids fabricated
   Rust module layouts. Conformance remains unestablished until the concrete runtime is
   audited, fault-injected, benchmarked against the same-model/same-harness baseline, and
   shown to refine the abstract state machine.

                                                  Abstract
          Mathematical compression claims do not become operational merely because their formulas
      can be translated into arithmetic. A deployable theorem must determine what the runtime
      constructs, what a terminating checker verifies, what certificate is issued, what authority follows,
      what happens on missing evidence, and how the claim is falsified. This paper defines that
      theorem-to-runtime discipline for ZeroStack. We specify the trusted boundary, semantic object
      inventory, event-sourced state machine, four-repository authority split, phased implementation
      sequence, conformance and fault program, and complete resource ledger. We state a concrete
      runtime-refinement theorem: every authoritative implementation transition must project to
      a permitted abstract RACC transition or leave authority unchanged. We then map decision
      compression, causal caching, Q99 residency, transactional effects, no-degradation, and Pareto
      closure into implementable certificate protocols. The result is a direct agenda for extending
      existing Rust projects without confusing untrusted planning with proof authority.


Contents
1 The theorem-to-runtime rule                                                                                2

2 Trusted boundary                                                                                           2

3 Four-repository implementation boundary                                                                    3
  3.1 ZeroStack . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .          3
  3.2 FSZero . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .         3
  3.3 GraphZero . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .          3
  3.4 TokenZero . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .          3

4 Concrete runtime refinement                                                                                3

                                                       1
ZeroStack RACC Draft 6                                                                       Aditya G


5 Semantic object inventory                                                                           4

6 Required checkers                                                                                   5

7 Implementation sequence                                                                             5
  7.1 Audit and baseline . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .    5
  7.2 Identity and epistemic kernel . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .     5
  7.3 Read-only Zero Execute . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .      5
  7.4 Cache factorization and invalidation . . . . . . . . . . . . . . . . . . . . . . . . . . .      5
  7.5 Private composition . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .     5
  7.6 Transactional effects . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .   6
  7.7 Residency and capabilities . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .    6

8 No-degradation conformance                                                                          6

9 Cache conformance                                                                                   6

10 Transactional conformance                                                                          6

11 Complete accounting                                                                                7

12 Conformance and fault catalog                                                                      7

13 Operational gates                                                                                  7

14 Public empirical gate                                                                              7

15 Conclusion                                                                                         7


1    The theorem-to-runtime rule
Every release-level theorem must compile into seven operational elements:
1. Rooted evidence: all variables and premises with exact provenance;
2. Total checker: a deterministic terminating function over that evidence;
3. Certificate or counterexample: machine-revalidatable output;
4. Authority consequence: the only permitted execution/publication implication;
5. Unknown behavior: expansion, refinement, or baseline fallback;
6. complexity and resource charge: checker work included in the ledger;
7. direct falsifier: a test or fault that must cause rejection.
    A theorem that cannot yet supply a finite checker remains a proof target or empirical claim. It
cannot authorize production behavior.


2    Trusted boundary
Untrusted proposers include:
• the model;
• planner and controller;
• retrievers and graph builders;
• cache optimizer and eviction policy;


                                                   2
ZeroStack RACC Draft 6                                                                       Aditya G


• proof/test generators;
• capability extractor;
• harness adapter input.
   The trusted boundary contains the smallest practical set of components:
• canonical serialization and root verification;
• formation-receipt verification;
• exact snapshot/object resolution;
• total Safe/Unsafe/Unknown checkers;
• baseline reserve and restoration;
• sandbox isolation and exact-delta derivation;
• verification and successor check;
• short-lived authority issuance;
• expected-parent atomic commit;
• audit replay and complete ledger sealing.
   The model and planner cannot construct authority objects. Proof-carrying code established the
broader producer-proof/consumer-checker pattern [1]; ZeroStack applies that separation to project
continuations and effects.


3     Four-repository implementation boundary
3.1   ZeroStack
ZeroStack owns the semantic ABI, task contract, harness adapters, orchestration, continuation,
authority, verification coordination, capability registry, accounting, and conformance. It calls domain
engines through rooted interfaces.

3.2   FSZero
FSZero owns immutable bytes, exact objects, snapshots, spans, sandboxes, effect traces, deltas,
rollback, materialization, and compare-and-swap publication. It does not decide relevance, causal
semantics, or model rendering.

3.3   GraphZero
GraphZero owns definitions, references, callers, tests, dependency graphs, causal lenses, completeness
grades, and incremental invalidation. It does not mutate files or publish model messages.

3.4   TokenZero
TokenZero owns stable tool schemas, canonical model rendering, decision capsules, expansion
references, provider-cache observations, and token ledgers. It does not determine filesystem or graph
truth.
    No domain engine imports another. This prevents circular authority and permits matched
worker binaries with exact digests.


4     Concrete runtime refinement
Let A be the abstract RACC state machine, C the concrete implementation, and α : C → A an
abstraction map.

                                                  3
ZeroStack RACC Draft 6                                                                       Aditya G


Definition 4.1 (Conforming concrete step). A concrete step c → c′ conforms when one of the
following holds:
1. α(c) = α(c′ ) and the step is a non-authoritative stutter/evidence operation;
2. α(c) → α(c′ ) is a permitted abstract evidence-refinement transition;
3. the step carries a valid certificate for a permitted authoritative abstract transition;
4. failure leaves authority unchanged or restores a related baseline state.

Theorem 4.2 (Concrete Runtime Refinement). If the initial concrete state maps to a valid
abstract state, every concrete step conforms, crash recovery preserves the relation, and concrete
results/deltas/ledgers project to the abstract transition, then every concrete execution trace projects
to a valid abstract RACC trace. All abstract safety and nonregression invariants therefore hold for
the projected concrete trace.

Proof. Induct over the concrete trace. The initial relation holds by premise. Each step either
preserves the abstract state, advances by an allowed refinement, advances by a certified authoritative
transition, or restores a related state. Therefore the relation is maintained. Projection of outputs
and ledgers preserves the abstract observations. Abstract invariants hold on every reachable abstract
trace.

   The theorem is standard refinement reasoning; its importance is operational. Until the actual
Rust events and authority paths satisfy it, the papers do not prove the implementation.


5     Semantic object inventory
The runtime needs semantic equivalents of:

    Object family             Required meaning
    Identity/contracts        ABI version, canonical root, task, model, reasoning, harness, tool,
                              verification, protected-scope contracts
    Project state             project/snapshot roots, exact objects/spans, dependency roots,
                              graph/lens roots, completeness certificates
    Harness state             request envelope, continuation handle, decision view, stable capsule,
                              expansion handle, action table
    Execution                 sandbox root, plan, effect/read/write trace, exact delta, verifier and
                              successor receipts
    Authority                 baseline authority, execution lease, commit lease, no-mutation and
                              commit receipts
    Cache                     causal key, formation receipt, logical validity, physical residency,
                              provider observation, admission/eviction certificate
    Accounting                raw resource vector, Q99 report, Pareto interval, frontier-closure
                              report
    Capabilities              verified episode, asset, failure syndrome, freshness/revocation receipt

   Names may change. Semantic identity may not be represented by timestamps, display paths,
random row IDs, or mutable object addresses.




                                                  4
ZeroStack RACC Draft 6                                                                     Aditya G


6     Required checkers
The current backlog defines 130 requirements. Critical checkers include:
• canonical-byte and root verifier;
• formation-receipt verifier;
• task/contract compatibility checker;
• decision-view sufficiency checker for finite/domain scope;
• continuation compatibility resolver;
• private-composability/decision-gate checker;
• dependency-closure and completeness checker;
• equality-boundary cutoff checker;
• L2 logical validity checker;
• L3 capacity/Q99 residency checker;
• eviction-slack checker;
• sandbox/effect-scope checker;
• verifier and future-successor checker;
• authority-lease verifier;
• expected-parent CAS;
• complete-resource and Pareto checker;
• capability freshness/revocation checker.
   An optimizer may be sophisticated, learned, or heuristic. The checker should be small, deter-
ministic, and independently testable.


7     Implementation sequence
7.1   Audit and baseline
First map every requirement to actual code. Freeze same-model/same-harness native-tool traces.
Identify current identity, index, cache, execution, rollback, and ledger semantics.

7.2   Identity and epistemic kernel
Implement or verify canonical serialization, roots, receipts, versioned contracts, append-only event
log, and Safe/Unsafe/Unknown. Prohibit ordinary code from constructing authority.

7.3   Read-only Zero Execute
Deliver exact project snapshots, indexed causal lenses, stable decision capsules, expansion handles,
continuation, fallback, and resource receipts. This is the first product milestone.

7.4   Cache factorization and invalidation
Separate L1, L2, and L3. Implement causal keys, formation receipts, invalidation cones, equality
cutoff, forced-miss instrumentation, demand weights, and Q99 reports.

7.5   Private composition
Classify mechanical segments, execute them privately, and return to the model at uncovered decisions.
Measure critical path and model-visible transcript reduction.


                                                 5
ZeroStack RACC Draft 6                                                                     Aditya G


7.6   Transactional effects
Add child sandboxes, exact delta, verification, successor proof, authority leases, and atomic
commit/no-mutation failure.

7.7   Residency and capabilities
Add capacity-certified residency, eviction guards, cross-session deduplication, and finally guarded
capability capture. Learning is last because it depends on every preceding truth and authority layer.


8     No-degradation conformance
A treatment run may claim the no-degradation envelope only when:
1. baseline model, harness, tools, reasoning, and stopping policy are rooted;
2. native tools and full baseline path remain callable;
3. optimization cannot consume the reserve needed for fallback;
4. every hidden operation is decision-preserving;
5. every publication is equivalent/dominating in the declared scope or is the baseline;
6. stateful commits preserve future protected simulation;
7. Unknown dimensions are disclosed.
    An accuracy benchmark alone cannot prove strategy inclusion. Conformance tests must deliber-
ately request baseline escape, exact expansion, and native tools after optimized operation.


9     Cache conformance
A causal-cache hit requires:
• canonical key equality;
• valid formation receipt;
• exact payload/root integrity;
• current dependency/contract validity;
• authorized scope;
• correct layer classification.
    A Q99 certificate additionally requires the exact denominator, weights, window, tier, capacity
where applicable, and post-eviction state. Provider cached tokens are observed, not inferred from
L2.


10     Transactional conformance
The fault program injects crashes and races before and after every verify, authorize, and commit
boundary. Accepted outcomes are:
• complete verified successor root;
• exact old root with no mutation receipt;
• exact baseline restoration.
   Partial mutation, mixed worker generations, silent fallback, stale lease commit, or missing ledger
entries fail conformance.




                                                 6
ZeroStack RACC Draft 6                                                                     Aditya G


11    Complete accounting
The raw resource vector includes at least:

            (model calls, arguments, results, uncached input, cache reads, cache writes,
             reasoning, output, wire bytes, disk bytes, CPU, GPU,
             indexing, verification, latency, storage, failed work, fallback).
    Scalar cost may be reported only with declared nonnegative weights. Raw coordinates remain
visible. Preparation and maintenance are amortized over a declared campaign, never omitted.


12    Conformance and fault catalog
Mandatory attacks include noncanonical encodings, payload/key substitution, missing dependency
edges, stale roots, forged/replayed leases, cross-project handles, provider historical rewrites, L2
corruption, L3 loss, sandbox escape, undeclared effects, verifier disagreement, CAS races, fallback
reserve exhaustion, stale capabilities, adapter truncation, and ledger omission.
    The expected response is Unsafe, Unknown, exact no-mutation, or baseline fallback. A successful
optimized publication under one of these faults is a release blocker.


13    Operational gates
The default persistent backend should not require a heavy daemon. The release target is near-
invisible idle overhead, approximately no more than 0.1% background CPU and 500 MB resident
memory in the default local mode, with heavier indexing scheduled and disclosed. There is no
spawn-per-call runtime, random worker checkout, or mixed-generation rollback. Worker binaries
and manifests are pinned by exact digests.


14    Public empirical gate
Before a production claim, publish:
• same-model/same-harness paired traces;
• one-/two-call decision annotations;
• protected regression and strict-rescue adjudication;
• forced L1/L2/L3 miss matrix;
• causal invalidation and Q99 restoration curves;
• full resource vectors;
• storage and daemon overhead;
• fault-injection results;
• negative findings and unsupported dimensions;
• implementation commit, schemas, manifests, and checksums.


15    Conclusion
The implementation boundary is the scientific boundary. Theorems become useful when they produce
small checkers and narrow authority. ZeroStack should be built so increasingly sophisticated models
and optimizers can propose aggressive compression, causal frontiers, edit plans, and capabilities


                                                   7
ZeroStack RACC Draft 6                                                                      Aditya G


without being trusted to certify themselves. The trusted kernel remains exact, fail-closed, auditable,
and baseline-preserving. That separation is what allows the system to push the Pareto frontier
without turning token savings into hidden capability loss.


References
[1]   George C. Necula. “Proof-Carrying Code”. In: POPL. 1997.




                                                  8


---

## SOURCE: `current/model_ingest/text/pdf_text/current/papers/04_ZeroStack_RACC_Draft6_Research_Agenda.txt`

                  ZeroStack RACC Draft 6 Research Agenda
          Formal Proof, Implementation, Evaluation, and Public Release Program

                                               Aditya G

                                     Draft 6 – 14 August 2026


   Research status
   This is a separate research and release agenda, not part of the archival theorem conclusions. It
   collects unresolved proof obligations, implementation experiments, benchmark requirements,
   novelty review, and kill criteria. Items are not represented as achieved. The agenda is
   intended to evolve while the four Draft 6 papers remain stable research objects.


Contents
1 Formal proof program                                                                                 2

2 Implementation experiments                                                                           2
  2.1 Read-only Zero Execute . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .       2
  2.2 Causal graph completeness . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .        2
  2.3 Cache layers . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .   2
  2.4 Decision depth . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .     2
  2.5 Transactional effects . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . . .    3

3 Workload studies                                                                                     3

4 Q99 research                                                                                         3

5 No-degradation and quality                                                                           3

6 Operational research                                                                                 3

7 Capability research                                                                                  4

8 Novelty and literature review                                                                        4

9 Kill criteria                                                                                        4

10 Research stop rule                                                                                  4

11 Public repository gate                                                                              4




                                                   1
 ZeroStack RACC Draft 6                                                                  Aditya G


 1     Formal proof program
 Priority proofs are:
 1. finite harness decision-boundary trace projection;
 2. adaptive decision-round lower bound;
 3. exact longest-common-prefix rewrite and horizon economics;
 4. dependency-complete DAG invalidation and equality-boundary cutoff;
 5. Q99 residency budget and eviction slack;
 6. multi-resource Pareto interval intersection;
 7. atomic runtime refinement and no-partial-authority;
 8. protected successor simulation;
 9. finite decision-view quotient with exact expansion;
10. certified redundant-work lower-bound composition.
     Lean formalization should use explicit finite structures first. General symbolic or open-world
 claims remain conditional on domain verifiers and receipts.


 2     Implementation experiments
 2.1   Read-only Zero Execute
 Build the smallest complete path from task contract and exact root to indexed decision view,
 expansion, continuation, fallback, and ledger. Measure factual support and transcript reduction
 before any write path.

 2.2   Causal graph completeness
 Combine static and dynamic evidence. Inject undeclared dependencies. Measure completeness
 grades, Unknown rate, refinement convergence, and false-safe rate. A single stale accepted object
 caused by a missing edge is a release blocker for the claimed scope.

 2.3   Cache layers
 Independently force:
 • L1 provider miss with L2/L3 warm;
 • L1 miss with L2 valid and L3 cold;
 • L2 invalidity after project change;
 • storage corruption;
 • branch divergence and convergence.
     Report model-visible replay, backend work, transfer, recomputation, and Q99 restoration sepa-
 rately.

 2.4   Decision depth
 Blindly annotate real baseline traces for genuine observation-contingent semantic choices. Compare
 annotated depth with Zero Execute calls. Publish tasks where one-/two-call normal form fails.




                                                 2
ZeroStack RACC Draft 6                                                                     Aditya G


2.5   Transactional effects
Inject failures at every instruction boundary around sandbox, verification, lease issuance, and
compare-and-swap. Verify exact old or new roots and replayable event logs.


3     Workload studies
Task strata should include repository explanation, local refactor, cross-cutting refactor, API migra-
tion, Python-to-C++ port, dependency/build migration, security repair, performance work, and
greenfield game/application construction.
    Study mature repetitive repositories separately from novel greenfield projects. The empirical
novelty/fallback fraction is expected to differ substantially.


4     Q99 research
Measure:
• weighted logical L2 reuse;
• local and remote L3 residency;
• provider L1 hits;
• model-visible project-context elimination;
• complete-work reduction;
• capacity required for L3 Q99;
• eviction slack consumption;
• time/work to restore Q99 after changes;
• equality-boundary cutoff frequency;
• invalidation hazard by object class.
    Compare LRU/LFU with size-, causal-value-, hazard-, and recomputation-aware plans. Optimizer
cost and checker cost remain separate.


5     No-degradation and quality
Use same-model/same-harness pairing. Evaluate:
• protected regressions;
• strict rescues;
• factual evidence support;
• build/test/security outcomes;
• reasoning allowance and native-tool availability;
• subjective dimensions through blinded human review;
• fallback reserve and deadline success;
• long-horizon successor effects.
   No universal semantic claim is made outside the protected verifier/human scope.


6     Operational research
Measure idle and active CPU, memory, index update latency, storage growth, compaction, corruption
recovery, and multi-project isolation. The default local mode targets near-invisible idle overhead.



                                                 3
ZeroStack RACC Draft 6                                                                      Aditya G


Heavier whole-repository analysis must be scheduled, cancelable, and attributed to the task/campaign
ledger.


7    Capability research
Measure verified capability capture rate, proof cost, applicability, invalidation hazard, maintenance,
strict rescues, and lifetime value. Capabilities begin in shadow mode. Negative transfer, stale
execution, and cross-project leakage are kill conditions.


8    Novelty and literature review
Expand backward/forward citation review across communication complexity, zero-error information,
incremental computation, build systems, content-addressed storage, proof-carrying code, runtime
assurance, program analysis, agent interfaces, prompt caching, and software-engineering agents.
Candidate originality is the exact composition, not ownership of established components. Historical
priority remains unresolved until independent review.


9    Kill criteria
A proposed path is rejected or re-scoped when:
• it reduces model-visible tokens by hiding a genuine semantic decision;
• causal incompleteness causes a protected stale hit;
• fallback cannot execute within the baseline budget/deadline;
• complete work exceeds baseline without a declared quality trade;
• Q99 relies on omitting misses, Unknown objects, maintenance, or invalidation;
• provider caching already captures the same saving with lower complete cost;
• capability capture/maintenance has nonpositive lifetime value;
• the persistent backend violates operational overhead constraints;
• the same-model treatment exhibits protected regressions without exact fallback.


10     Research stop rule
No new headline theorem enters the current release unless it changes a runtime authority decision,
tightens a certified lower bound, yields a finite checker, reduces a measured Frontier Closure burden,
explains a reproducible failure, or proves an impossibility that prevents wasted engineering.


11     Public repository gate
A public release contains papers, sources, claim status, correction ledger, implementation require-
ments, schemas, traces, raw ledgers, benchmark scripts, negative results, provider facts with dates,
manifests, hashes, and reproduction instructions. Papers remain labeled Draft until independent
mathematical and systems review is complete.




                                                  4
