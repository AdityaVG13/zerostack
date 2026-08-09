# CodeMode and MCP mode

ZeroStack engines can run in one of two integration modes. These are alternatives, not layers.

## Standard MCP adapter

The harness registers engine tools and invokes each operation through MCP. This mode favors compatibility with clients that expect ordinary tool calls.

## CodeMode

The harness exposes a constrained JavaScript execution surface. An agent submits a plan that can call typed ZeroStack APIs sequentially or in parallel, reuse intermediate refs, and return a small final result.

~~~js
const [files, graph] = await Promise.all([
  zero.fs.compound("search", { query: "recoverable ref" }),
  zero.graph.orient("architecture", "ref flow"),
]);

return { files: files.content, graph: graph.content };
~~~

This Cloudflare-style execution model reduces protocol round trips and keeps intermediate values inside the sandbox rather than exposing each one to model context.

### Capability result shape

Every public `zero.*` capability returns `zero-result/v1` with exactly two top-level fields:

- `ack` -- a bounded status atom.
- `content` -- either `{kind:"inline", value:...}` or `{kind:"ref", ref:"<canonical ref>", preview?:"..."}`.

The aggregate host normalizes the transport-owned worker result before JavaScript observes it. It never synthesizes legacy `text`, `visible`, `stdout_ref`, or flat `ref` aliases. Inline content retains the complete validated worker result, including its domain value and transport metadata. A producer selects ref content only with an explicit typed `kind:"ref"` result.

~~~js
const run = await zero.token.shell("echo hi");
if (run.content.kind === "inline") {
  return run.content.value;
}
return await zero.token.expand(run.content.ref);
~~~

Reading any field outside the active typed shape throws a `TypeError` instead of yielding a silent `undefined`:

~~~js
run.text;  // TypeError: unknown property 'text' ... available properties: ack, content
~~~

Use `Object.keys(value)` to inspect an unfamiliar nested domain value. The strict guard applies recursively.

## Install the `zs` wrapper

The tracked Python wrapper supports Linux and macOS and has no third-party dependencies.

~~~sh
python3 scripts/install_zs.py --dry-run
python3 scripts/install_zs.py
python3 scripts/install_zs.py --verify
zs --version
~~~

The default destination is `~/.kimi-code/bin/zs`. Use `--prefix DIR` for another user bin directory. Installation uses a temporary sibling, fsync, executable-mode preservation, and atomic replacement.

Smoke from any directory while preserving engine root isolation:

~~~sh
zs -C . --verbose fs 'return await zero.fs.compound("list", { path: "." });'
printf '%s
' 'return await zero.graph.orient("delta");' | zs -C . graph -
zs -C . fs-search 'read a file'
~~~

`-C/--root` must precede the engine command. Plan paths stay relative to that validated root. `--verbose` reports wrapper and invoked engine version/revision. `--json` emits the complete engine result. Normal output preserves typed status and copyable scalar `fz://`, `gz://`, `tz://`, and `cm://` refs. Unpinned engines resolve through the same order as [binary discovery](#binary-discovery-for-embedded-harnesses), never from a live `target/release` tree: a Cargo build unlinks and replaces those files, so a build in one checkout would kill every concurrent session spawning from it. Binary overrides are `ZS_FSZERO_BIN`, `ZS_GRAPHZERO_BIN`, and `ZS_TOKENZERO_BIN`; `ZS_TIMEOUT_MS` changes the 120000 ms default.

## Binary discovery for embedded harnesses

A harness needs four executables: the aggregate host `zerostack-codemode-host` and
the `fszero-codemode` / `graphzero-codemode` / `tokenzero-codemode` delegates. None of
them belong in a config as an absolute path — a shipped config that names one
developer's worktree resolves nothing on any other machine and fails at spawn
time with a bare `ENOENT`.

Ask the host where things are instead:

~~~sh
zerostack-codemode-host --locate
~~~

It prints a `zerostack.binary_discovery.v1` JSON report naming, per binary, either
the resolved path and the rule that found it, or every candidate that was probed.

Resolution order, highest precedence first:

| Order | Source label | Location |
| --- | --- | --- |
| 1 | `zerostack_home` | `$ZEROSTACK_HOME/bin/<binary>` |
| 2 | `dev_checkout` | `$ZEROSTACK_DEV_ROOT/<Repo>/target/release/<binary>` |
| 3 | `xdg_data` | `$XDG_DATA_HOME/zerostack/bin/<binary>`, default `$HOME/.local/share/zerostack/bin` |
| 4 | `platform_install` | `/usr/local/lib/zerostack/bin`, `/opt/zerostack/bin`, `/usr/lib/zerostack/bin`; on Windows the `LOCALAPPDATA` and Program Files equivalents |
| 5 | `path` | each absolute `PATH` entry, in order |

`ZEROSTACK_DEV_ROOT` is the documented dev-checkout override: point it at the
parent directory holding the sibling `ZeroStack`, `FSZero`, `GraphZero`, and
`TokenZero` checkouts and their `target/release` builds are used directly.

Rules that keep resolution predictable:

- An empty or whitespace-only variable is treated as unset. Exporting a blank
  value is a shell artifact, not an instruction.
- A relative install root, or a blank `PATH` entry meaning "current directory",
  is skipped. Otherwise resolution would silently depend on the spawning cwd.
- An explicit `XDG_DATA_HOME` replaces the `~/.local/share` default rather than
  adding to it, so a redirect is honored instead of quietly bypassed.
- Only a regular file with an execute bit is accepted, so a same-named directory
  cannot shadow a real binary further down the order.
- Each directory is probed once even if several rules name it.

An engine that is not installed is reported as unresolved rather than aborting
discovery; the harness decides which delegates it actually requires.

## Exclusive deployment rule

Choose exactly one:

| Deployment | Register standard engine MCP tools | Register CodeMode |
| --- | ---: | ---: |
| Standard MCP mode | Yes | No |
| CodeMode | No | Yes |

Never register both for the same deployment. Duplicate surfaces waste context, create ambiguous routing, and can split state. The active ZeroStack deployment uses CodeMode only.

CodeMode is not a fourth engine. It is an execution mode over TokenZero, FSZero, and GraphZero.

See the [Release-N engine MCP compatibility policy](mcp-compatibility-policy.md) for defaults, maintenance scope, migration, and staged removal gates.

### Shared surface registration

`zero-codemode` exposes `SurfaceRegistration` as the hub-owned install-time
contract. An engine adapter supplies one `DomainAdapterRegistration` containing
its `CanonicalRegistry`, effect/approval policy, `RefOwnership`, telemetry
schema, and capability descriptors. The host converts only a CodeMode
registration into `GlobalRegistration`; an MCP registration is rejected at that
boundary. `SurfaceKind` therefore models one selected artifact face, not a
runtime switch or a dual catalog.

This contract is metadata and validation only. It does not import FastMCP,
QuickJS, or engine-domain code. An engine compatibility package may provide a
thin MCP carrier, but that carrier must consume the same registration and must
not reimplement the registry, result envelope, ref ownership, or telemetry.

## Harness-neutral raw-worker client

The zero_codemode::worker module is the stable Rust ownership boundary between
any harness and raw-worker v2 processes. It is independent of Pi, MCP, QuickJS,
and engine runtimes. WorkerRegistry maps the closed EngineIdentity set
(FsZero, GraphZero, TokenZero) to a WorkerFactory. A factory returns a
WorkerSpec; WorkerClient alone owns spawn, framed stdin/stdout/stderr,
handshake, dispatch, cancel, deadlines, shutdown, kill, and reap.

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
