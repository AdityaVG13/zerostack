# Recovery-Aware Context Compression

RACC is TokenZero's public compression model.

The goal is not to make every response as short as possible. The goal is to minimize total task cost while keeping exact recovery available when visible context is not enough.

## Components

| Component | Meaning |
| --- | --- |
| Visible capsule | The compact text returned to the agent immediately |
| Exact cached payload | Byte-for-byte local payload stored outside model-visible context |
| Recovery handles | Stable refs for raw payload, file ranges, anchors, symbols, search hits, or error blocks |
| Recovery-adjusted objective | Visible tokens plus tokens recovered later for task completion |
| Task-lossless savings | Recovery-adjusted savings counted only for non-failing, non-negative events or validation tasks that preserve required facts |
| RATC | Cost proxy: visible tokens plus recovery tokens plus configured retry and failure penalties |

## Contract

TokenZero may omit payload text from the visible capsule only when one of these is true:

- The omitted content is already represented by a protected anchor.
- The omitted content is recoverable through an exact local ref.
- The mode explicitly chooses lossy visible compression and reports that recovery may be needed.

Exact refs are not model-readable payloads. They are local handles. A response that only emits an exact ref has high visible savings, but honest evaluation must count any later `expand` output used by the agent.

### Omission enforcement (RACC backport)

Capsule emission validates this rule at runtime. Exact recovery evidence must be a visible `tz://` handle with a concrete byte, line, or symbol selector. Protected-anchor evidence must name a visible `[[anchor:...]]`. A capsule without either must set `mode: lossy`, provide non-empty `lossy_spans` whose entries declare `recovery_may_be_needed: true`, and name a stable `lossy_policy_id`. The visible text repeats the lossy declaration so that consumers which render only capsule text cannot silently discard the warning.

The backport intentionally treats the omission declaration as a correctness floor: for an impossibly small token budget, the complete declaration may exceed the budget rather than degrade to unclassified text such as `omitted`.

## Public Objective

TokenZero tracks:

- Visible savings: first response token reduction.
- Recovery-adjusted savings: first response tokens plus recovered tokens.
- Task-lossless savings: recovery-adjusted savings after exact recovery and task-success gates.
- RATC: visible tokens plus weighted recovery, retry, and failure penalties for release reports.
- Exact-ref savings: compact handle cost, reported separately from model-readable content.
- Task success: whether expected task facts are present after any recovery.
- Anchor recall: preservation of signatures, imports, symbols, paths, errors, literals, and other protected facts.
- Downstream cost: latency, repeated reads, cache hits, and recovery requests.

## Zero Loss By Recovery

TokenZero's public claim is zero loss by recovery for local runtime payloads: the exact original payload can be recovered from the local cache while the cache entry exists.

The cache is bounded (per-kind counts plus a byte ceiling), and under pressure the oldest entries are evicted first. Eviction is FIFO by first insertion: the eviction position is set when a ref is first stored, and neither re-putting nor reading a ref refreshes it. A re-put of an existing ref appends a duplicate order entry that collapses back to the ref's first occurrence at compaction and on concurrent-session merge (first occurrence wins), so an interleaved re-put from another process cannot displace younger refs. An evicted ref reports `dangling-ref` on expand — never wrong bytes — and eviction cannot break a surviving ref: every ref kind stores its payload inline, so dropping one entry never dangles another.

That is different from claiming every visible capsule is semantically complete. Visible capsules are measured by task success and anchor recall. Exact refs are measured by roundtrip recovery. Exact mode is the deliberate exception on the visible side: it hides the payload behind the ref by contract, trading visible anchors for ref-only recovery.

## Promotion Rule

## Memory verbs (actions v2)

TokenZero owns the token-side working-set interface. The hub owns policy.
These six verbs are the named surface a trained policy can drive later
without changing the deterministic substrate (`WorkingSet`).

| Verb | Substrate target | Meaning |
| --- | --- | --- |
| `store` | `working_set.admit` | Persist bytes and admit a span |
| `commit_session` | `recovery_store.persist` | Flush session recovery records |
| `update_capsule` | `working_set.rewrite_render` | Replace a resident capsule |
| `forget_visible` | `working_set.evict` | Drop visible text, keep the exact ref |
| `promote_anchor` | `working_set.touch` | Mark a span hot |
| `link_refs` | `working_set.evicted_refs` | Record that one ref recovers another |

Types: `tokenzero_recovery::memory_verbs`. `describe_memory_verb` is
describe-only (`applied: false`). Do not put policy in this crate.

## Promotion Rule

A compression profile is not promoted by visible savings alone.

It needs:

- Exact refs with no dangling handles.
- Recovery-adjusted savings above the baseline.
- Task-lossless savings that does not regress behind a visible-only win.
- No protected-anchor regression for safety-critical modes.
- Task success on the release validation trace set.
- Clear artifact paths outside model-visible context.
