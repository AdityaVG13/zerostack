# zerostack-codemode-contract v1.0

Status: normative. This document supersedes prior prose specs for conformance purposes. Durable step-log/replay is explicitly deferred and is not part of v1.0.

## 1. Scope

ZeroStack CodeMode v1.0 defines the machine-checkable contract shared by FSZero (`fz`), TokenZero (`tz`), and GraphZero (`gz`). A conforming substrate exposes a launch-mode-selected CodeMode MCP surface, accepts recipe, JSON, and JavaScript plans, returns ref-first execution artifacts, emits the telemetry vocabulary below, and enforces every limit it declares.

Normative keywords use RFC 2119 meanings.

## 2. Namespaces and mutation capability

Each substrate has one namespace:

| Substrate | `ns` | Required `mutation` value |
|---|---:|---|
| FSZero | `fz` | `allowed` |
| TokenZero | `tz` | `denied` |
| GraphZero | `gz` | `readonly` |

`mutation` semantics:

- `allowed`: mutating CodeMode bindings MAY be present. The substrate MUST wrap a multi-step execution in a cross-step transaction journal. If any later step fails, previously applied mutation in the same execution MUST roll back before the final `X0` response is returned. FSZero's TransactionJournal model is the reference behavior.
- `denied`: mutating sandbox bindings MUST be absent. Structured or recipe plan operations that request mutation MUST be rejected with `Error.kind = "policy"`. No cross-step transaction journal is required while mutation is denied.
- `readonly`: same rejection behavior as `denied`, and the substrate additionally declares that no mutating substrate operations exist in CodeMode. Read-only metadata/ref writes needed to store execution records are not domain mutation.

## 3. Capability manifest

Every substrate MUST serve `{ns}.codemode.describe("capabilities")` and `{ns}_codemode_describe` with `name = "capabilities"` returning a JSON object matching `schemas/capability-manifest.schema.json`:

```json
{
  "contract_version": "1.0",
  "ns": "fz",
  "mutation": "allowed",
  "plan_forms": ["recipe", "json", "js"],
  "limits": {
    "max_logical_ops": 1000,
    "max_physical_ops": 256,
    "max_wall_ms": 250,
    "hard_max_wall_ms": 5000,
    "max_microtasks": 4096,
    "max_memory_bytes": 33554432,
    "max_output_bytes": 65536,
    "max_result_ref_bytes": 10485760,
    "max_refs_emitted": 256,
    "max_parallel_width": 16,
    "max_code_bytes": 65536
  }
}
```

Required fields:

- `contract_version`: exactly `"1.0"`.
- `ns`: one of `"fz"`, `"tz"`, `"gz"`.
- `mutation`: one of `"allowed"`, `"denied"`, `"readonly"`.
- `plan_forms`: MUST contain `"recipe"`, `"json"`, and `"js"`.
- `limits`: object containing only enforced limits. Echoed means enforced. A substrate MAY omit a limit it cannot enforce, but MUST NOT echo a dead limit.

Normative default limits:

| Limit | Default | Rule |
|---|---:|---|
| `max_logical_ops` | `1000` | Host-call or plan-op logical cap. |
| `max_physical_ops` | `256` | Native/kernel operation cap. |
| `max_wall_ms` | `250` | Default execution wall-clock cap. |
| `hard_max_wall_ms` | `5000` | Absolute dev/benchmark wall-clock cap. |
| `max_microtasks` | `4096` | Promise/microtask drain cap. |
| `max_memory_bytes` | `33554432` | JS heap or nearest lower/equal engine cap. |
| `max_output_bytes` | `65536` | Maximum visible response bytes before ref redirection. |
| `max_result_ref_bytes` | `10485760` | Maximum stored execution-result payload bytes. |
| `max_refs_emitted` | `256` | Maximum refs emitted in one response. |
| `max_parallel_width` | `16` | Maximum concurrent/batched logical width. |
| `max_code_bytes` | `65536` | Maximum accepted JavaScript source bytes. |

## 4. Launch modes and MCP exposure

Each substrate server binary MUST accept exactly one launch mode flag:

- `--mode=mcp`: serves per-operation MCP tools only. It MUST list zero CodeMode tools.
- `--mode=codemode`: serves exactly three CodeMode tools and no per-operation tools.

The CodeMode tools are:

1. `{ns}_execute_code`
2. `{ns}_codemode_search`
3. `{ns}_codemode_describe`

The two tool sets MUST NOT coexist in one process.

### `{ns}_execute_code`

Input schema:

```json
{
  "type": "object",
  "required": ["plan"],
  "additionalProperties": false,
  "properties": {
    "plan": { "type": "string", "maxLength": 65536 },
    "form": { "type": "string", "enum": ["recipe", "json", "js", "auto"] },
    "limits": { "type": "object", "additionalProperties": { "type": "integer", "minimum": 0 } }
  }
}
```

Output on success:

```json
{
  "ack": "C",
  "execution_id": "cm://exec/1782920000000-012345abcdef",
  "refs": {
    "code": "fz://codemode/execution/1782920000000-012345abcdef/code",
    "steps": "fz://codemode/execution/1782920000000-012345abcdef/steps",
    "telemetry": "fz://codemode/execution/1782920000000-012345abcdef/telemetry",
    "result": "fz://codemode/execution/1782920000000-012345abcdef/result"
  },
  "telemetry": {
    "kind": "codemode.execute",
    "status": "ok",
    "logical_ops": 1,
    "physical_ops": 1,
    "batched_ops": 0,
    "internal_actions": 1,
    "cache_hits": 0,
    "cache_misses": 0,
    "store_writes": 4,
    "wall_ms": 3,
    "bytes_materialized": 128
  }
}
```

Output on failure:

```json
{
  "ack": "X0",
  "execution_id": "cm://exec/1782920000000-012345abcdef",
  "error_ref": "fz://codemode/execution/1782920000000-012345abcdef/error",
  "error": { "kind": "validation", "message": "invalid JSON plan", "retryable": false }
}
```

### `{ns}_codemode_search`

Input schema:

```json
{
  "type": "object",
  "required": ["query"],
  "additionalProperties": false,
  "properties": {
    "query": { "type": "string", "minLength": 1 },
    "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
  }
}
```

The response MUST be small and MAY return refs for full method descriptions.

### `{ns}_codemode_describe`

Input schema:

```json
{
  "type": "object",
  "required": ["name"],
  "additionalProperties": false,
  "properties": {
    "name": { "type": "string", "minLength": 1 }
  }
}
```

`name = "capabilities"` MUST return the manifest in §3.

## 5. Plan forms

A conforming substrate MUST accept:

- `recipe`: named built-in recipe plus arguments.
- `json`: structured DAG/plan with explicit operations and dependency edges.
- `js`: sandboxed JavaScript using the substrate's CodeMode bindings and shared `ctx` helpers.

## 6. Context helpers

Every JS sandbox MUST bind `ctx.step` and `ctx.ref` with identical signatures:

```ts
ctx.step<T>(name: string, fn: () => T): T
ctx.ref(value: unknown): string
```

`ctx.step` MUST execute the callback, record a step named `name`, and return the callback result. Value-form `ctx.step(name, value)` is non-conforming.

`ctx.ref(value)` MUST store `value` behind a substrate ref and return a ref string matching §7.

## 7. Ref and execution-ID scheme

Execution IDs MUST match:

```text
^cm://exec/\d+-[0-9a-f]{12}$
```

Execution artifact refs MUST match:

```text
^{ns}://codemode/execution/[^/]+/(code|steps|telemetry|result|error)$
```

Payload blobs MUST match:

```text
^{ns}://blob/[0-9a-f]{64}$
```

All CodeMode output refs MUST use one of those two forms:

```text
^{ns}://(blob/[0-9a-f]{64}|codemode/execution/[^/]+/(code|steps|telemetry|result|error))$
```

No bare/unprefixed refs are allowed in CodeMode output. The same substrate resolver that expands `{ns}://blob/<sha256>` MUST also parse and expand `{ns}://codemode/execution/<safe-id>/<part>`.

The `<safe-id>` segment is the execution id without the `cm://exec/` prefix. Example:

```json
{
  "execution_id": "cm://exec/1782920000000-012345abcdef",
  "result_ref": "gz://codemode/execution/1782920000000-012345abcdef/result"
}
```

## 8. Telemetry schema

Telemetry MUST validate against `schemas/telemetry.schema.json` and MUST contain only this frozen top-level vocabulary plus optional `extra`:

```json
{
  "kind": "codemode.execute",
  "status": "ok",
  "logical_ops": 100,
  "physical_ops": 4,
  "batched_ops": 1,
  "internal_actions": 110,
  "cache_hits": 2,
  "cache_misses": 1,
  "store_writes": 5,
  "wall_ms": 8,
  "bytes_materialized": 4096,
  "extra": {
    "substrate_detail": "namespaced here only"
  }
}
```

Required keys: `kind`, `status`, `logical_ops`, `physical_ops`, `batched_ops`, `internal_actions`, `cache_hits`, `cache_misses`, `store_writes`, `wall_ms`, `bytes_materialized`.

`raw_leak` is deleted from the contract. It MUST NOT appear in telemetry, execution records, capability manifests, reports, or success/failure responses. Leak safety is proven by G4: oversize results are reachable only by ref and never inline.

## 9. Error taxonomy

Every failure path MUST return `ack = "X0"` and an error ref pointing to an error object matching `schemas/error.schema.json`:

```json
{ "kind": "sandbox", "message": "fetch is not available", "retryable": false }
```

`kind` MUST be one of:

- `validation`: malformed plan, invalid schema, invalid arguments.
- `sandbox`: denied global, denied host capability, memory/microtask sandbox violation.
- `runtime`: thrown JS, panic converted at the execution boundary, uncaught callback exception.
- `substrate`: missing target, unresolved ref, native substrate failure.
- `policy`: operation denied by declared mutation capability or launch policy.

`message` MUST be a non-empty string. `retryable` MUST be boolean.

## 10. Execution record

An execution record MUST validate against `schemas/execution-record.schema.json`. It MUST include:

- `execution_id`
- `ns`
- `status`
- `refs` for `code`, `steps`, `telemetry`, and either `result` or `error`
- `telemetry`
- `error` when `status = "error"`

Every ref in the record MUST follow §7.

## 11. Output-guard and leak-proof rule

A visible response MUST be less than or equal to the effective `max_output_bytes` limit. If an execution result would exceed the limit, the substrate MUST store it behind `{ns}://codemode/execution/<safe-id>/result` or `{ns}://blob/<sha256>` and return only refs and bounded summary metadata.

## 12. Sandbox denial categories

Every category below MUST have a dedicated negative conformance check and MUST fail with `Error.kind = "sandbox"`:

| Category | Examples that must not be available |
|---|---|
| network/fetch | `fetch`, `XMLHttpRequest`, `WebSocket` |
| env | environment variable APIs |
| process/spawn | `process`, `child_process`, `spawn`, `exec` |
| raw host FS | native filesystem modules outside substrate bindings |
| direct DB/store | sqlite/store internals not mediated by CodeMode bindings |
| native modules | `require`, `import`, `node:`, Deno/Bun native access |
| timers | `setTimeout`, `setInterval`, unbounded timer APIs |

Capability-scoped bindings are primary. Substring scanners MAY remain as defense in depth but MUST NOT be the only mechanism claimed by the contract.

## 13. Conformance checks

The conformance crate names these checks G1-G10:

| Check | Maps audit gap(s) | Requirement |
|---|---|---|
| G1 exposure | #1 | `--mode=codemode` lists exactly the three CodeMode tools; `--mode=mcp` lists zero CodeMode tools. |
| G2 refs | #2 | Every CodeMode ref and execution ID matches §7. |
| G3 telemetry | #4, #5 | Telemetry validates schema; no unknown top-level fields; no `raw_leak`. |
| G4 leak-proof | #5 | A >64 KiB result returns bounded visible output and only refs to full payload. |
| G5 errors | #10 | One failure for each error kind validates the taxonomy. |
| G6 ctx.step | #8 | `ctx.step(name, () => value)` executes callback and records the step. |
| G7 limits | #6 | Every echoed limit is violated once and enforcement is observed. |
| G8 mutation capability | #3 | Behavior matches `allowed`, `denied`, or `readonly`. |
| G9 coalescing | #4, #9 | N=100 logical batch reads coalesce: `physical_ops` is much less than 100 and `batched_ops >= 1`. |
| G10 sandbox denial | #7 | Every denial category in §12 fails with `kind = "sandbox"`. |

Durable step-log/replay is deferred and is not mapped to a v1.0 conformance check.

## 14. Reports

A conformance run MUST emit a JSON report at `conformance/reports/<ns>-<date>.json` matching:

```json
{
  "ns": "gz",
  "bin": "/path/to/graphzero",
  "contract_version": "1.0",
  "passed": false,
  "checks": [
    { "id": "G1", "name": "exposure", "passed": true, "details": [] },
    { "id": "G2", "name": "refs", "passed": false, "details": ["bare ref: codemode/execution/1/result"] }
  ]
}
```

A red report is useful output for Wave 1 substrate sessions. The conformance harness MUST prefer actionable diffs over only pass/fail booleans.

## 12. RACC conformance gates

The deterministic hub suite publishes six machine-readable gate IDs. It owns immutable fixtures, derives expected results independently, and MUST NOT accept a substrate verifier or self-reported arithmetic as proof.

| Gate | Normative invariant | Reference |
|---|---|---|
| `RACC-CERT` | Every supported typed query returns the exact payload, locked parser/index/operator provenance, query-bound completeness witness, and no omissions or extras. | T2 |
| `RACC-RECEIPT` | Replay identity and exact per-phase arithmetic include successful and failed trials, retries, verification/recovery calls, expansions, and fallback charges. | T8, 12.2 |
| `RACC-GATE-IRREV` | An irreversible effect without verified evidence routes to `RawFallback` rather than committing a compressed decision. | T2, T8 |
| `RACC-BUDGET` | Expansion budgets are nested monotone doublings and independently satisfy the cumulative factor-4 bound. | T10 |
| `RACC-INLINE` | A certified payload and its certificate arrive in one substrate round trip. | 12.2 |
| `RACC-RESIDENCY` | Resident objects recover byte-identically with metadata; guarded removal produces a typed miss. | T8 |

### Release aggregate

Paper 12.2 release evidence MUST fix the preregistered target identity and digest before evaluation and report each task's raw cost `R`, compressed cost `C`, and ratio `C/R`. Every task MUST show no statistically or transactionally demonstrated regression through powered paired evidence or a valid T13 no-regret receipt. Accounting MUST include all fallback, retry, failed-trial, verification, recovery, and failed-expansion charges. A green run against the deterministic fake substrate validates the hub harness only; it is explicitly **not** a production release pass.

These gates make no universal compression-percentage claim (T5) and no semantic-sufficiency claim (T6). They establish only the listed machine-checkable invariants.


## CONTRACT T13: objective task-verified speculation

A transactional task attempt begins inside a named journal and carries a nonzero, integer attempt cost. TaskAcceptanceReceipt is an opaque linear capability: it is not deserializable, has no public fields, and is minted only by the hub verify_task_acceptance function after an injected trusted TaskAcceptanceVerifier authenticates actual run evidence. The receipt fixes task_id, verifier CommandId, verifier-environment digest, passing outcome and zero exit code, expected and observed artifact digests, journal_id, and attempt cost. Artifact lists are bounded to 64 entries. A nonzero exit, count mismatch, or digest mismatch is a typed verifier failure; model judgment is never evidence.

A sandbox attempt has no commit operation. A passing receipt permits the private commit transition. Verifier rejection, objective failure, or a missing receipt produces deterministic raw rollback carrying the same journal and attempt cost. Both committed and rolled-back attempts MUST charge their nonzero cost through zero-ledger checked append-only accounting (Billed for committed attempts, FailedTrial for rolled-back attempts). Missing charge and missing-receipt commit are conformance failures. Transaction mechanics are pure synchronous state transitions with no I/O or async work.

EffectClass::Irreversible task speculation is rejected before an attempt even when a receipt is supplied. Pre-action Certified evidence behavior is unchanged. Reversible and approval-required transactional attempts remain policy-controlled. The prior six RACC gates and the Paper 12.2 release aggregate remain conjunctive; T13 adds a gate and does not weaken either.

### Required pi-stack host mirror

No existing T13 pi-stack host bead ID was found in repository metadata. Do not invent one. Create and link a downstream mirror with this exact specification:

**Title:** `feat(pi-zerostack): drive T13 journaled task verification and charge attempts`

**Acceptance:**

1. Begin every speculative task under an FSZero TransactionJournal; capture the verifier CommandId, pinned environment digest, exit code, expected and observed artifact digests, journal ID, and nonzero integer attempt cost from the actual run.
2. Inject the trusted host verifier and pass actual evidence to the hub contract; commit only with the returned passing TaskAcceptanceReceipt.
3. Roll back to raw on verifier rejection, nonzero exit, artifact mismatch, or missing receipt; expose and charge the same attempt cost on every path.
4. Reject EffectClass::Irreversible before sandbox execution while preserving Certified pre-action behavior and approval policy.
5. Add end-to-end host tests for passing commit, failing rollback, missing-charge and missing-receipt mutations, irreversible rejection, and journal teardown.
