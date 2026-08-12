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

## Native runtime and MCP compatibility

The canonical runtime is `zsx-core`. It embeds FSZero, GraphZero, and TokenZero domain adapters in one process. The `zsx` executable, Pi adapter, OMP adapter, and Node binding all use that same session authority, including configured Pi subagents. No worker process, session socket, or daemon exists.

`zero-mcp` is a separate optional FastMCP compatibility carrier. A deployment chooses native CodeMode or MCP registration for a harness surface. It never exposes both catalogs at once.

Read [CodeMode and MCP compatibility](docs/codemode.md).

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

The production runtime links the three engine domain APIs through reviewed, pinned dependencies. Generic interpreter, lifecycle, store, ref, ABI, and MCP authority remains in this hub.

## Installation status

The native CLI and Node binding are verified from source. Thin private adapters live at `pi/packages/pi-zsx` and `omp/packages/omp-zsx-native` in `pi-stack`. Public registry publication and Windows native verification remain separate release gates.

## Roadmap

- Publish reproducible end-to-end benchmark methodology.
- Publish signed native Node prebuilds and the thin Pi and OMP packages.
- Add supported adapters that select the optional MCP compatibility carrier.

## Current limitations

ZeroStack is not yet a complete public install. Only TokenZero is publicly available, benchmark artifacts may describe evolving implementations, and private components can change before release. Status labels in this README are the source of truth.
