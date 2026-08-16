# CodeMode and MCP mode

ZeroStack engines can run in one of two integration modes. These are alternatives, not layers.

## Standard MCP adapter

The harness registers engine tools and invokes each operation through MCP. This mode favors compatibility with clients that expect ordinary tool calls.

## CodeMode

The harness exposes a constrained JavaScript execution surface. An agent submits a plan that can call typed ZeroStack APIs sequentially or in parallel, reuse intermediate refs, and return a small final result.

~~~js
const [files, graph] = await Promise.all([
  zero.fs.compound("search", { query: "recoverable ref" }),
  zero.graph.orient("context", "architecture"),
]);

return { files: files.content, graph: graph.content };
~~~

This execution model reduces protocol round trips and keeps intermediate values inside the interpreter rather than exposing each one to model context.

### Restricted interpreter provenance

ZeroStack's owned Rust interpreter adopts the restricted in-process architecture reviewed directly in `anomalyco/opencode`, branch `dev`, commit `0bff28de09105088ff5bdefab91413d55c28dff1`, package `packages/codemode`. That upstream package is MIT licensed, Copyright (c) 2025 opencode. ZeroStack reimplements the architecture against its own ABI, planner, raw-worker, ref, store, lifecycle, and conformance contracts; it does not execute source through `eval`, Node.js, QuickJS, or any ambient host runtime.

The upstream MIT terms permit use, copying, modification, distribution, sublicensing, and sale provided the copyright and permission notice remain with copies or substantial portions. The software is provided "AS IS", without warranty, and the authors are not liable for claims or damages arising from its use. This provenance record binds the reviewed source revision and those obligations.

### Capability result shape

Every public `zero.*` capability returns `zero-result/v1` with exactly two top-level fields:

- `ack` -- a bounded status atom.
- `content` -- either `{kind:"inline", value:...}` or `{kind:"ref", ref:"<canonical ref>", preview?:"..."}`.

The aggregate host normalizes the transport-owned worker result before JavaScript observes it. It never synthesizes legacy `text`, `visible`, `stdout_ref`, or flat `ref` aliases. `content.value` is the **domain payload** (the worker `value`), not the `{metadata, value}` wrapper. A producer or an oversized execute result uses `kind:"ref"` -- the same shape whether the call is cold or warm.

~~~js
const run = await zero.token.shell("echo hi");
if (run.content.kind === "inline") {
  return run.content.value; // domain payload
}
return await zero.token.expand(run.content.ref);
~~~

Reading any field outside the active typed shape throws a `TypeError` instead of yielding a silent `undefined`:

~~~js
run.text;  // TypeError: unknown property 'text' ... available properties: ack, content
~~~

Use `Object.keys(value)` to inspect an unfamiliar nested domain value. The strict guard applies recursively.

For composed plans, `ctx` provides transport-safe helpers so callers do not
hard-code nested envelope paths:

~~~js
const read = await zero.fs.compound("read", { path: "README.md" });
const domain = ctx.result(read);   // validated domain result
const payload = ctx.payload(read); // domain result's value
const refs = ctx.refs(read);       // ownership refs, or []
return { operation: domain.operation, text: payload.payload_utf8, refs };
~~~

## Run the native `zsx` executable

Build the canonical one-process runtime:

~~~sh
cargo build -p zsx --release
printf '%s\n' 'return await zero.fs.read({path:"Cargo.toml"});' \
  | ./target/release/zsx exec -C "$PWD"
~~~

`zsx exec` reads a plan from stdin or `--file PLAN`. `-C ROOT` authorizes and canonicalizes one engine root. Paths in the plan are relative to that root; absolute paths stay rejected. `--timeout-ms N` overrides the 30000 ms default. Discover call shapes with `zero.help()` or `zero.help.search({query})`.

The executable embeds `zsx-core` and all three domain adapters. It creates no engine worker, NDJSON pipe, session socket, or daemon. Pi and OMP load the same runtime through `@zerostack/zsx-native`; their adapters do not own execution authority.

## Native addon selection for embedded harnesses

Pi and OMP import `@zerostack/zsx-native`. Its loader checks `ZSX_NATIVE_ADDON` first, then selects an exact packaged `prebuilds/<platform>-<arch>/zsx.node` file.

Missing and unsupported addons fail with typed errors. The loader never invokes Cargo, Git, a shell, or a sibling repository.

## Bounded verdict loops

`ZsxSession::execute_verdict_loop` runs a server-side plan under a declared
`VerdictLoopEnvelope`. The envelope caps logical dispatches, encoded raw-worker
request and response bytes, and cumulative raw, visible, recovery, billed, and
cached token accounting. Every dispatch requests `WorkerTokenAccountingV1`;
missing, malformed, estimated, overflowed, or over-budget accounting is a
typed `verdict_rejected` failure and remains terminal even if plan JavaScript
catches the capability rejection.

The plan may compose ordinary capabilities, including launching a background
TokenZero shell job and polling `zero.token.job`, but it must return exactly
the string `pass` or `fail`. The model-visible decision and
`VerdictLoopReceiptV1` are separate Rust values. The receipt records exact JSON
bytes for the final atom, encoded frame bytes, tokenizer/count-kind labels, and
leaves `exact_ref_tokens` unknown when a worker cannot certify it. The raw
worker remains planner-free; no JavaScript runtime or catalog enters an engine.

## Exclusive deployment rule

Choose exactly one:

| Deployment | Register standard engine MCP tools | Register CodeMode |
| --- | ---: | ---: |
| Standard MCP mode | Yes | No |
| CodeMode | No | Yes |

Never register both for the same deployment. Duplicate surfaces waste context, create ambiguous routing, and can split state. The active ZeroStack deployment uses CodeMode only.

CodeMode is not a fourth engine. It is an execution mode over TokenZero, FSZero, and GraphZero.

See the [engine MCP compatibility policy](mcp-compatibility-policy.md) for the
default, maintenance scope, local migration, permanent no-release boundary, and
source-removal rule.

### Shared surface registration

`zero-abi` exposes `SurfaceRegistration` as the hub-owned install-time
contract. An engine adapter supplies one `DomainAdapterRegistration` containing
its `CanonicalRegistry`, effect/approval policy, `RefOwnership`, telemetry
schema, and capability descriptors. The host converts only a CodeMode
registration into `GlobalRegistration`; an MCP registration is rejected at that
boundary. `SurfaceKind` therefore models one selected artifact face, not a
runtime switch or a dual catalog.

This contract is metadata and validation only. It does not import FastMCP,
an unrestricted JavaScript runtime, or engine-domain code. An engine compatibility package may provide a
thin MCP carrier, but that carrier must consume the same registration and must
not reimplement the registry, result envelope, ref ownership, or telemetry.

The hub-owned FastMCP carrier lives in the `zero-mcp` crate and uses
`McpTransportConfig`. It permits at most `MAX_MCP_MAX_INFLIGHT` (256)
concurrent callbacks. Zero and larger values are rejected. The `fastmcp`
compatibility transport remains an optional feature of `zero-mcp`.

## Legacy raw-worker conformance client

`zero_codemode::worker` remains only for raw-worker v2 compatibility tests and
protocol conformance. `zsx`, the Node binding, Pi, and OMP do not construct it.
The module owns bounded spawn, framed I/O, handshake, cancellation, shutdown,
and reap when a compatibility test explicitly creates a `WorkerClient`.

Clients provide an explicit store root and session id in WorkerContext and pin
worker revision, semantic contract digest, and operation registry digest in the
factory. The adapter propagates context through the canonical raw-worker v2
handshake and ZEROSTACK_STORE_ROOT, ZEROSTACK_SESSION_ID, and ZEROSTACK_ENGINE.
Startup fails closed unless protocol version/digest, engine, root, session,
semantic digest, registry digest, and revision all match. Raw-worker v2 and its
digest are unchanged by this process adapter.

WorkerClientConfig bounds NDJSON frames, the stdout queue, response payloads,
stderr capture, startup, operation deadlines, and shutdown. Reader threads never
require an unbounded join, including when another process keeps stderr open. Failures are typed as
WorkerAdapterError protocol, handshake, bounds, deadline, crash, remote,
registration, spawn, or I/O errors. WorkerAccounting reports request and byte
counts. An optional observer receives lifecycle, timing, byte, deadline, crash,
and bounds observations. dispatch_with_cancel accepts a cloneable CancellationSignal, sends Cancel for
the active request, requires the matching CancelAck, and returns the typed
Cancelled error after reaping. Mismatched response ids fail closed. Dropping a
client performs bounded shutdown and reaps the child; deadline, bounds, cancel,
and protocol failures make the client terminal. is_reaped and terminal_status
expose the direct child wait result without platform-specific PID probing.

Engine migration beads adopt this boundary without moving runtime logic:

1. Keep each engine's operation registry and domain dispatch in its engine.
2. Expose one planner-free raw-worker v2 binary using the existing zero-abi
   frames and digest.
3. Register a factory with the engine identity, executable, revision, semantic
   contract digest, and registry digest.
4. Launch through WorkerRegistry with the harness store root and session id;
   remove copied process, framing, timeout, cancellation, and reaping code only
   after that engine's migration conformance passes.

The feature-gated zero-codemode-worker-fixture exists only for adapter
conformance and is not built by default.

### Worker lifecycle details

Deadline conversion uses checked Instant arithmetic. Deadlines outside the
portable signed-millisecond range fail as DeadlineOverflow and terminate the
worker. Every forced termination kills the owned process tree and waits for the
direct child. command-group 5.0.1 retains the platform containment handle:
Unix uses a dedicated process group and Windows uses a kill-on-close Job Object.
The Linux process-group path is exercised on Spark; the Windows Job Object path
is provided by command-group but is not executed by the Linux-only test host. Drop retries termination whenever no direct
child ExitStatus has been collected.

A standalone CancelAck with cancelled=false is a correlated, non-poisoning
Ok(false). During dispatch cancellation, a true acknowledgement returns the
typed Cancelled error and reaps the tree. A false acknowledgement is accepted
once, is never resent, and permits only the matching result or remote error;
other response correlation fails closed. Matching remote errors emit the same
Dispatch observation as matching results.

Crash stderr is represented by StderrCapture. Its text is bounded, while
observed_bytes saturates, complete states whether EOF was observed, and
truncated states whether observed bytes exceeded retained text. The client uses
a bounded condition-variable wait for direct-child stderr without waiting
indefinitely on inherited descriptors.


All stdin frames use one dedicated writer thread behind a capacity-one channel.
Submission uses try_send and never blocks the caller; the writer acknowledges
write and flush completion before the same checked operation deadline used for
the response. Writer queue, write, or acknowledgement failure terminates the
contained tree and synchronously waits for the direct child. Drop never joins
the writer thread.

When a matching result or remote error races ahead of CancelAck(false), the
client retains that completion, consumes exactly one matching late false
acknowledgement, and then returns the completion. The worker remains reusable.
A true acknowledgement still returns Cancelled and reaps the tree; mismatched
or duplicate correlation fails closed.
