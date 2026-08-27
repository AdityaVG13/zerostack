# ZeroStack

A daemonless, in-process agent kernel that composes filesystem, structure, and output authority behind six operations.

**Status:** active development, Rust 2024 edition, MIT

ZeroKernel is the only model-facing execution surface in this repository. It provides one reusable Rust host and one fresh, bounded JavaScript or TypeScript frame per cell. Each cell receives a single global `z` with exactly six operations. ZeroStack owns host lifecycle, budgets, cancellation, transactions, owned processes, session state, and the terminal response. Three independent engines supply typed contracts behind that surface and never import one another.

> **Project status:** ZeroStack is the source-only composition hub for the Zero family. FSZero, GraphZero, and TokenZero are product domains built from the same Cargo workspace. The six-operation surface is canonical and under active dogfooding, while package and engine contracts may still change. Homebrew, npm, and Pi distribution files are pre-release scaffolds and are not published release claims.

## What ZeroKernel is

An embedding application creates one `ZeroKernel` host for a workspace, session, and budget. The host retains initialized engine adapters and durable roots. Every call creates a fresh frame that owns its interpreter values, promises, cancellation token, staged filesystem transaction, child processes, and dirty state. Completion, failure, or cancellation settles every owned resource and destroys the frame.

```mermaid
graph LR
  H[Embedding application] --> K[Reusable ZeroKernel host]
  K --> F[Fresh bounded frame]
  F --> FS[FSZero]
  F --> G[GraphZero]
  F --> T[TokenZero]
  F --> P[Owned process tree]
  F --> S[CAS session state]
```

ZeroKernel opens no network listener and starts no machine-wide daemon. Rust hosts and the asynchronous Node binding both run in-process.

## Authority boundaries

| Owner | Authority | Visible through ZeroKernel |
| --- | --- | --- |
| ZeroStack | Host lifecycle, budgets, cancellation, transactions, processes, session state, terminal response | All six operations and the final `ZeroKernelResponse` |
| FSZero | Exact bytes, snapshots, guarded file effects, restoration | `z.read`, `z.edit`, `z.apply` |
| GraphZero | Syntax, symbols, relationships, freshness, coverage | `z.find` |
| TokenZero | Measurement, projection, compression, exact expansion | Automatic at operation and response boundaries |

Authority is narrow by design. A graph result does not become file content. A compact projection does not become source bytes. A staged receipt does not commit the cell on its own. See `docs/architecture.md` and `docs/components.md` for the full boundary table and flow.

Engines never import one another. Hub composition lives under `crates/zerostack`. Cross-engine behavior has one place to audit.

## Repository layout

```
crates/zerostack/   ZeroKernel hub, ABI, stores, processes, gates, and conformance
crates/fszero/      byte and filesystem authority
crates/graphzero/   structural search, relationships, freshness, and coverage
crates/tokenzero/   measurement, projection, compression, and exact recovery
contracts/          machine-readable contracts, fixtures, matrices, and digest pins
tests/              hub integration tests, domain tests, and shared support
docs/               canonical architecture, engine guides, and RACC documentation
packaging/package/  pre-release Homebrew, npm, and Pi distribution scaffolds
bindings/node/      source and native prebuilds for @zerostack/zero-kernel
demo/               one end-to-end ZeroKernel example covering all three engines
fuzz/               one cargo-fuzz workspace for hub and engine targets
xtask/              repository maintenance CLI
scripts/            public-surface and release-perf build helpers
benchmarks/         preserved reference harnesses and measured evidence
```

## The six operations

| Operation | Contract |
| --- | --- |
| `z.read` | Read files, directories, snapshots, exact handles, and bounded selections |
| `z.find` | Search text or structure; query definitions, references, callers, imports, paths, and semantics |
| `z.edit` | Create, replace, remove, or patch one file with exact preimage guards |
| `z.apply` | Apply an atomic multi-file operation set or typed effect request |
| `z.run` | Run one bounded process with owned cancellation and recoverable output |
| `z.state` | Get, set, has, delete, or list small session-scoped JSON facts |

There is no guest transaction method. Every mutation in a cell already shares one host-owned transaction. Normal JavaScript or TypeScript provides orchestration.

## Examples

Independent work uses `Promise.all`. Dependent work stays in the same bounded cell.

```javascript
const [source, callers] = await Promise.all([
  z.read("src/lib.rs"),
  z.find("execute_cell", { mode: "callers", path: "src" }),
]);

if (callers.hits.length === 0) {
  return { source, decision: "No callers found" };
}

return { source, callers };
```

A normal UTF-8 file returns complete content. A large value returns a bounded view plus an exact handle that `z.read` can recover.

```javascript
// Bounded views and exact recovery
const lines = await z.read("src/lib.rs", { range: "18:42" });
const snapshot = await z.read({ path: "src/lib.rs", view: { mode: "decision" } });
const page = await z.read(snapshot.source.exact, { offset: 4096, limit: 4096 });
const dir = await z.read("src", { recursive: false });
```

Guarded edits use a snapshot that carries the exact preimage.

```javascript
const snapshot = await z.read({
  path: "src/config.rs",
  view: { mode: "decision" },
});

return await z.edit(snapshot, {
  find: "const RETRIES: usize = 2;",
  replacement: "const RETRIES: usize = 3;",
});
```

Processes are bounded and owned by the host.

```javascript
return await z.run(["git", "status", "--short"], {
  cwd: ".",
  timeoutMs: 30000,
});
```

Session state carries small JSON facts across otherwise fresh frames.

```javascript
await z.state.set("selected-path", "src/lib.rs");
const selectedPath = await z.state.get("selected-path");
return await z.read(selectedPath);
```

A structured handle returned inside a snapshot or projection is accepted by later `z.read` and `z.edit` calls. Do not write an outline or preview back as if it were the source file.

## Guarantees

**Effects and atomicity.** The first file mutation lazily opens one cell transaction. FSZero returns exact preimages and typed receipts. A completed cell publishes effects and a successor state root at the terminal boundary. Failure, cancellation, verification failure, state publication failure, or output publication failure restores receipts in reverse order and leaves the prior state root authoritative. `z.edit` changes one file. `z.apply` changes an atomic set.

**Cancellation and cleanup.** One cancellation token reaches the interpreter, engine calls, concurrent promises, and the owned process tree. On failure or cancellation ZeroKernel requests sibling cancellation, terminates the exact child tree, waits for task and process counters to reach zero within a bounded window, rolls back staged effects and session state, records one terminal event, and destroys the frame. A response cannot report completion while its frame still owns tasks or child processes.

**Process ownership.** `z.run` belongs to ZeroStack. It validates the working directory, starts the exact child tree, captures bounded output, and terminates and reaps descendants on timeout, cancellation, frame failure, shutdown, or object destruction. TokenZero projects captured output but never owns the process lifecycle.

**Determinism and exact recovery.** Source bytes are authoritative through FSZero snapshots and CAS content addressing. Outputs pass through TokenZero measurement and projection at operation and response boundaries. Small values pass through unchanged. Larger values return a bounded projection plus an opaque `z://blob/<digest>` handle whose recovery returns exact bytes. Handles are content-addressed and recovery contributes to recovery-aware accounting. The terminal event records the exact projection or structured error digest.

**State.** `z.state` is a bounded session-scoped JSON map for small facts that must survive fresh frames, such as a selected path or workflow checkpoint. The host hydrates state from the committed CAS root before evaluation and commits a successor root with compare-and-set only on completed dirty cells. Interpreter variables, imports, and promises never persist across cells.

**Terminal response.** Every accepted cell returns one structured response. Completed output is the exact TokenZero projection recorded by the terminal event. No extra model-visible envelope is added.

```typescript
interface ZeroKernelResponse {
  protocol: "ZeroKernel";
  outcome: "Completed" | "Cancelled" | "Failed";
  value?: unknown;
  error?: { kind: string; detail: string; retryable: boolean };
  handles: string[];
  event: string;
  state: { before?: string; after?: string; unchanged: boolean };
  ledger: ResourceLedger;
}
```

## Build and test

This repository is a single Cargo workspace. No sibling checkout is required.

```bash
git clone https://github.com/AdityaVG13/zerostack
cd zerostack

# Build the host
cargo build -p zero-kernel

# Focused host contract
cargo test -p zero-kernel --test direct_host

# Frame syntax and codemode
cargo test -p zero-codemode --test syntax
```

Use the narrowest relevant test target during development. Domain tests live under `tests/fszero`, `tests/graphzero`, `tests/tokenzero`, and `tests/support`. Name the package and exact `--lib`, `--bin`, or `--test` target.

The CLI binary is `zero-kernel` in `crates/zerostack/zero-kernel`. It exposes `doctor`, `health`, `exec`, `mcp` (when built with the `mcp-carrier` feature), and `migrate`. `exec` reads one cell from stdin and is intended for diagnostics, not as the production model path.

Reference benchmark evidence is under `benchmarks/`. Reproduce the host lifecycle and fixtures with:

```bash
node benchmarks/zero-kernel-reference.cjs
```

Retained samples, environment metadata, and method are in `benchmarks/zero-kernel-reference.json`.

## Distribution scaffolds

Release packaging is intentionally pre-release. The repository keeps all package-manager work under `packaging/package/`:

- `homebrew/zerostack.rb` is a head-only formula that builds `zero-kernel` from source.
- `npm/` validates and packs the real `@zerostack/zero-kernel` package from `bindings/node`.
- `pi/` is a private Pi package scaffold with a local `/zerostack` status command. It does not register native tools yet.

```bash
brew install --HEAD --build-from-source packaging/package/homebrew/zerostack.rb
npm --prefix packaging/package/npm run validate
pi install ./packaging/package/pi
```

Run the combined FSZero, GraphZero, and TokenZero example with:

```bash
node demo/run.js
```

## Node embedding

The Node binding is `crates/zerostack/zero-kernel-node`; its package name is `@zerostack/zero-kernel`. It is an asynchronous N-API class that embeds the same Rust host in-process. It selects an exact platform prebuild or the explicit development override. It does not compile, download, or start a service at runtime.

Build and stage a prebuild from source:

```bash
cargo build --profile release-node -p zero-kernel-node
./packaging/package/npm/build-prebuild.sh --stage-only
```

Embed in an application:

```javascript
const { ZeroKernel } = require("@zerostack/zero-kernel");

const kernel = new ZeroKernel({ root: process.cwd(), sessionId: "example" });
await kernel.initialize();

const response = await kernel.executeCell("return await z.read('README.md');");

await kernel.shutdown();
```

The binding exposes `initialize()`, `executeCell(source, signal?)`, `status()`, and `shutdown()`. A ZeroKernel failure is a structured terminal outcome with `outcome`, `error`, `event`, `state`, and `ledger`. Do not rerun the same source through an unsandboxed fallback. For development only, `ZERO_KERNEL_NATIVE_ADDON` may point to an explicit addon file. `bindings/node/package.json` declares the package metadata and supported Node range.

## Documentation

| Document | Purpose |
| --- | --- |
| `docs/README.md` | Index of the published documentation set |
| `docs/architecture.md` | Authority boundaries, lifecycle, effects, and scheduling |
| `docs/components.md` | FSZero, GraphZero, TokenZero, and hub responsibilities |
| `docs/racc/RACC.md` | Recovery-aware output projection and exact expansion |
| `docs/fszero/` | Filesystem and byte authority |
| `docs/graphzero/` | Structure and query authority |
| `docs/tokenzero/` | Output economics and accounting |
| `contracts/README.md` | Machine-readable contract inventory and change rules |
| `crates/zerostack/zerostack-conformance/CONTRACT.md` | Shared executable conformance contract |
| `packaging/README.md` | Pre-release package-manager layout |
| `benchmarks/README.md` | Benchmark harness and evidence notes |

Typed contracts live in `contracts/` and `crates/zerostack/zero-abi`. Historical execution catalogs remain available through Git history rather than as supported surfaces.

## Contributing and security

Read `CONTRIBUTING.md` before changing engine contracts or the six-operation surface. Report vulnerabilities through `SECURITY.md`. ZeroStack is harness-agnostic and can be embedded by any compatible caller.

## License

MIT. See `LICENSE`.
