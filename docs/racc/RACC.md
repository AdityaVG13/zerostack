# Recovery-Aware Context Compression

RACC is ZeroKernel's output economics. TokenZero applies it at operation and response boundaries.

It minimizes total task cost while keeping exact recovery available when visible context is not enough. A large result is stored in a recoverable backing store. The model receives a compact capsule plus a handle. Later work expands only the needed lines, symbols, or raw bytes.

## Handles

Model-facing recovery uses `z.read` on a `z://blob/<digest>` handle. Engine-local schemes remain inside their stores.

| Ref | Producer | Typical content |
| --- | --- | --- |
| `z://blob/<digest>` | ZeroKernel / TokenZero | Bounded projections, large reads, exact recovery |
| `tz://` | TokenZero (engine-local) | Compacted logs, search output, shell output |
| `fz://` | FSZero (engine-local) | File reads, searches, plans, mutation receipts |
| `gz://` | GraphZero (engine-local) | Graph snapshots, impact paths, orientation results |

Handles are useful only when consumers preserve type and recovery path. Expand the smallest sufficient range. Surface an explicit error when a handle is unavailable or expired. Do not copy full payloads back into context, and do not write an outline or preview back as source bytes.

## Components

| Component | Meaning |
| --- | --- |
| Visible capsule | Compact text returned immediately |
| Exact cached payload | Byte-for-byte local payload stored outside model-visible context |
| Recovery handles | Stable refs for raw payload, file ranges, anchors, symbols, search hits, or error blocks |
| Recovery-adjusted objective | Visible tokens plus tokens recovered later for task completion |
| Task-lossless savings | Recovery-adjusted savings counted only for non-failing tasks that preserve required facts |
| RATC | Visible tokens plus recovery tokens plus configured retry and failure penalties |

## Contract

TokenZero may omit payload text from the visible capsule only when one of these is true:

- the omitted content is already represented by a protected anchor;
- the omitted content is recoverable through an exact local handle;
- the mode explicitly chooses lossy visible compression and reports that recovery may be needed.

Exact handles are identifiers, not model-readable payloads. A response that only emits a handle has high visible savings, but honest evaluation must count any later expansion.

Capsule emission validates this rule at runtime. Exact recovery evidence must be a visible handle with a concrete byte, line, or symbol selector (`z://blob/<digest>` at the ZeroKernel boundary; engine-local `tz://` inside TokenZero). Protected-anchor evidence must name a visible `[[anchor:...]]`. A capsule without either must set `mode: lossy`, provide non-empty `lossy_spans` whose entries declare `recovery_may_be_needed: true`, and name a stable `lossy_policy_id`. The visible text repeats the lossy declaration so consumers that render only capsule text cannot drop the warning.

For an impossibly small token budget, the complete declaration may exceed the budget rather than degrade to unclassified text such as `omitted`.

## Accounting

Tracked quantities:

- Visible savings: first-response token reduction
- Recovery-adjusted savings: first-response tokens plus recovered tokens
- Task-lossless savings: recovery-adjusted savings after exact recovery and task-success gates
- RATC: visible tokens plus weighted recovery, retry, and failure penalties
- Exact-handle savings: compact handle cost, reported separately from model-readable content
- Task success: whether expected facts are present after any recovery
- Anchor recall: preservation of signatures, imports, symbols, paths, errors, literals, and other protected facts
- Downstream cost: latency, repeated reads, cache hits, and recovery requests

## Zero loss by recovery

While a cache entry exists, expand returns the original payload, never a second summary.

The cache is bounded (per-kind counts plus a byte ceiling). Under pressure the oldest entries evict first. Eviction is FIFO by first insertion: the position is set when a handle is first stored, and neither re-putting nor reading refreshes it. A re-put of an existing handle appends a duplicate order entry that collapses back to the first occurrence at compaction and on concurrent-session merge (first occurrence wins). An evicted handle reports `dangling-ref` on expand; it never returns the wrong bytes. Every handle kind stores its payload inline, so dropping one entry never dangles another.

Visible capsules are measured by task success and anchor recall. Exact handles are measured by roundtrip recovery. Exact mode hides the payload behind the handle by contract, trading visible anchors for handle-only recovery.

## Promotion

A compression profile is not promoted by visible savings alone. It needs:

- exact handles with no dangling refs;
- recovery-adjusted savings above the baseline;
- task-lossless savings that does not regress behind a visible-only win;
- no protected-anchor regression for safety-critical modes;
- task success on the release validation trace set;
- artifact paths outside model-visible context.

## Memory verbs

TokenZero owns the token-side working-set interface. The hub owns policy. These six verbs are the named surface a trained policy can drive later without changing the deterministic substrate (`WorkingSet`).

| Verb | Substrate target | Meaning |
| --- | --- | --- |
| `store` | `working_set.admit` | Persist bytes and admit a span |
| `commit_session` | `recovery_store.persist` | Flush session recovery records |
| `update_capsule` | `working_set.rewrite_render` | Replace a resident capsule |
| `forget_visible` | `working_set.evict` | Drop visible text, keep the exact handle |
| `promote_anchor` | `working_set.touch` | Mark a span hot |
| `link_refs` | `working_set.evicted_refs` | Record that one handle recovers another |

Types: `tokenzero_recovery::memory_verbs`. `describe_memory_verb` is describe-only (`applied: false`). Do not put policy in that crate.
