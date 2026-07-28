# ADR 0001: CodeMode execution boundary

- **Status:** Accepted
- **Date:** 2026-07-28
- **Decision owner:** ZeroStack owner
- **Tracking:** `zerostack-codemode-first-execution-jgm.1`

**Owner approval:** Approved by the ZeroStack owner on 2026-07-28.

## Context

CodeMode is the canonical and default ZeroStack execution mode. Standard engine
MCP adapters remain a compatibility option, but a deployment chooses exactly one
mode and never registers both. The boundary must prevent duplicate runtimes,
implicit host execution, and recursively planned engine calls while preserving
engine ownership of domain behavior.

The key words MUST, MUST NOT, REQUIRED, SHOULD, SHOULD NOT, and MAY are to be
interpreted as described by RFC 2119.

## Decision

### Deployment alternatives

A deployment MUST select exactly one alternative:

| Alternative | Registered surface | Execution path |
| --- | --- | --- |
| CodeMode (canonical/default) | A standalone engine CodeMode entry point or the aggregate `zero_execute` entry point | One sandboxed runtime dispatches canonical operations directly. |
| Standard MCP adapter (compatibility-only) | The selected engines' ordinary per-operation MCP catalogs | The client invokes engine operations through MCP; no CodeMode runtime or `zero_execute` is registered. |

CodeMode is not a fourth engine. The standard MCP adapter is not an inner layer
of CodeMode. Transporting a CodeMode entry point through a harness tool protocol
does not turn it into an ordinary per-engine MCP catalog.

### Standalone engine boundary

A standalone engine contains exactly one sandboxed runtime and one dispatcher.
The dispatcher calls that engine's domain implementation and ref store directly.

~~~text
client / harness
      |
      v
standalone CodeMode transport
      |
      v
+--------------------------------------------------+
| TokenZero, FSZero, or GraphZero process          |
|  one sandboxed runtime                           |
|    - cell lifecycle, scheduler, limits, policy   |
|    - journal/approval/telemetry coordination     |
|             |                                    |
|             v                                    |
|  one canonical operation dispatcher              |
|             |                                    |
|             v                                    |
|  engine core + engine-owned typed ref store      |
+--------------------------------------------------+
~~~

The standalone process MUST NOT start another CodeMode runtime to implement an
operation. Its dispatcher MAY use internal engine helpers, but MUST NOT reach the
engine through its MCP adapter.

### Aggregate boundary

Aggregate CodeMode contains exactly one sidecar process with exactly one
sandboxed runtime. That runtime invokes planner-free engine raw workers using the
private raw-worker v2 protocol. Each worker accepts only canonical typed
operations and calls its engine dispatcher.

~~~text
provider / harness adapter
            |
            v
aggregate zero_execute transport
            |
            v
+--------------------------------------------------------+
| pi-zerostack sidecar                                   |
|  one sandboxed runtime                                 |
|    - cells, scheduler, policy, approvals, journal      |
|    - ref routing/recovery, telemetry                   |
+----------+------------------+--------------------------+
           | raw-worker v2    | raw-worker v2
           v                  v
  +----------------+  +----------------+  +----------------+
  | FSZero worker  |  | GraphZero      |  | TokenZero      |
  | no planner/JS  |  | worker         |  | worker         |
  | dispatcher     |  | no planner/JS  |  | no planner/JS  |
  | engine core    |  | dispatcher     |  | dispatcher     |
  +----------------+  +----------------+  +----------------+
~~~

Raw workers MUST NOT expose a planner, JavaScript runtime, cell runtime, MCP
catalog, provider adapter, or nested CodeMode entry point. Their protocol is the
bounded NDJSON handshake/call/cancel/shutdown contract in
`conformance/schemas/raw-worker-v2.schema.json` and
`crates/zero-abi/src/raw_worker.rs`.

### Transport and catalog distinction

An **engine MCP catalog** is the compatibility surface owned by one engine. It
exposes that engine's ordinary operations as separate MCP tools. It is selected
only in standard MCP mode and MUST be absent from a CodeMode deployment.

The aggregate **`zero_execute` transport** is the single CodeMode ingress owned
by `pi-zerostack`. It carries one plan into the aggregate sandbox and dispatches
planner-free calls over raw-worker v2. It MUST NOT discover, register, or call
engine MCP catalogs. A harness MAY carry `zero_execute` as one tool call, but
that carrier is only transport and does not change this execution boundary.

## Normative invariants

1. **One runtime:** A standalone execution MUST contain one sandboxed runtime and
   one dispatcher. An aggregate execution MUST contain one sidecar/runtime and
   only planner-free raw workers beneath it.
2. **No ambient host eval:** Untrusted plan JavaScript MUST execute only in the
   constrained CodeMode sandbox. Provider adapters, transport handlers, raw
   workers, and host processes MUST NOT evaluate it with ambient Node, shell,
   Python, `eval`, `Function`, `vm`, or equivalent host facilities.
3. **No nested planners:** A runtime MUST NOT invoke another planner or CodeMode
   runtime. Aggregate workers MUST accept canonical typed operations, not plans.
4. **Direct dispatch:** Standalone runtimes MUST call their local dispatcher.
   Aggregate runtimes MUST call engine dispatchers only through raw-worker v2.
   Neither path may traverse an engine MCP adapter.
5. **Exclusive mode:** A deployment MUST register CodeMode or standard engine
   MCP, never both. CodeMode is canonical/default; engine MCP is
   compatibility-only and never enters the CodeMode hot path.
6. **Fail-closed binding:** Aggregate startup MUST bind each worker to engine,
   root, session, revision, semantic contract digest, operation registry digest,
   and ref scheme. A mismatch MUST fail before the first call.
7. **Single policy decision chain:** The runtime owns orchestration policy and
   the engine dispatcher owns domain authorization/effect classification. A
   transport or provider adapter MUST NOT bypass either decision.
8. **Ref provenance:** Every returned ref MUST retain engine and session
   ownership. The aggregate may route and recover refs but MUST NOT rewrite an
   `fz://`, `gz://`, or `tz://` ref as if another engine owned it.
9. **Journal and approval propagation:** Effect, approval, revert, snapshot, and
   journal metadata MUST survive dispatcher-to-runtime transport. Raw workers
   MUST NOT prompt for approval; the owning runtime coordinates approval before
   an approval-required effect proceeds.
10. **Cell containment:** The runtime creates, limits, cancels, and destroys
    cells. Raw workers know request IDs and trace cell IDs only; they MUST NOT
    create child cells. Cell termination MUST cancel in-flight worker calls.
11. **Trace continuity:** Runtime, cell, request, trace, parent span, worker
    revision, and contract digest identities MUST remain correlated across the
    transport. Unknown or unbounded worker frames MUST fail closed.
12. **Provider neutrality:** Engine cores, dispatchers, raw workers, schemas, and
    shared runtime foundations MUST remain provider-neutral. Provider-specific
    adaptation terminates before the CodeMode ingress.

## Ownership

The public ZeroStack hub owns shared contracts, ABI/schema types, this ADR, and
suite-level conformance. Engine product code remains in its engine repository.
TokenZero is public; FSZero and GraphZero are private and in development.

In `crates/zero-abi/src/raw_worker.rs`, the statement that aggregate CodeMode
owns refs, journaling, and telemetry refers to cross-worker ref routing,
aggregate journal coordination, and plan/cell trace assembly. Engines retain
ownership of ref storage and namespace, domain transactions and revert
primitives, and engine spans.

The aggregate Pi host is tracked downstream by `pi-stack-l73.1`; engine tracks
are `tokenzero-slim-public-repo-4uql`,
`fszero-zerostack-parity-uxin`, and
`graphzero-zerostack-parity-b5ci`.

### Validation gates

These gates validate implementations; they are not claims that all downstream
suites already pass, and they do not determine ADR status. Only explicit owner
approval moves this ADR from Proposed to Accepted.

| Gate | Concrete evidence |
| --- | --- |
| **RW2** | In this hub, `cargo test --manifest-path conformance/Cargo.toml --test raw_worker_v2` validates the schema, shared Rust types, golden frames, and typed-call round trips. `cargo test -p zero-abi raw_worker` validates bounded/unknown frame denial, handshake binding, deadlines, and the deterministic protocol manifest. A-BOUNDARY supplies behavioral proof of the manifest's planner/JavaScript/MCP/nested-CodeMode negative space. |
| **G1-G10** | For each installed engine artifact, run `zerostack-codemode-conformance --ns <fz|gz|tz> --bin <engine-codemode> --surface codemode --reports-dir conformance/reports`; the emitted namespace report MUST be green. G1 covers exclusive exposure, G2 refs, G3 telemetry, G4 leak-proof output, G7 limits, G8 mutation capability, and G10 sandbox denial. |
| **S-TOPOLOGY** | Each engine track MUST have a packaged-boundary test that observes exactly one runtime and one dispatcher, verifies local direct dispatch, and fails if the standalone path spawns CodeMode or invokes MCP. |
| **A-BOUNDARY** | `pi-stack-l73.1` MUST have an aggregate integration test that observes one `pi-zerostack` sidecar/runtime, handshakes planner-free raw workers, proves that only `zero_execute` is registered, denies ambient host eval, and fails on any worker planner/JavaScript/MCP/CodeMode surface. |
| **A-EFFECTS** | `pi-stack-l73.1` MUST exercise cancellation, deadline, approval required/granted/denied, reversible rollback, irreversible denial, ref provenance, cell teardown, and end-to-end trace correlation across raw-worker v2. |
| **OWNER** | The ZeroStack owner MUST explicitly approve this ADR. Approval is recorded by changing `Status` to `Accepted`; no other event is equivalent. |

### Component-to-repository matrix

In this matrix, `engine repository` means TokenZero for `tz`, FSZero for
`fz`, and GraphZero for `gz`. The ZeroStack hub never owns their product
implementation.

| Component | Standalone owner | Aggregate owner | Shared contract owner | Required gate(s) |
| --- | --- | --- | --- | --- |
| Sandbox | The selected TokenZero, FSZero, or GraphZero repository | `pi-zerostack` | ZeroStack conformance limits and denial taxonomy | G7, G10, S-TOPOLOGY, A-BOUNDARY |
| Scheduler | The selected engine repository's runtime | `pi-zerostack` runtime | ZeroStack raw-worker deadlines/cancellation/limits ABI | RW2, G7, S-TOPOLOGY, A-EFFECTS |
| Policy | Engine dispatcher for domain authorization; engine runtime for orchestration | `pi-zerostack` for orchestration plus each worker's engine dispatcher for domain authorization | ZeroStack effect/approval/revert contract | RW2, G7, G8, A-EFFECTS |
| Refs | Engine repository owns storage, namespace, validity, and recovery | `pi-zerostack` owns the cell/session registry and cross-worker routing; the originating engine retains ref ownership | ZeroStack ZeroRef/raw-worker ownership contracts | RW2, G2, G4, A-EFFECTS |
| Journal | Engine repository owns domain transaction and revert primitives; its runtime coordinates the execution | `pi-zerostack` coordinates the aggregate execution journal; engines execute declared rollback operations | ZeroStack revert metadata and mutation capability contract | RW2, G8, S-TOPOLOGY, A-EFFECTS |
| Approvals | Engine runtime coordinates; dispatcher classifies effects | `pi-zerostack` coordinates with the provider adapter; raw workers only report/verify metadata | ZeroStack approval/effect metadata contract | RW2, A-EFFECTS |
| Telemetry | Engine runtime emits execution telemetry; dispatcher supplies engine spans | `pi-zerostack` assembles plan/cell traces; workers supply correlated spans | ZeroStack telemetry and raw-worker trace schemas | RW2, G3, A-EFFECTS |
| Cell lifecycle | The selected engine repository's one runtime | `pi-zerostack` one runtime | ZeroStack cell/trace/limit contract | G7, S-TOPOLOGY, A-BOUNDARY, A-EFFECTS |
| Provider adapters | `pi-zerostack` for the Pi integration; engine repositories remain provider-neutral | `pi-zerostack` | ZeroStack transport-neutral request/result contract | A-BOUNDARY, A-EFFECTS |
| Aggregate `zero_execute` transport | Not present in standalone execution | `pi-zerostack` | ZeroStack exclusive-deployment and raw-worker contracts | RW2, A-BOUNDARY |
| Compatibility transports | Each engine repository owns its standard MCP adapter; it is absent in CodeMode | None in aggregate CodeMode; `pi-zerostack`'s `zero_execute` carrier is canonical, not a compatibility transport | ZeroStack exclusive-deployment contract | G1, S-TOPOLOGY, A-BOUNDARY |

## Rejected alternatives

### Ambient host evaluation

Rejected. Evaluating submitted JavaScript in the provider adapter, transport
handler, sidecar host globals, shell, or an unrestricted language runtime makes
sandbox policy non-authoritative and splits telemetry, approval, and lifecycle
ownership.

### CodeMode nested in CodeMode

Rejected. Having aggregate CodeMode call standalone CodeMode, a raw worker
planner, or an engine MCP catalog creates multiple schedulers, policy chains,
journals, and cell lifecycles. It also makes cancellation and approval ordering
ambiguous.

### One runtime per engine in aggregate mode

Rejected. Aggregate orchestration requires one scheduler and one cell lifecycle.
Raw workers provide process isolation and engine dispatch without introducing
additional runtimes.

### One monolithic engine binary

Rejected. Engines remain independently owned and released. The raw-worker v2
boundary composes them without moving product code into this hub or the sidecar.

## Consequences

- CodeMode has one authoritative sandbox, scheduler, policy chain, and cell
  lifecycle at each selected execution boundary.
- Aggregate startup pays worker handshake and process-isolation costs, but plan
  execution avoids MCP round trips and nested runtime overhead.
- Engine MCP remains available for compatible clients while staying outside the
  canonical CodeMode path.
- Moving this ADR to Accepted requires explicit owner approval. Downstream
  topology/effects evidence validates implementations independently of ADR
  status.
