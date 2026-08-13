---
title: "RACC: Exact Causal Communication and the Zero-Degradation Pareto Frontier for Tool-Using Language-Model Agents"
author: "GPT-5.6 Pro — formal research draft"
date: "28 July 2026"
geometry: margin=1in
fontsize: 10pt
header-includes:
  - |
    ```{=latex}
    \usepackage{microtype}
    \usepackage{booktabs}
    \usepackage{longtable}
    \usepackage{mathtools}
    \usepackage{amssymb}
    \usepackage{mathrsfs}
    \usepackage{enumitem}
    \usepackage{xcolor}
    \definecolor{deepblue}{RGB}{20,55,100}
    ```
---

# Abstract

Long-horizon language-model agents repeatedly transmit files, tool outputs, logs, plans, and prior dialogue through the model boundary. A byte-exact external archive makes those observations recoverable, but archival losslessness alone does not ensure that the agent receives the evidence required for its next decision. This paper defines **recovery-aware context compression (RACC)** as an exact interactive-communication problem. For a fixed agent, environment, action interface, tokenizer, backend query algebra, and resource gauge, we define the **exact causal agency communication complexity**: the least language-model-visible communication among admissible protocols that preserve source bytes, the reference agent's action law, and task utility.

We prove five classes of results. First, exact action sufficiency plus successor closure gives equality of complete trajectory laws; a sound pre-action gate with raw fallback therefore gives universal zero degradation, although not universal high compression. Second, finite interactive systems possess a canonical coarsest exact action quotient, yielding sharp cardinality and entropy lower bounds; unrestricted exact-sufficiency verification is undecidable, and a universal fixed-percentage theorem is impossible. Third, independent sparse demands admit an exact recovery rate, while a general exposure identity decomposes production savings into fixed overhead, causal evidence coverage, and replay multiplicity. Fourth, bounded or sublinear exact working views force cumulative retained-token fractions to vanish; target savings such as 97%, 99%, and 99.9% are exact level sets of this task-indexed phase surface. Fifth, nested evidence expansion is reducible to online bidding: geometric expansion is strictly 4-competitive in deterministic cumulative active-context cost, and no deterministic black-box expansion policy can improve that constant. We combine these facts in a master certified-frontier theorem and give a constructive dependency-closure class in which exact 97--99.9% savings follow from explicit graph-size inequalities.

The remaining production claim is isolated as the **Recovery-Aware Derivative Conjecture (RADC-D)**: broad verifiable coding-agent workloads admit sublinear exact causal certificates and a fixed auditable query hierarchy approximates those certificates online. The implication from that conjecture to asymptotically vanishing retained-token fraction is proved here; the prevalence assumptions are not presented as proved facts. The resulting Rust contract stores all bytes immutably, exposes typed exact queries, emits proof-carrying evidence and accounting receipts, and falls back before irreversible action whenever exact sufficiency cannot be certified.

# 1. Claim ledger

This paper deliberately separates mathematical theorems from workload conjectures.

| ID | Claim | Status |
|---|---|---|
| T1 | Exact action-sufficient simulation preserves the full trajectory law | **Proved** |
| T2 | Sound pre-action certification plus raw fallback gives universal zero degradation | **Proved** |
| T3 | A finite controlled agent has a canonical minimal exact action quotient | **Proved + exact finite checks** |
| T4 | Quotient cardinality and entropy lower-bound exact communication | **Proved** |
| T5 | No universal positive compression ratio is possible over arbitrary tasks/interfaces | **Proved** |
| T6 | Exact future-action equivalence is undecidable for unrestricted computable agents | **Proved** |
| T7 | Independent sparse demands have an exact recovery-aware zero-error rate | **Proved + exact enumeration** |
| T8 | Production input accounting obeys an exact exposure/replay identity | **Proved + exact checks** |
| T9 | Bounded/sublinear exact views imply savings tending to one | **Proved + exact checks** |
| T10 | Geometric evidence expansion is deterministic 4-competitive, sharply | **Proved + independent exact checks** |
| T11 | Minimum monotone evidence-certificate selection is NP-hard | **Proved** |
| T12 | Dependency-local task classes admit explicit exact one-expansion certificates | **Proved + exact graph checks** |
| T12b | Exact streaming and causal-cut operators compress all-relevant inputs to exact sufficient interfaces | **Proved + exact checks** |
| T13 | Sandboxed verification plus raw rollback is task-success non-regressive | **Proved + exact checks** |
| T14 | Master certified zero-degradation phase theorem | **Proved conditional on stated certificates/gauges** |
| RADC-D | Broad real coding workloads have sublinear certifiable causal width | **Conjecture; falsifiable** |

The universal statement proved is **universal exactness through fallback**. The high-compression statement is a **task-indexed phase theorem**, not a universal constant.

# 2. Formal setting and anti-cheating lock

## 2.1 Agent, environment, and raw execution

Fix a finite or countable action space \(\mathcal A\), observation space \(\mathcal O\), task set \(\Omega\), and finite horizon \(T(\omega)\). For task \(\omega\), the raw history before decision \(t\) is

\[
H_t=(g_\omega,a_1,o_1,\ldots,a_{t-1},o_{t-1}),
\]

where \(g_\omega\) is the task contract. A fixed reference agent is a stochastic kernel

\[
\pi_A(\cdot\mid \sigma(H_t)),
\]

where \(\sigma\) is the locked raw serializer. The environment has a transition kernel

\[
P_E(o_t\mid H_t,a_t,\omega).
\]

The tokenizer is a fixed map \(\tau:\Sigma^*\to V^*\), where \(V\) is its vocabulary. The cumulative raw input accountability is

\[
R_\tau(\omega)
:=
\sum_{t=1}^{T(\omega)}
|\tau(\sigma(H_t))|.
\tag{2.1}
\]

Provider cache discounts may be represented by nonnegative per-token weights, but no theorem below silently equates cached billing with physical prefill work. The cost gauge must state which quantity is being optimized.

## 2.2 Recovery-aware protocol

A RACC protocol \(\Pi\) maintains

\[
Y_t=(Z_t,\mathcal D_t),
\]

where \(Z_t\) is compact model-visible state and \(\mathcal D_t\) is an immutable exact archive. A finite typed query algebra \(\mathcal Q\) maps explicit queries and archived objects to exact evidence certificates. The model-visible rendering at a call is denoted \(M_t^\Pi\). Its cumulative input accountability is

\[
C_\tau(\Pi,\omega)
:=
\sum_{t,k}|\tau(M_{t,k}^\Pi)|,
\tag{2.2}
\]

where \(k\) ranges over every model call, including retries, recovery calls, verification prompts, and raw fallback. This prevents failed expansion attempts from disappearing from the ledger.

The complete resource vector is

\[
\mathbf J_\omega(\Pi)
=
(C_{\rm in},C_{\rm out},N_{\rm calls},D_{\rm byte},D_{\rm policy},D_{\rm task},L,C_{\rm back},S_{\rm store},E_{\rm energy}).
\tag{2.3}
\]

The zero-degradation Pareto face is obtained by fixing

\[
D_{\rm byte}=D_{\rm policy}=D_{\rm task}=0
\]

and Pareto-minimizing the remaining resources.

## 2.3 Byte-exact archive

Every stored byte string \(x\) receives a content identity. A span reference is

\[
r=(\operatorname{id},a,b,h),
\]

with offset \(a\), length \(b\), and digest \(h\). Exactness requires

\[
\operatorname{Read}(r)=x[a:a+b],
\qquad
\operatorname{Digest}(\operatorname{Read}(r))=h.
\tag{2.4}
\]

This gives \(D_{\rm byte}=0\). It does not imply decision sufficiency.

## 2.4 Admissible backend and no-cheating condition

Without a restriction, a "compression backend" could solve the task and send the model only the answer. That would measure delegation, not context compression. We therefore lock an admissible class \(\mathfrak R\) of backend transducers. Each backend operator must be:

1. specified before the task instance;
2. deterministic or explicitly randomized with its randomness logged;
3. provenance preserving;
4. invoked through a typed query in \(\mathcal Q\);
5. unable to call the reference model or an uncharged substitute model;
6. unable to emit an environment action except through the locked action interface; and
7. charged in \(C_{\rm back}\), latency, and any secondary-model ledger.

Canonical serialization prevents arbitrary metadata from becoming a covert answer channel. Selection power is part of the locked router class \(\mathfrak R\), so all optima below are indexed by \((A,E,\mathcal Q,\mathfrak R,\tau)\).

## 2.5 Three exactness notions

Archival distortion is zero when (2.4) holds. Policy distortion over a finite horizon is

\[
D_{\rm policy}(\Pi)
:=
 d_{\rm TV}\!
 \left(
 P_\Pi(A_{1:T},O_{1:T}),
 P_{\rm raw}(A_{1:T},O_{1:T})
 \right).
\tag{2.5}
\]

For a bounded task utility \(U\in[0,1]\), define exact task distortion and one-sided task degradation by

\[
D_{\rm task}(\Pi;U)
:=
\left|
\mathbb E_\Pi U-
\mathbb E_{\rm raw}U
\right|,
\qquad
D_{\rm task}^{-}(\Pi;U)
:=
\max\{0,\mathbb E_{\rm raw}U-\mathbb E_\Pi U\}.
\tag{2.6}
\]

Exact policy preservation implies \(D_{\rm task}=0\) for every bounded utility. A transactional system may instead prove the weaker but production-relevant no-regression property \(D_{\rm task}^{-}=0\), allowing improvement. Byte exactness alone implies neither.

## 2.6 Resource-gauged exact causal communication complexity

Let \(\Lambda\) impose upper bounds on latency, backend work, storage, calls, or energy. Define

\[
K^0_{A,E,\mathcal Q,\mathfrak R,\tau}(\omega;\Lambda)
:=
\inf_{\Pi}
C_\tau(\Pi,\omega),
\tag{2.7}
\]

where the infimum ranges over admissible protocols satisfying the resource gauge \(\Lambda\) and

\[
D_{\rm byte}=D_{\rm policy}=D_{\rm task}=0.
\]

The optimal exact saving is

\[
S^{0,*}_\omega(\Lambda)
:=
1-
\frac{K^0_{A,E,\mathcal Q,\mathfrak R,\tau}(\omega;\Lambda)}
{R_\tau(\omega)}.
\tag{2.8}
\]

A 97% result is the level-set condition \(K^0\le0.03R\); a 99.9% result is \(K^0\le0.001R\). Neither number is the entire Pareto frontier.

# 3. Exact simulation and universal safe fallback

## Definition 3.1 (action-sufficient rendering)

A rendering \(v_t\) is exactly action sufficient for raw history \(H_t\) when

\[
\pi_A(\cdot\mid v_t)=\pi_A(\cdot\mid\sigma(H_t)).
\tag{3.1}
\]

For deterministic decoding this is equality of the next action. For stochastic decoding it is equality of kernels, not equality under one sampled seed.

## Definition 3.2 (reference-action bisimulation)

A relation \(\mathscr R\) between raw histories \(H\) and recovery-aware states \(Y\) is a reference-action bisimulation when, whenever \(H\,\mathscr R\,Y\):

1. \(\operatorname{Render}(Y)\) is action sufficient for \(H\); and
2. for every action \(a\) and observation \(o\) having positive probability under the common policy and environment, the updated states satisfy
   \[
   H^+\,\mathscr R\,Y^+.
   \tag{3.2}
   \]

## Theorem 3.3 (exact action-law simulation)

If the initial raw and recovery-aware states are related by a reference-action bisimulation, then for every finite horizon,

\[
P_\Pi(A_{1:T},O_{1:T})
=
P_{\rm raw}(A_{1:T},O_{1:T}).
\tag{3.3}
\]

Hence

\[
D_{\rm policy}=0,
\qquad
D_{\rm task}(\Pi;U)=0
\]

for every bounded measurable utility \(U\).

### Proof

We prove equality of the joint finite-dimensional laws by induction. At \(t=1\), relatedness and (3.1) give identical conditional action kernels. Couple the executions to draw the same action. Conditional on the common history and action, both use the same environment kernel, so they can be coupled to draw the same observation. Successor closure gives related next states. Repeating this argument through \(T\) yields equality of the joint trajectory law. Equation (2.5) is therefore zero. Integrating any bounded measurable \(U\) against equal measures yields equal expected utility. \(\square\)

### Remark 3.4

If the protocol retains the exact raw history in \(\mathcal D_t\) and recomputes a certified view afresh at every step, one may replace explicit successor closure with the requirement that (3.1) hold at every reachable history. The induction is unchanged.

## Theorem 3.5 (sound pre-action fallback envelope)

Let \(G(H,v)\in\{\mathrm{accept},\mathrm{reject}\}\) be a sound gate:

\[
G(H,v)=\mathrm{accept}
\Longrightarrow
\pi_A(\cdot\mid v)=\pi_A(\cdot\mid\sigma(H)).
\tag{3.4}
\]

Before every irreversible model action, define the rendered view

\[
\widehat v(H)=
\begin{cases}
v,&G(H,v)=\mathrm{accept},\\
\sigma(H),&G(H,v)=\mathrm{reject}.
\end{cases}
\tag{3.5}
\]

Then the resulting protocol has \(D_{\rm policy}=D_{\rm task}=0\) for every task and every fixed reference agent. If the gate is incomplete, incompleteness can reduce compression but cannot create policy distortion.

### Proof

For every reachable history, either the gate accepts and soundness gives (3.1), or it rejects and the model receives the exact raw rendering, for which (3.1) is tautological. Apply Theorem 3.3. \(\square\)

### Corollary 3.6 (harness portability, agent-parametric guarantee)

One backend implementation may serve arbitrary harnesses and models through adapters. The exactness theorem remains parameterized by the model and its serializer: a view certified for one policy need not be sufficient for another. Universal compatibility does not imply a universal model-independent compact view.

## Definition 3.7 (RACC dominance receipt)

For a task and target retained fraction \(\varepsilon\), a machine-checkable dominance receipt contains:

- the raw and recovery-aware token ledgers \((R,C)\);
- immutable object and span digests;
- the sequence of accepted sufficiency certificates or raw-fallback receipts;
- verifier and environment receipts;
- backend, latency, and storage gauges; and
- the inequality \(C\le\varepsilon R\).

## Corollary 3.8 (ex-post certified exact phase)

A valid dominance receipt proves, for that execution,

\[
D_{\rm byte}=D_{\rm policy}=D_{\rm task}=0,
\qquad
S\ge1-\varepsilon.
\tag{3.6}
\]

This is the theorem-to-runtime bridge: the code emits premises that a small checker verifies.

# 4. Canonical exact states and unavoidable lower bounds

## 4.1 Finite controlled transducers

Consider a finite controlled agent state machine

\[
\mathcal M=(S,\mathcal A,\mathcal O,\pi,\delta),
\]

where \(\pi(\cdot\mid s)\) is the action kernel and

\[
\delta:S\times\mathcal A\times\mathcal O\to S
\]

is deterministic state update. Define \(s\simeq t\) as the greatest relation satisfying

\[
\pi(\cdot\mid s)=\pi(\cdot\mid t)
\tag{4.1}
\]

and

\[
\delta(s,a,o)\simeq\delta(t,a,o)
\quad
\text{for all admissible }(a,o).
\tag{4.2}
\]

## Theorem 4.1 (canonical minimal exact action quotient)

The relation \(\simeq\) is an equivalence relation and a controlled bisimulation. Its quotient \(S/{\simeq}\) is an exact abstraction. Moreover, if

\[
f:S\to Z
\]

is any exact deterministic abstraction whose fibers preserve the action kernel and whose update is well-defined on \(Z\), then

\[
f(s)=f(t)\Longrightarrow s\simeq t.
\tag{4.3}
\]

Therefore every exact abstraction refines \(S/{\simeq}\), and

\[
|Z|\ge |S/{\simeq}|.
\tag{4.4}
\]

The quotient is unique up to relabeling.

### Proof

Start with the partition of \(S\) by equality of \(\pi(\cdot\mid s)\). Iteratively refine a block whenever two of its states have successors in different current blocks for some \((a,o)\). Finiteness guarantees termination. The terminal partition satisfies (4.1)--(4.2), hence defines an exact quotient.

Now let \(f\) be exact. If \(f(s)=f(t)\), exact action preservation gives (4.1). Because update on \(Z\) is well-defined, for every \((a,o)\),

\[
f(\delta(s,a,o))=f(\delta(t,a,o)).
\]

Induction through the refinement sequence shows that \(s,t\) are never separated, so they lie in the same terminal block and satisfy \(s\simeq t\). Thus every fiber of \(f\) is contained in a quotient block, proving (4.4). Any two coarsest exact quotients refine one another and are therefore isomorphic. \(\square\)

The included exact checker enumerates all set partitions for several finite machines and verifies that every exact abstraction refines the computed quotient.

## Theorem 4.2 (exact token-cardinality converse)

Let

\[
N=|S/{\simeq}|.
\]

Suppose an exact state message uses at most \(k\) tokens over a vocabulary of size \(v\ge2\). Then

\[
N\le \sum_{j=0}^{k}v^j
=
\frac{v^{k+1}-1}{v-1},
\]

and hence

\[
\boxed{
 k\ge
 \left\lceil
 \log_v((v-1)N+1)
 \right\rceil-1.
}
\tag{4.5}
\]

For fixed-length messages the sharper bound is

\[
k\ge\lceil\log_vN\rceil.
\tag{4.6}
\]

If quotient classes occur with distribution \(P_C\) and messages form a uniquely decodable \(v\)-ary code, then

\[
\boxed{
\mathbb E L\ge\frac{H_2(C)}{\log_2v}.
}
\tag{4.7}
\]

### Proof

A string of at most \(k\) tokens has at most the displayed number of possible values. Exactness requires distinct messages for distinct quotient classes, giving (4.5). Equation (4.7) is the source-coding lower bound after converting \(v\)-ary symbols to bits. \(\square\)

## Theorem 4.3 (no universal high-compression percentage)

Fix any \(\varepsilon<1\). There exists a finite task family and a locked literal-output action interface for which the worst-case zero-degradation communication, and the uniform-distribution expected uniquely-decodable communication, satisfy

\[
K^0_{\rm worst}\ge R,
\qquad
\mathbb E K^0\ge R.
\tag{4.8}
\]

In particular, no theorem can guarantee 97%, 99%, or any other positive fixed saving over all tasks and all locked action interfaces.

### Proof

Let the task input be \(x\in V^R\) and require the model's literal semantic action to equal \(x\). The action interface does not include a delegated copy-by-reference operation. The full-context reference policy emits \(x\). Two different strings require different semantic actions, so the family contains \(|V|^R\) action-distinguishable classes. A fixed-length exact code therefore needs at least \(R\) tokens in the worst case. Under the uniform distribution, (4.7) gives expected length at least

\[
\frac{\log_2 |V|^R}{\log_2|V|}=R.
\]

Thus no positive uniform saving is possible on this family. \(\square\)

### Remark 4.4 (action interfaces matter)

If the action interface instead permits `Copy(object_id)` and the environment interprets it as exact reproduction, the same task may require only a short reference. That is a valid systems improvement, but it changes the action alphabet and delegates execution. RACC theory must therefore lock both observation and action interfaces.

## Theorem 4.5 (undecidability of unrestricted exact sufficiency)

There is no algorithm that, given an arbitrary total computable agent and two finite history prefixes, always decides whether they induce identical future action behavior under all finite continuations.

### Proof

Assume such a decider \(D\) exists. Given a Turing machine \(M\) and input \(w\), construct a total computable agent \(A_{M,w}\). A history begins with a branch bit \(b\in\{0,1\}\), followed by a continuation encoding a natural number \(n\). On branch \(b=0\), the agent outputs action 0. On branch \(b=1\), it simulates \(M(w)\) for exactly \(n\) steps and outputs 1 if the simulation halts within those steps, otherwise 0. Every invocation terminates because the simulation is bounded.

Let \(h_0\) and \(h_1\) be the two branch prefixes. They have identical future action behavior for every continuation \(n\) exactly when \(M(w)\) never halts. Running \(D(A_{M,w},h_0,h_1)\) would therefore decide non-halting, a contradiction. \(\square\)

### Consequence 4.6

For unrestricted black-box agents, a sound and complete action-sufficiency gate is unattainable. Production systems require one or more of:

- sound but incomplete certificates on restricted query/task classes;
- transactional speculation with an objective task verifier;
- additional evidence expansion; or
- raw fallback.

This limitation is structural, not a missing engineering trick.

# 5. Exact sparse demand, replay, and cumulative phase laws

## 5.1 Independent sparse-demand model

Let

\[
X=(X_1,\ldots,X_N)
\]

be mutually independent uniform binary blocks with

\[
X_i\in\{0,1\}^{B_i},
\qquad B_i\in\mathbb N.
\]

A demand sequence \(S_1,\ldots,S_m\in[N]\) is independent of \(X\). Let

\[
U_m:=\{S_1,\ldots,S_m\}
\]

be the distinct demanded set. A pre-demand no-recovery message \(M\) must permit zero-error reconstruction of \(X_i\) for every possible demand \(i\). A recovery-aware archive may send an object when it is first demanded and retain it at the recipient.

## Theorem 5.1 (exact zero-error sparse-demand rate)

The least expected pre-demand no-recovery information is

\[
\boxed{
R_{\rm NR}^{0}=\sum_{i=1}^{N}B_i.
}
\tag{5.1}
\]

The least expected recovery-aware payload is

\[
\boxed{
R_{\rm RA}^{0}
=
\sum_{i=1}^{N}B_i\Pr(i\in U_m).
}
\tag{5.2}
\]

If demands are iid with probabilities \(\theta_i\), then

\[
\boxed{
R_{\rm RA}^{0}
=
\sum_{i=1}^{N}B_i
\left[1-(1-\theta_i)^m\right].
}
\tag{5.3}
\]

### Proof

For no recovery, each \(X_i\) is a deterministic function of \((M,i)\). Running all decoders on the same \(M\) reconstructs the complete vector. Since the vector is uniform over \(2^{\sum_iB_i}\) possibilities, any zero-error binary code needs at least \(\sum_iB_i\) bits in the worst case and in uniform expected prefix length. Sending \(X\) attains equality.

For recovery awareness, condition on a realized demand set \(U\). The demanded vector is uniform over \(2^{\sum_{i\in U}B_i}\) possibilities. Zero-error delivery therefore requires \(\sum_{i\in U}B_i\) bits, while sending each newly demanded block once attains it. Taking expectation and using indicators gives

\[
\mathbb E\sum_{i\in U_m}B_i
=
\sum_iB_i\Pr(i\in U_m).
\]

Under iid demands, \(i\) is never requested with probability \((1-\theta_i)^m\), proving (5.3). \(\square\)

For nonuniform finite sources, the same entropy expressions are converse bounds and become asymptotically attainable under ordinary block source coding; the one-shot exact equality above is stated for uniform blocks to avoid hiding coding redundancy.

### Corollary 5.2 (uniform equal-block phase)

For \(B_i=B\) and \(\theta_i=1/N\),

\[
\frac{R_{\rm RA}^{0}}{R_{\rm NR}^{0}}
=
1-\left(1-\frac1N\right)^m.
\tag{5.4}
\]

The exact retained-fraction target \(\varepsilon\) holds iff

\[
1-\left(1-\frac1N\right)^m\le\varepsilon.
\tag{5.5}
\]

Equivalently,

\[
N\ge
\frac{1}{1-(1-\varepsilon)^{1/m}}.
\tag{5.6}
\]

When \(m\ll N\), the retained fraction is \(m/N+O(m^2/N^2)\).

### Scope note 5.3

Theorem 5.1 assumes a persistent recipient. A stateless model API may need the same object in multiple calls. Production accounting must therefore count every exposure, not merely first recovery.

## 5.2 Exact production exposure identity

Partition model-visible evidence into canonically framed objects with token charges \(b_i\). Let \(r_i\) be the number of raw calls exposing object \(i\) and \(d_i\) the number of RACC calls exposing it. Let \(H_{\rm raw}\) and \(H_{\rm TZ}\) contain all other charged input.

For an actual tokenizer, the ledger should tokenize each final rendered message directly. The additive object form below is exact when framing resets tokenization at object boundaries; otherwise all boundary interactions are placed in \(H\).

## Theorem 5.4 (exposure/replay identity)

\[
\boxed{
C_{\rm raw}
=
H_{\rm raw}+\sum_i r_i b_i,
\qquad
C_{\rm TZ}
=
H_{\rm TZ}+\sum_i d_i b_i.
}
\tag{5.7}
\]

Consequently the exact saving is

\[
\boxed{
S
=
1-
\frac{H_{\rm TZ}+\sum_i d_i b_i}
{H_{\rm raw}+\sum_i r_i b_i}.
}
\tag{5.8}
\]

When \(H_{\rm raw}=0\), define

\[
B:=\sum_i b_i,
\qquad
\delta:=\frac{\sum_i d_i b_i}{B},
\qquad
\mu:=\frac{\sum_i r_i b_i}{B},
\qquad
h:=\frac{H_{\rm TZ}}{C_{\rm raw}}.
\]

Then

\[
\boxed{
S=1-h-\frac{\delta}{\mu}.
}
\tag{5.9}
\]

If each needed object is exposed exactly once, \(d_i=\mathbf1\{i\in U\}\), then \(\delta\) is the weighted causal-coverage fraction \(\phi\), recovering

\[
S=1-h-\frac{\phi}{\mu}.
\tag{5.10}
\]

### Proof

Equations (5.7) are definitions of complete token accountability under the locked framing. Substitution gives (5.8). For \(H_{\rm raw}=0\), divide numerator and denominator by \(B\mu=C_{\rm raw}\) to obtain (5.9). \(\square\)

### Exact target surfaces

The 97% phase is

\[
h+\frac{\delta}{\mu}\le0.03,
\tag{5.11}
\]

the 98% phase is

\[
h+\frac{\delta}{\mu}\le0.02,
\tag{5.12}
\]

and the 99.9% phase is

\[
h+\frac{\delta}{\mu}\le0.001.
\tag{5.13}
\]

Thus high savings can come from sparse evidence exposure, replay elimination, or both. Re-expansion thrashing increases \(\delta\) and is visible rather than hidden.

## 5.3 Long-horizon cumulative law

## Theorem 5.5 (bounded exact working-view phase)

Suppose the raw input length at call \(t\) obeys

\[
R_t\ge R_0+g(t-1),
\qquad g>0,
\tag{5.14}
\]

while an exact RACC view has length at most \(B\) at each call. Then through \(T\) calls,

\[
C_{\rm raw}(T)
\ge
TR_0+\frac{gT(T-1)}2,
\tag{5.15}
\]

\[
C_{\rm TZ}(T)\le BT,
\tag{5.16}
\]

and

\[
\boxed{
S_T
\ge
1-
\frac{B}{R_0+g(T-1)/2}.
}
\tag{5.17}
\]

For target retained fraction \(\varepsilon\), it is sufficient that

\[
\boxed{
T
\ge
1+
\frac{2(B/\varepsilon-R_0)}{g},
}
\tag{5.18}
\]

with the right side rounded upward and replaced by 1 when negative.

### Proof

Sum (5.14) over \(t=1,\ldots,T\) and compare with \(BT\). Division yields (5.17), and solving its residual fraction for \(T\) gives (5.18). \(\square\)

This theorem explains how 99.9% can emerge from replay elimination: raw cumulative communication is quadratic in horizon when a linearly growing transcript is resent, whereas a bounded exact view is linear.

## Theorem 5.6 (polynomial derivative law)

Suppose for constants \(a,b>0\),

\[
R_t\ge at^\alpha,
\qquad
K_t\le bt^\beta,
\qquad
-1<\beta<\alpha.
\tag{5.19}
\]

Then

\[
\frac{C_{\rm TZ}(T)}{C_{\rm raw}(T)}
\le
\frac ba
\frac{\alpha+1}{\beta+1}
T^{\beta-\alpha}(1+o(1)),
\tag{5.20}
\]

so

\[
\boxed{S_T\to1.}
\tag{5.21}
\]

### Proof

The standard power-sum asymptotic gives

\[
\sum_{t=1}^{T}t^\gamma
=
\frac{T^{\gamma+1}}{\gamma+1}(1+o(1))
\]

for \(\gamma>-1\). Apply it to numerator and denominator and divide. Since \(\beta-\alpha<0\), the ratio tends to zero. \(\square\)

## Corollary 5.7 (elasticity criterion)

For a differentiable exact communication envelope \(K(R)>0\), let

\[
S(R)=1-\frac{K(R)}R.
\]

Then

\[
\boxed{
S'(R)=\frac{K(R)-RK'(R)}{R^2},
}
\tag{5.22}
\]

and therefore

\[
S'(R)>0
\iff
\frac{d\log K}{d\log R}<1.
\tag{5.23}
\]

Exact savings improve with scale precisely when exact causal communication has elasticity below one.

# 6. Online evidence discovery and its sharp cost

An offline theorem may assert that a sufficient view of size \(K\) exists. A runtime still has to discover enough evidence without knowing \(K\). The simplest exact abstraction is a nested hierarchy

\[
V(b_0)\subseteq V(b_1)\subseteq\cdots,
\]

with monotone sufficiency: once \(V(b)\) is sufficient, every larger canonical view is sufficient. Let \(K\ge b_0\) be the unknown least sufficient budget.

## Theorem 6.1 (deterministic geometric expansion)

Bid budgets

\[
b_j=2^jb_0,
\qquad j=0,1,2,\ldots,
\]

until \(b_J\ge K\). If each trial incurs its complete active-context budget, then

\[
\boxed{
\sum_{j=0}^{J}b_j<4K.
}
\tag{6.1}
\]

If each trial also costs at most \(q\) control tokens, then

\[
C_{\rm search}
<
4K+q\left(1+\left\lceil\log_2\frac{K}{b_0}\right\rceil\right).
\tag{6.2}
\]

### Proof

Minimality of \(J\) gives \(2^{J-1}b_0<K\le2^Jb_0\). Therefore

\[
\sum_{j=0}^{J}b_j
=(2^{J+1}-1)b_0
<2^{J+1}b_0
<4K.
\]

The trial count is at most the displayed logarithmic term. \(\square\)

If a stateful transport charges only fresh incremental bytes rather than full active-context processing, the final exposed evidence is below \(2K\). The factor 4 is the conservative cumulative active-context bound.

## Theorem 6.2 (the deterministic factor 4 is sharp)

No deterministic black-box bidding strategy has competitive ratio smaller than 4 for every unknown threshold \(K\).

### Proof

Let an arbitrary increasing bid sequence be

\[
0<x_1<x_2<\cdots,
\]

and let

\[
s_n:=\sum_{i=1}^{n}x_i.
\]

Assume it is \(c\)-competitive for some \(c<4\). A threshold tending to \(x_{n-1}\) from above forces the algorithm to pay \(s_n\), hence

\[
s_n\le c x_{n-1}.
\tag{6.3}
\]

Define

\[
q_n:=\frac{s_n}{x_n},
\qquad
r_n:=\frac{x_n}{x_{n-1}}.
\]

Dividing (6.3) by \(x_{n-1}\) gives

\[
r_n+q_{n-1}\le c.
\tag{6.4}
\]

Also

\[
q_n
=1+\frac{s_{n-1}}{x_n}
=1+\frac{q_{n-1}}{r_n}
\ge
1+\frac{q_{n-1}}{c-q_{n-1}}
=
\frac{c}{c-q_{n-1}}.
\tag{6.5}
\]

For \(0<q<c\),

\[
\frac{c}{c-q}-q
=
\frac{q^2-cq+c}{c-q}.
\]

The numerator has discriminant \(c(c-4)<0\), hence is strictly positive. Thus \(q_n>q_{n-1}\). Competitiveness at threshold \(x_n\) also gives \(q_n\le c\), so \((q_n)\) has a finite limit \(q\le c\). Taking limits in (6.5) gives

\[
q\ge\frac{c}{c-q}>q,
\]

a contradiction. Therefore \(c\ge4\). \(\square\)

The exact checker confirms ratios approaching 4 from below at thresholds \(2^{j-1}b_0+1\), and an independent C++ program verifies the bound through ten million integer thresholds.

## Remark 6.3 (randomization)

The classical randomized online-bidding algorithm chooses a uniform logarithmic phase and uses exponentially spaced bids. In the ideal unbounded geometric model its expected competitive ratio is \(e\): the winning-bid overshoot has mean \(e-1\), and the geometric prefix costs \(e/(e-1)\) times the winning bid. The optimality of \(e\) is a known online-bidding result. The deterministic theorem above is proved here because it gives a portable worst-case critical path.

## Theorem 6.4 (nonmonotone obstruction)

Without a nested family or another structural approximation guarantee, no constant competitive ratio against the least sufficient subset follows from black-box accept/reject feedback alone.

### Proof

Let the admissible evidence family contain \(n\) incomparable singleton views and one raw view of cost \(M\). An adversary designates the last singleton queried by the algorithm as the unique sufficient singleton, or designates none and makes only the raw view sufficient. By choosing \(M\) and \(n\) arbitrarily large, the ratio between the algorithm's accumulated cost and the hidden optimum is unbounded. \(\square\)

Thus the hierarchy is not cosmetic. It is the source of the online constant.

## Theorem 6.5 (minimum certificate selection is NP-hard)

Given a monotone sound certificate predicate and additive evidence costs, finding the least-cost accepted evidence subset is NP-hard.

### Proof

Reduce SET COVER. Given universe \(U\) and subsets \(S_1,\ldots,S_N\), create one evidence object for each \(S_i\), with unit cost. Define the certificate predicate

\[
G(I)=\mathrm{accept}
\iff
\bigcup_{i\in I}S_i=U.
\]

The predicate is monotone and decidable. A least-cost accepted certificate is exactly a minimum set cover. \(\square\)

This result separates three quantities:

1. the unconstrained exact certificate optimum;
2. the best certificate available in a computable hierarchy; and
3. the online cost of finding that hierarchical certificate.

# 7. A constructive exact class: dependency-local agents

The previous theorems are abstract. This section gives a nontrivial formal class with an explicit one-expansion certificate.

## Definition 7.1 (locked repository evidence graph)

Let

\[
G=(V,E_1,\ldots,E_p)
\]

be a finite typed graph whose nodes identify exact repository objects: symbols, AST nodes, files, test traces, schema objects, build targets, or byte spans. Every edge type has a deterministic complete extractor under a locked language/toolchain semantics. For a seed set \(S\), relation set \(\mathcal R\), and radius \(r\), let

\[
C_\omega:=\operatorname{Cl}_{\mathcal R,r}(S)
\tag{7.1}
\]

be the exact dependency closure.

## Definition 7.2 (dependency-local reference policy)

A reference policy is \((\mathcal R,r)\)-local on task \(\omega\) when its action kernel factors through

\[
(g_\omega,Z_t,X_{C_\omega});
\]

that is, there exists a kernel \(\widetilde\pi\) such that

\[
\pi_A(\cdot\mid H_t)
=
\widetilde\pi(\cdot\mid g_\omega,Z_t,X_{C_\omega})
\tag{7.2}
\]

for every reachable history. This is a formal property of the task-agent pair, not an assumption that every real model is automatically local.

## Theorem 7.3 (exact dependency-closure certificate)

Suppose:

1. the graph and all closure operators are complete under the locked semantics;
2. the seed set is derived by an admissible deterministic operator;
3. the reference policy satisfies (7.2); and
4. the closure bytes and compact state are canonically rendered.

Then one query returning \(C_\omega\) is action sufficient. The RACC protocol has zero byte, policy, and task distortion, and

\[
\boxed{
C_{\rm TZ}
\le
H_\omega+
\sum_{v\in C_\omega}b_v,
}
\tag{7.3}
\]

where \(H_\omega\) charges task, state, query, framing, and certificate overhead.

### Proof

Completeness returns every and only node in the locked closure together with exact bytes and provenance. Equation (7.2) says the full-history action kernel is measurable with respect to exactly the rendered variables. Hence the closure rendering satisfies (3.1). Apply Theorem 3.3. The cost bound is direct accounting. \(\square\)

## Corollary 7.4 (explicit graph-size phase)

Suppose \(|S|=k\), the maximum relevant out-degree is \(\Delta\), radius is \(r\), every evidence node costs at most \(b\), and the archive has \(N\) nodes each costing at least \(b_{\min}\). Then

\[
|C_\omega|
\le
\begin{cases}
k(r+1),&\Delta=1,\\[4pt]
k\dfrac{\Delta^{r+1}-1}{\Delta-1},&\Delta>1.
\end{cases}
\tag{7.4}
\]

If the raw baseline exposes every node once, then

\[
\boxed{
S
\ge
1-
\frac{H_\omega+b|C_\omega|}
{Nb_{\min}}.
}
\tag{7.5}
\]

Therefore a \((1-\varepsilon)\) exact saving is certified whenever

\[
\boxed{
N
\ge
\frac{H_\omega+b|C_\omega|}
{\varepsilon b_{\min}}.
}
\tag{7.6}
\]

### Proof

A breadth-first expansion contains at most \(k\Delta^j\) nodes at depth \(j\); sum the geometric series and substitute into (7.3). \(\square\)

For \(\varepsilon=0.001\), the archive must be at least one thousand times the token-equivalent certificate scale after overhead. This is a theorem, not a prediction that every repository task has bounded \((k,\Delta,r)\).

## Remark 7.5 (one expansion versus arbitrary semantics)

Byte awareness guarantees exact one-call recovery of a known span and exact one-call execution of a complete typed closure query. It does not guarantee that an arbitrary natural-language question over a million-line object has a known complete semantic closure. Theorem 4.5 excludes a universal complete resolver for unrestricted agents. Unknown semantics require iterative recovery, a richer locked operator, a verifier, or raw fallback.

## 7.6 Exact aggregation: all bytes may matter without crossing the LM boundary

Sparse demand is not the only route to high savings. A locked deterministic operator may scan every source byte while communicating only a small exact sufficient statistic.

## Theorem 7.6 (exact streaming-query compression)

Let the archive consist of objects \(X_1,\ldots,X_N\). A query operator \(q\in\mathcal Q\) is implemented by a total deterministic streaming transducer

\[
s_0=s_{\rm init}(q),
\qquad
s_i=F_q(s_{i-1},X_i),
\qquad
y_q=G_q(s_N).
\tag{7.7}
\]

Assume the operator emits a completeness receipt proving that every locked input object was processed, and the reference action kernel factors through its exact output:

\[
\pi_A(\cdot\mid H_t)
=
\widetilde\pi_A(\cdot\mid g_\omega,Z_t,y_q).
\tag{7.8}
\]

Then the backend may scan all \(X_i\), send only \(y_q\) and its receipt, and preserve the exact action and task laws. If the model-visible charge is

\[
C_{\rm TZ}\le H_q+|\tau(y_q)|
\tag{7.9}
\]

while the raw baseline charge is \(B_N\), then

\[
\boxed{
S\ge1-\frac{H_q+|\tau(y_q)|}{B_N}.
}
\tag{7.10}
\]

If \(H_q+|\tau(y_q)|=o(B_N)\), exact savings tend to one even though every input object is read by the backend.

### Proof

The transducer and receipt establish that \(y_q\) is the exact canonical result of applying the locked operator to the entire archive. Factorization (7.8) makes the rendering action sufficient, so Theorem 3.3 gives exact trajectory and task-law preservation. Equation (7.10) is direct accounting. \(\square\)

### Examples 7.7

For locked syntactic or numerical semantics, exact outputs may include:

- a line count using \(O(\log N)\) state bits;
- exact keyword existence using a finite pattern automaton;
- a complete list of symbol definitions or references;
- an integer sum using output width plus \(O(\log N)\) carry bits;
- a compiler diagnostic set;
- a complete test receipt; or
- a dependency closure.

These operators may consume linear backend work while using sublinear LM communication. Backend compute remains a separate Pareto coordinate.

## Theorem 7.8 (compositional causal-cut theorem)

Let a task computation be a finite directed acyclic graph. Each processed node \(v\) consumes its local exact bytes and incoming interface messages and deterministically emits exact outgoing interface messages. For a topological cut \(t\), let \(\partial_t\) be the set of interface messages crossing from processed to unprocessed nodes. Assume every future reference action depends on the processed subgraph only through \(Z_t\) and the messages in \(\partial_t\).

Then the rendering

\[
V_t=(g_\omega,Z_t,\{m_e:e\in\partial_t\})
\tag{7.11}
\]

is exactly action sufficient. If message \(e\) has token length \(\ell_e\), the active exact context is bounded by

\[
\boxed{
|V_t|\le H_t+\sum_{e\in\partial_t}\ell_e.
}
\tag{7.12}
\]

Consequently, with causal cut width

\[
W:=\max_t\sum_{e\in\partial_t}\ell_e,
\tag{7.13}
\]

one has a bounded exact view \(H_t+W\), independently of the total bytes already processed.

### Proof

Induct over a topological ordering. Initially the claim is immediate. When a node is processed, its exact local transition replaces its incoming interface messages by deterministic outgoing messages. By the premise, two processed subgraphs agreeing on the new frontier induce the same future action law. Therefore the frontier relation is a reference-action bisimulation, and Theorem 3.3 applies. The size bound is the sum of the canonical interface encodings. \(\square\)

Theorem 7.8 generalizes both bounded streaming state and modular software evidence. The true structural parameter is not total transcript length but the size of the exact causal interface crossing the current computation cut.

# 8. Task-exact speculative execution for verifiable workloads

Exact policy equality is stronger than many production tasks require and is undecidable in general. Coding, theorem proving, data migration, and build tasks often provide an objective acceptance predicate. A second exact route preserves task success rather than the literal action trajectory.

## Definition 8.1 (transactional verifier wrapper)

Let \(V_\omega(y)\in\{0,1\}\) be a sound verifier:

\[
V_\omega(y)=1
\Longrightarrow
y\text{ is a successful task result}.
\tag{8.1}
\]

A compressed attempt runs in an isolated clone with no irreversible side effect. If its result is accepted, it is committed. If not, all effects are discarded and the original raw agent runs from the unchanged initial state.

## Theorem 8.2 (task-success no-regret wrapper)

Let

- \(p\) be the probability that the compressed attempt returns a verifier-accepted result;
- \(p_{\rm raw}\) be the raw agent's success probability on restart;
- \(c\) be compressed-attempt LM communication; and
- \(r\) be raw-agent LM communication.

Assuming restart randomness is conditionally fresh, the wrapper's success probability is

\[
\boxed{
 p_{\rm wrap}=p+(1-p)p_{\rm raw}\ge p_{\rm raw}.
}
\tag{8.2}
\]

Its expected communication is

\[
\boxed{
 \mathbb E C_{\rm wrap}=c+(1-p)r,
}
\tag{8.3}
\]

and its one-sided task degradation is zero:

\[
D_{\rm task}^{-}=0.
\]

and its expected saving relative to raw is

\[
\boxed{
 \mathbb E S_{\rm wrap}=p-\frac cr.
}
\tag{8.4}
\]

Thus expected saving at least \(1-\varepsilon\) holds exactly when

\[
\boxed{
\frac cr+1-p\le\varepsilon.
}
\tag{8.5}
\]

### Proof

Accepted compressed outputs are successful by soundness. With probability \(1-p\), the wrapper invokes the raw agent, which succeeds with probability \(p_{\rm raw}\). This gives (8.2). The compressed cost is always paid and the raw cost only on rejection, giving (8.3). Divide by \(r\) and subtract from one to obtain (8.4)--(8.5). \(\square\)

### Scope note 8.3

Theorem 8.2 guarantees no regression in the verifier-defined task success rate, not equality of the raw action law. It requires transactional rollback. For physical actions that cannot be cloned or undone, use the pre-action certified branch or raw mode.

## Corollary 8.4 (speculation with geometric evidence expansion)

Suppose a compressed attempt increases a nested evidence budget by doubling until a sound verifier accepts, and the least accepting budget is \(K\). Then compressed active-context communication is below

\[
4K+q\left(1+\left\lceil\log_2(K/b_0)\right\rceil\right).
\tag{8.6}
\]

Substitute this value for \(c\) in (8.5). This gives an exact distributional 97--99.9% criterion that charges failed expansions and raw restarts.

# 9. Master certified frontier theorem

We now combine the exactness, cost, hierarchy, and phase results.

## Definition 9.1 (hierarchical exact certificate complexity)

For task \(\omega\), let \(K^0(\omega;\Lambda)\) be (2.7). Let \(K_H(\omega)\) be the least cumulative communication of a certificate in a fixed computable nested hierarchy \(H\) that is accepted by a sound gate. Assume the hierarchy has approximation parameters \((\chi,\eta)\):

\[
K_H(\omega)
\le
\chi K^0(\omega;\Lambda)+\eta(\omega),
\qquad \chi\ge1.
\tag{9.1}
\]

This condition is a property to prove for a task class or measure empirically; it is not automatic because Theorem 6.5 shows exact certificate selection can be NP-hard.

## Theorem 9.2 (master exact certified-frontier theorem)

Fix \((A,E,\mathcal Q,\mathfrak R,\tau,\Lambda)\). Suppose for task \(\omega\):

1. the archive is byte exact;
2. every committed compressed decision carries a sound action-sufficiency certificate, with raw fallback otherwise; any rejected model-visible trial is isolated and does not enter committed state;
3. the certificate hierarchy obeys (9.1); and
4. total framing, state, query, verification, and accounting overhead is \(G(\omega)\).

Then

\[
D_{\rm byte}=D_{\rm policy}=D_{\rm task}=0.
\tag{9.2}
\]

If the least accepted hierarchical view is constructed directly, then

\[
\boxed{
C_{\rm TZ}
\le
\chi K^0+\eta+G.
}
\tag{9.3}
\]

If it is discovered through model-visible deterministic geometric expansion, then

\[
\boxed{
C_{\rm TZ}
<
4\chi K^0+4\eta
+q\left(1+\left\lceil
\log_2\frac{\chi K^0+\eta}{b_0}
\right\rceil\right)
+G.
}
\tag{9.4}
\]

Consequently, in the direct certified branch, exact saving at least \(1-\varepsilon\) is guaranteed whenever

\[
\boxed{
\chi K^0+\eta+G
\le
\varepsilon R_\tau.
}
\tag{9.5}
\]

In the black-box geometric branch, it is guaranteed whenever the right-hand side of (9.4) is at most \(\varepsilon R_\tau\).

### Proof

Conditions 1--2 invoke Theorem 3.5, giving (9.2). Direct construction costs at most \(K_H+G\), and (9.1) gives (9.3). Geometric discovery costs strictly below \(4K_H\) plus trial overhead by Theorem 6.1; apply (9.1) to obtain (9.4). Divide either bound by \(R_\tau\) and use the definition of saving. \(\square\)

## Corollary 9.3 (sublinear exact causal phase)

Assume along a workload family indexed by raw accountability \(R\),

\[
K^0(R)\le cR^\beta,
\qquad 0\le\beta<1,
\tag{9.6}
\]

\[
\eta(R)+G(R)+q\log R=o(R),
\tag{9.7}
\]

and \(\chi=O(1)\). Then both certified branches satisfy

\[
\boxed{
\frac{C_{\rm TZ}(R)}{R}\to0,
\qquad
S(R)\to1.
}
\tag{9.8}
\]

For the geometric branch, a sufficient finite-\(R\) target condition is

\[
4\chi cR^\beta+4\eta(R)
+q\left(1+\left\lceil
\log_2\frac{\chi cR^\beta+\eta(R)}{b_0}
\right\rceil\right)
+G(R)
\le\varepsilon R.
\tag{9.9}
\]

### Proof

Divide (9.3) or (9.4) by \(R\). Every term tends to zero because \(R^{\beta-1}\to0\), the additive terms are \(o(R)\), and \(\log R/R\to0\). Equation (9.9) is direct substitution. \(\square\)

## Corollary 9.4 (explicit target labels)

Under the theorem premises:

- **97% exact phase:** use \(\varepsilon=0.03\);
- **98% exact phase:** use \(\varepsilon=0.02\);
- **99% exact phase:** use \(\varepsilon=0.01\);
- **99.9% exact phase:** use \(\varepsilon=0.001\).

The labels are determined by the measured/proved inequality, not selected by marketing convention.

## Theorem 9.5 (full Pareto dominance condition)

Let \(\Pi_0\) be the raw protocol and \(\Pi\) a certified RACC protocol. RACC strictly Pareto-dominates raw on a locked objective subset \(J\) iff

\[
J_j(\Pi)\le J_j(\Pi_0)
\quad\text{for every }j\in J,
\tag{9.10}
\]

with strict inequality for at least one coordinate. Token dominance alone does not imply latency or backend-compute dominance. In particular, if input-token savings are \(\Delta C\), backend cost dominance requires the monetized or resource-normalized inequality

\[
C_{\rm back}^{\Pi}-C_{\rm back}^{0}
<
\operatorname{Value}(\Delta C),
\tag{9.11}
\]

and latency dominance requires the recovery and verification latency to be below saved prefill/attention latency.

This is definitional but essential: 99.9% token reduction at catastrophic latency is not a complete Pareto break.

# 10. Recovery-Aware Derivative Conjecture (RADC-D)

The mathematical implications are now proved. The remaining claim about broad real workloads is the following three-part conjecture.

## Conjecture 10.1 (RADC-D: competitive exact causal realizability)

There exists a finite auditable query algebra \(\mathcal Q_{\rm code}\), an admissible policy-oblivious Rust runtime class \(\mathfrak R_{\rm TZ}\), and constants or slowly growing functions \(\chi,\eta\) such that, for a broad distribution \(\mathcal D_{\rm code}\) of verifiable tool-using software tasks:

### Structural causal-width premise

For some \(\beta<1\) and workload-dependent \(c\),

\[
K^0(\omega)
\le
cR_\tau(\omega)^\beta
\tag{10.1}
\]

with high probability as raw accountability grows.

### Certifiable hierarchy premise

The fixed hierarchy satisfies

\[
K_H(\omega)
\le
\chi K^0(\omega)+\eta(\omega),
\tag{10.2}
\]

with \(\chi=O(1)\) or at most slowly growing.

### Low-fallback premise

Either pre-action exact certificates exist with high coverage, or the transactional verifier branch has acceptance probability \(p\) satisfying

\[
\frac{c_{\rm attempt}}{R}+1-p=o(1).
\tag{10.3}
\]

Under these premises, Theorem 9.2 gives exact policy preservation on the pre-action-certified branch, while Theorem 8.2 gives zero one-sided task degradation on the transactional branch. Both yield task-dependent savings tending to one on their respective certified workload phases.

## What is proved and what is not

The implication

\[
\text{(10.1)--(10.3)}
\Longrightarrow
S\to1
\]

is proved. The assertion that broad real coding workloads satisfy (10.1)--(10.3) is not proved by pure mathematics because it is a statement about a workload distribution, model behavior, parsers, toolchains, and verifiers. It is experimentally falsifiable.

A universal version of (10.1) is false by Theorem 4.3. A universal complete certificate finder is impossible by Theorem 4.5. Exact minimization can be computationally hard by Theorem 6.5. These obstructions define the largest credible conjecture rather than defeating the program.

## Falsifiers

RADC-D fails for a proposed task class if any of the following persists with scale:

1. the exact causal certificate is linear in raw accountability;
2. the fixed hierarchy has unbounded approximation ratio;
3. re-expansion multiplicity cancels replay savings;
4. exact pre-action certificates almost never fire;
5. verifier rejection makes \(1-p\) dominate the target residual fraction;
6. backend work or latency exceeds the value of token savings; or
7. raw fallback cannot be made transactional before irreversible actions.

# 11. Rust realization: theorem as runtime contract

The theorem does not become one compression formula hidden inside a router. It becomes an invariant-carrying protocol whose output is a dominance receipt.

## 11.1 Core types

```rust
pub struct ObjectId([u8; 32]);
pub struct Digest([u8; 32]);

pub struct SpanRef {
    pub object_id: ObjectId,
    pub byte_start: u64,
    pub byte_len: u64,
    pub object_digest: Digest,
    pub span_digest: Digest,
}

pub enum Query {
    ReadSpan(SpanRef),
    ExactSearch { scope: ObjectId, pattern: Vec<u8> },
    Definition { symbol: SymbolId },
    References { symbol: SymbolId },
    AstClosure { seeds: Vec<NodeId>, relations: RelationMask, radius: u32 },
    CallPath { source: SymbolId, target: SymbolId },
    DataflowSlice { sink: NodeId },
    Diff { old: ObjectId, new: ObjectId },
    BuildReceipt { command: CommandId },
    TestTrace { test: TestId },
}

pub struct EvidenceCertificate {
    pub query: Query,
    pub spans: Vec<SpanRef>,
    pub payload: Vec<u8>,
    pub provenance: Provenance,
    pub completeness: CompletenessWitness,
    pub input_token_cost: u64,
    pub backend_work_units: u64,
}

pub enum DecisionGate {
    Certified(PolicySufficiencyWitness),
    TaskVerified(TaskAcceptanceReceipt),
    Expand(NextBudget),
    RawFallback,
}
```

## 11.2 Required modules

| Formal object | Runtime module | Critical invariant |
|---|---|---|
| \(\mathcal D_t\) | `archive` | immutable byte-exact recovery |
| stable reference | `object_id` / `span` | digest-bound identity and range |
| \(\mathcal Q\) | `query` | typed deterministic semantics |
| evidence certificate | `certificate` | exact payload and provenance |
| completeness proof | `verify` | sound acceptance for locked operator |
| compact state \(Z_t\) | `state` | append-only constraints and decisions |
| hierarchy | `router` | nested monotone expansion budgets |
| token objective | `ledger` | actual tokenizer, every call charged |
| task-safe speculation | `transaction` | clone, commit, rollback |
| universal safety | `fallback` | exact raw rendering before action |
| ex-post theorem | `receipt` | machine-checkable phase inequality |

## 11.3 Execution protocol

1. Intercept every observation before it is appended to the model prompt.
2. Store the original bytes and metadata immutably.
3. Build deterministic indexes and typed relations.
4. Maintain a compact, explicitly typed task-state ledger; never substitute an unverifiable free-form summary for a protected constraint.
5. Assemble the smallest canonical certificate known for the current request.
6. Verify byte provenance and operator completeness.
7. Use one of two exact branches:
   - accept a pre-action sufficiency certificate; or
   - run a transactional attempt and commit only an objectively verified result.
8. Expand geometrically when the evidence hierarchy is monotone and the gate asks for more.
9. Fall back to exact raw context before an irreversible action when certification fails.
10. Emit the complete resource and dominance receipt.

## 11.4 What the runtime can certify automatically

The runtime can directly certify:

- byte identity and range correctness;
- exact-search completeness over a locked byte domain;
- all references under a locked parser/index version;
- graph closure under declared edge relations;
- exact build/test command outcomes in an immutable environment;
- token counts under a locked tokenizer;
- exposure multiplicities, retries, and fallback cost; and
- the final \(C\le\varepsilon R\) inequality.

It cannot generally certify, for an arbitrary black-box model, that an arbitrary semantic subset would induce exactly the same next-action distribution. That is the undecidable boundary from Theorem 4.5. Such decisions require a restricted formal agent, a task verifier, or raw fallback.

## 11.5 Harness interface

```rust
pub trait RaccBackend {
    fn ingest(
        &mut self,
        bytes: &[u8],
        metadata: ObservationMetadata,
    ) -> Result<ObjectId, RaccError>;

    fn propose_view(
        &mut self,
        request: ViewRequest,
        budget: TokenBudget,
    ) -> Result<CertifiedView, RaccError>;

    fn expand(
        &self,
        query: Query,
    ) -> Result<EvidenceCertificate, RaccError>;

    fn verify(
        &self,
        certificate: &EvidenceCertificate,
    ) -> Result<VerifiedEvidence, VerificationError>;

    fn raw_fallback(
        &self,
        history: HistoryId,
    ) -> Result<RawView, RaccError>;

    fn finalize_receipt(
        &self,
        target: RetainedFraction,
    ) -> Result<DominanceReceipt, ReceiptError>;
}
```

Any harness may implement an adapter around this interface. The backend remains generic; exact certificate semantics are supplied by language/toolchain plugins.

# 12. Experimental program needed to attack RADC-D

The proof program and the empirical program must be run together without confusing their status.

## 12.1 Exact finite track

For finite agents and environments:

1. enumerate raw histories;
2. compute the canonical quotient by partition refinement;
3. compute \(K^0\) by dynamic programming or integer programming under the locked query algebra;
4. compute the best hierarchy certificate and its \(\chi\) ratio;
5. run exact Python and independent C++ checkers; and
6. compare measured online expansion with the 4-competitive bound.

This track can deliver genuine exhaustive certificates.

## 12.2 Production coding track

Evaluate raw and TokenZero modes using identical base models, temperatures, tool sets, and task seeds. Include:

- bug fixes;
- feature additions;
- cross-language ports;
- refactors;
- dependency upgrades;
- schema migrations;
- build failures;
- repository questions;
- replay-heavy multi-day sessions;
- adversarial all-relevant tasks; and
- alternating working sets designed to induce thrashing.

Report per task, not only averages:

\[
R,
\ C,
\ S,
\ D_{\rm byte},
\ D_{\rm policy}\text{ when measurable},
\ D_{\rm task},
\ p_{\rm accept},
\ \#\text{expansions},
\ \{d_i\},
\ L,
\ C_{\rm back},
\ S_{\rm store}.
\]

The first release gate should require:

- 100% byte recovery for resident objects;
- 100% correctness of certified typed query results;
- no statistically or transactionally demonstrated task regression;
- complete fallback and retry accounting; and
- a preregistered target phase such as \(C/R\le0.03\).

## 12.3 Phase-surface experiments

The critical regressions are

\[
\log K^0\quad\text{versus}\quad\log R,
\]

estimating causal-width exponent \(\beta\), and

\[
K_H/K^0,
\]

estimating hierarchy factor \(\chi\). Additional surfaces should vary:

- number of repository objects \(N\);
- relevant closure size;
- horizon and replay multiplicity;
- evidence revisitation rate;
- model family;
- task verifier strength; and
- query-algebra richness.

Evidence for RADC-D requires \(\beta<1\) with confidence intervals and bounded or slowly growing \(\chi\), not merely a large compression percentage on one trace.

# 13. Relation to adjacent theory and systems

The basic ingredients have precedents, so novelty must be stated narrowly.

- Indexed exact archives and bounded dereferencing have been analyzed in Memex(RL), including a regime where decision quality is preserved with bounded working context.
- LCM retains deterministic pointers to original context through a summary DAG, but exact retrievability of bytes is distinct from a proof that the agent always requests the decisive evidence.
- Self-GC treats context as lifecycle-managed recoverable objects and reports production token reductions, but its "no-impact" measurements are empirical rather than exact action-law certificates.
- ClawVM places fidelity and residency invariants in the harness, closely matching the claim that the harness is the enforcement point.
- Recent bounded-interaction Myhill--Nerode work establishes canonical minimal observer-dependent quotients in finite POMDP settings. The finite quotient theorem here is specialized to exact reference-policy simulation and is used as a communication lower bound, not claimed as the first quotient theorem.
- Semantic rate-distortion work on bounded agents also derives communication alphabets from capacity-dependent quotients.
- Online bidding supplies the optimal deterministic factor 4 and randomized factor \(e\) for unknown thresholds; this paper applies that exact structure to monotone evidence expansion.
- SWE-Pruner and other empirical context systems demonstrate meaningful but generally smaller coding-agent reductions with near-matched success. Parallel compaction work explicitly observes that 90--99% natural-language summarization is inherently lossy and unstable.

The combined contribution pursued here is the joint object:

\[
\begin{aligned}
&\text{three exactness axes}
+\text{ resource-gauged causal communication complexity}\\
&+\text{ cumulative exposure/replay law}
+\text{ target phase inequalities}\\
&+\text{ quotient converse and undecidability boundary}
+\text{ sharp online discovery}\\
&+\text{ proof-carrying typed query algebra}
+\text{ universal pre-action fallback}.
\end{aligned}
\]

A targeted literature review did not locate this complete combination as one theorem-and-runtime contract. That is not an exhaustive patent or publication novelty proof.

# 14. Non-claims

1. No universal 97%, 99%, or 99.9% compression theorem is claimed.
2. Byte-exact storage is not called policy-exact retrieval.
3. Equal benchmark scores are not called equality of action kernels.
4. A backend model or hidden solver is not treated as free compression.
5. One-expansion semantic answering is claimed only for complete locked query operators.
6. The same compact view is not claimed sufficient for every possible model.
7. Token savings do not automatically imply latency, energy, or monetary Pareto dominance.
8. The conjectured prevalence of sublinear causal width in real coding workloads is not presented as proved.
9. Transactional task-level no-regret is not mislabeled as exact raw-policy simulation.
10. Tasks requiring all information are expected to fall back and may obtain little or no compression.

# 15. Conclusion

The strongest true theorem is not "TokenZero always compresses by 97%." It is:

\[
\boxed{
\text{Universal zero-degradation safety by exact certification or raw fallback,}
}
\]

combined with the task-indexed exact phase law

\[
\boxed{
S^{0,*}_\omega
=
1-
\frac{K^0_{A,E,\mathcal Q,\mathfrak R,\tau}(\omega;\Lambda)}
{R_\tau(\omega)}.
}
\]

When exact causal communication is sublinear, the retained fraction vanishes. When a dependency closure is explicit, 97--99.9% follows from a finite graph inequality. When information is dense, the quotient converse blocks high compression. When sufficiency is not decidable, sound incomplete certificates and raw fallback preserve capability. When the sufficient evidence threshold is unknown but nested, geometric expansion discovers it within the sharp deterministic factor 4.

The remaining frontier is no longer vague: prove or falsify sublinear exact causal width and bounded hierarchy approximation on useful task classes. TokenZero's code should be built to emit the exact receipts needed to test those statements.

# Appendix A. Compact proof dependency map

| Result | Depends on |
|---|---|
| Exact trajectory equality | action equality + successor closure |
| Universal zero degradation | sound gate + exact raw fallback + trajectory theorem |
| Quotient lower bound | canonical controlled bisimulation quotient |
| Universal-percentage obstruction | quotient cardinality + literal action interface |
| Sparse-demand law | independent entropy + zero-error decoding |
| Replay law | complete token exposure accounting |
| Horizon/derivative law | arithmetic sums and sublinear growth |
| 4-competitive expansion | online bidding geometry |
| Dependency-local phase | complete graph closure + policy factorization |
| Streaming/cut phase | exact local transducers + policy factorization through interfaces |
| Speculative no-regret | sound verifier + rollback + raw restart |
| Master phase theorem | exactness theorem + hierarchy approximation + expansion bound |
| RADC-D implication | master theorem + sublinear premises |

# Appendix B. Exact certificate artifacts

The accompanying artifact contains:

- `RACC_EXACT_CHECKS.py`: exact rational checks for sparse demand, replay identities, geometric expansion, speculative fallback, horizon thresholds, quotient minimality, dependency closures, and master phase arithmetic;
- `RACC_INDEPENDENT_CHECK.cpp`: independent C++ checks of the geometric bound, occupancy formula, and horizon arithmetic;
- execution logs with 214 Python assertions and independent C++ passes;
- `RACC_CONTRACT.rs`: a compile-ready Rust semantic skeleton (the current artifact container has no Rust compiler, so it is not represented as compiler-attested); and
- a one-command reproducibility runner.

# References

1. M. Chrobak, C. Kenyon, J. Noga, and N. E. Young. *Incremental Medians via Online Bidding*. Algorithmica 50, 455--478 (2008); arXiv:cs/0504103.
2. Z. Wang et al. *Memex(RL): Scaling Long-Horizon LLM Agents via Indexed Experience Memory*. arXiv:2603.04257 (2026).
3. C. Ehrlich and T. Blackman. *LCM: Lossless Context Management*. arXiv:2605.04050 (2026).
4. *Self-GC: Self-Governing Context for Long-Horizon LLM Agents*. arXiv:2607.00692 (2026).
5. A. T. Nixon. *The Myhill--Nerode Theorem for Bounded Interaction: Canonical Abstractions via Agent-Bounded Indistinguishability*. arXiv:2603.21399 (2026).
6. A. T. Nixon. *Semantic Rate-Distortion for Bounded Multi-Agent Communication*. arXiv:2604.09521 (2026).
7. M. Rafique and L. Bindschaedler. *ClawVM: Harness-Managed Virtual Memory for Stateful Tool-Using LLM Agents*. EuroMLSys 2026; arXiv:2604.10352.
8. *SWE-Pruner: Self-Adaptive Context Pruning for Coding Agents*. arXiv:2601.16746 (2026).
9. M. Cim, B. Topcu, C. Das, and M. Kandemir. *Parallel Context Compaction for Long-Horizon LLM Agent Serving*. arXiv:2605.23296 (2026).
10. *Are We Ready for an Agent-Native Memory System?* arXiv:2606.24775 (2026).
