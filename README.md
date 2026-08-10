# ZeroStack

**Recovery-aware context infrastructure for AI coding agents.**

ZeroStack combines three focused engines into one context-efficient system. TokenZero compresses tool output, FSZero understands the live filesystem, and GraphZero answers structural code questions. One by one they are useful. Together they turn bulky intermediate results into small, recoverable references that agents can compose without flooding their context window.

> **Project status:** TokenZero is public. FSZero and GraphZero are private and under active development. This repository is the canonical documentation, benchmark, and conformance hub. It does not contain the engine implementations.

## The stack

| Component | Purpose | Status |
| --- | --- | --- |
| [TokenZero](https://github.com/AdityaVG13/tokenzero) | Compresses, deduplicates, and selectively expands tool output | Public |
| FSZero | Provides live filesystem reads, search, planning, and safe mutations | Private, in development |
| GraphZero | Provides repository orientation, dependency analysis, impact, and recall | Private, in development |

The engines remain independently useful and independently versioned. ZeroStack defines how their results compose.

~~~text
FSZero reads and changes files ─┐
GraphZero maps code structure ──┼─> recoverable refs ─> agent context
TokenZero compacts and expands ─┘
~~~

See [component overviews](docs/components.md) and [architecture](docs/architecture.md).

## RACC: recover, do not repeat

ZeroStack is built on **recovery-aware context compression (RACC)**. Large tool results are stored outside the model context and replaced with typed handles:

- `tz://` for TokenZero results
- `fz://` for FSZero results
- `gz://` for GraphZero results

A ref is not a lossy summary. It is a compact pointer to recoverable output. Agents can pass refs between steps, expand only the exact lines or symbols they need, and avoid paying repeatedly for the same bytes. This keeps the working context small while preserving access to evidence.

Read [RACC and typed refs](docs/racc.md).

## Two deployment modes

Each engine supports exactly two integration modes:

1. **Standard MCP adapter** -- the harness invokes individual MCP tools.
2. **CodeMode** -- the harness executes a JavaScript plan in a constrained sandbox, batching or parallelizing many typed calls in one round trip.

A deployment must choose **one mode only**. Never register the standard MCP adapters alongside CodeMode for the same engines. The active ZeroStack deployment uses **CodeMode only**.

CodeMode follows the Cloudflare-style model: the agent writes a small JavaScript plan against a typed `zero` surface, composes calls locally, and returns only the final values or refs. This reduces round trips as well as visible tokens.

Read [CodeMode and MCP mode](docs/codemode.md).

## Why the combination matters

- FSZero finds the exact live bytes and performs controlled changes.
- GraphZero narrows work to relevant definitions, callers, tests, and impact paths.
- TokenZero keeps every intermediate result recoverable without keeping it visible.
- CodeMode lets the three engines run as one batched workflow.

The result is a context pipeline, not three unrelated tools.

## Repository contents

| Path | Contents |
| --- | --- |
| [crates/](crates/) | Shared foundation crates used by all three engines |
| [docs/](docs/) | Public architecture, RACC, CodeMode, and component guides |

Engine source belongs in the three engine repositories, not here. Other harnesses and CLIs should build engine backends from each repository's `origin/main`; this hub is the canonical aggregation and documentation point.

## Shared foundation crates

The engines share contract-critical infrastructure through small foundation crates in [crates/](crates/):

- **zero-abi** -- canonical JSON encoding, JSON Schema normalization and structural comparison, the operation contract digest, and raw-worker v2 as the shared canonical dispatch contract.
- **zero-ref** -- the ZeroRef v1 portable blob ref: parser, canonical formatter, fragment selection, and the shared golden-vector fixture. One grammar, one selection algebra, byte-identical across engines.
- **zero-store** -- the canonical content-addressed store layout (blobs/sha256/hh/hash) with the crash-safe, concurrency-safe publish protocol and digest-verified reads.
- **zero-process** -- native process identity, owner-death notification, and exact child-tree lifecycle primitives. Engines keep only thin compatibility adapters over this hub authority.
- **zerostack-machine-permit** -- the shared machine-permit issue/verify surface and scoped permit-base contract for isolating permit roots. It delegates generic process identity to `zero-process`.

Each engine still ships as a single statically linked binary with no runtime dependency on the others. Foundation crates are consumed at build time via pinned git dependencies, and each engine adopts updates on its own schedule.

## Installation status

TokenZero installation is documented in its [public repository](https://github.com/AdityaVG13/tokenzero).

A unified Pi package, **pi-zerostack**, is in private development. It will eventually install and configure the complete stack. It is referenced here for roadmap clarity but is not published or supported for external installation yet. FSZero and GraphZero will receive public installation instructions when their repositories are released.

## Roadmap

- Stabilize FSZero and GraphZero APIs and publish their repositories.
- Finalize cross-engine RACC and conformance contracts.
- Publish reproducible end-to-end benchmark methodology.
- Release the pi-zerostack package with CodeMode-first setup.
- Add supported adapters for harnesses that choose standard MCP mode.

## Current limitations

ZeroStack is not yet a complete public install. Only TokenZero is publicly available, benchmark artifacts may describe evolving implementations, and private components can change before release. Status labels in this README are the source of truth.
